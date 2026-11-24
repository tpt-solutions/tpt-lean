// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local HTTP transport with server-sent events, on `std::net` only.
//!
//! `GET /sse` opens an event stream. Its first event, `endpoint`, gives the POST URL for that
//! session, `/messages?sessionId=...`. Each `POST` carries one JSON-RPC message, answered with
//! `202 Accepted`; the JSON-RPC reply goes out as a `message` event on the session's stream.
//!
//! Only loopback addresses are served. A request is refused when its `Host` is not localhost,
//! when it has an `Origin` that is not localhost, or when a POST is not `application/json`.
//! Browsers cannot send JSON cross-origin without a preflight, which this server never
//! answers, so a web page cannot drive the server through the browser.

use crate::{error_response, Server, PARSE_ERROR};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Largest accepted POST body.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_LINES: usize = 100;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
const KEEP_ALIVE: Duration = Duration::from_secs(15);

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

type Sessions = Mutex<HashMap<String, Sender<String>>>;

/// Serve MCP over loopback HTTP+SSE until the process stops.
///
/// Fails without binding when `addr` is not a loopback address.
pub fn serve_http(server: &Server, addr: SocketAddr) -> io::Result<()> {
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to serve MCP on a non-loopback address",
        ));
    }
    let listener = TcpListener::bind(addr)?;
    let stop = AtomicBool::new(false);
    serve_listener(server, listener, &stop)
}

/// Serve on a bound listener until `stop` is set. Open event streams end within one poll
/// interval of `stop` being set.
pub(crate) fn serve_listener(server: &Server, listener: TcpListener, stop: &AtomicBool) -> io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to serve MCP on a non-loopback address",
        ));
    }
    listener.set_nonblocking(true)?;
    let sessions: Sessions = Mutex::new(HashMap::new());
    thread::scope(|scope| {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let sessions = &sessions;
                    scope.spawn(move || {
                        let _ = handle_connection(server, sessions, stream, stop);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })
}

/// The parts of a request head this transport looks at.
struct Head {
    method: String,
    target: String,
    host: Option<String>,
    origin: Option<String>,
    content_type: Option<String>,
    content_length: Option<usize>,
}

fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed HTTP request")
}

fn read_head(reader: &mut impl BufRead) -> io::Result<Head> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let (method, target, version) = {
        let mut parts = line.trim_end().split(' ');
        let method = parts.next().filter(|s| !s.is_empty()).ok_or_else(malformed)?;
        let target = parts.next().ok_or_else(malformed)?;
        let version = parts.next().ok_or_else(malformed)?;
        (method.to_string(), target.to_string(), version.to_string())
    };
    if !version.starts_with("HTTP/1.") {
        return Err(malformed());
    }
    let mut head = Head {
        method,
        target,
        host: None,
        origin: None,
        content_type: None,
        content_length: None,
    };
    for _ in 0..MAX_HEADER_LINES {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(malformed());
        }
        let text = line.trim_end();
        if text.is_empty() {
            return Ok(head);
        }
        if let Some((name, value)) = text.split_once(':') {
            let value = value.trim().to_string();
            match name.trim().to_ascii_lowercase().as_str() {
                "host" => head.host = Some(value),
                "origin" => head.origin = Some(value),
                "content-type" => head.content_type = Some(value),
                "content-length" => head.content_length = value.parse().ok(),
                _ => {}
            }
        }
    }
    Err(malformed())
}

/// `localhost`, `127.0.0.1`, or `[::1]`, with or without a port.
fn host_allowed(host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

/// An absent `Origin` (non-browser clients) or a localhost origin.
fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => o
            .strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
            .is_some_and(|authority| host_allowed(Some(authority))),
    }
}

fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| {
        ct.split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
    })
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn respond(w: &mut impl Write, status: &str, body: &str) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    w.flush()
}

fn handle_connection(
    server: &Server,
    sessions: &Sessions,
    stream: TcpStream,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let Ok(head) = read_head(&mut reader) else {
        return respond(&mut writer, "400 Bad Request", "malformed request");
    };
    if !host_allowed(head.host.as_deref()) {
        return respond(&mut writer, "403 Forbidden", "host not allowed");
    }
    if !origin_allowed(head.origin.as_deref()) {
        return respond(&mut writer, "403 Forbidden", "origin not allowed");
    }
    let (path, query) = head.target.split_once('?').unwrap_or((head.target.as_str(), ""));
    match (head.method.as_str(), path) {
        ("GET", "/sse") => serve_sse(sessions, writer, stop),
        ("POST", "/messages") => {
            let Some(length) = head.content_length else {
                return respond(&mut writer, "411 Length Required", "content-length is required");
            };
            if length > MAX_BODY_BYTES {
                return respond(&mut writer, "413 Payload Too Large", "body too large");
            }
            if !is_json(head.content_type.as_deref()) {
                return respond(
                    &mut writer,
                    "415 Unsupported Media Type",
                    "content-type must be application/json",
                );
            }
            let Some(session) = query_param(query, "sessionId") else {
                return respond(&mut writer, "400 Bad Request", "missing sessionId");
            };
            let sender = sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(session)
                .cloned();
            let Some(sender) = sender else {
                return respond(&mut writer, "404 Not Found", "unknown session");
            };
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body)?;
            let parsed = serde_json::from_slice::<Value>(&body);
            // Record the request before answering, so a cancel sent after the 202 is seen.
            if let Ok(msg) = &parsed {
                if let Some(id) = msg.get("id").filter(|_| msg.get("method").is_some()) {
                    server.note_received(&id.to_string());
                }
            }
            respond(&mut writer, "202 Accepted", "")?;
            let reply = match parsed {
                Ok(msg) => server.handle(&msg),
                Err(e) => Some(error_response(Value::Null, PARSE_ERROR, &e.to_string())),
            };
            if let Some(reply) = reply {
                let _ = sender.send(reply.to_string());
            }
            Ok(())
        }
        ("GET", _) | ("POST", _) => respond(&mut writer, "404 Not Found", "not found"),
        _ => respond(&mut writer, "405 Method Not Allowed", "method not allowed"),
    }
}

/// Hold one event stream open and forward replies for its session until it closes.
fn serve_sse(sessions: &Sessions, mut writer: TcpStream, stop: &AtomicBool) -> io::Result<()> {
    let id = new_session_id();
    let (tx, rx) = channel::<String>();
    sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), tx);
    let result = (|| -> io::Result<()> {
        writer.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        )?;
        write!(writer, "event: endpoint\ndata: /messages?sessionId={id}\n\n")?;
        writer.flush()?;
        let mut last_write = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            match rx.recv_timeout(POLL) {
                Ok(msg) => write!(writer, "event: message\ndata: {msg}\n\n")?,
                Err(RecvTimeoutError::Timeout) => {
                    if last_write.elapsed() < KEEP_ALIVE {
                        continue;
                    }
                    writer.write_all(b": keep-alive\n\n")?;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
            writer.flush()?;
            last_write = Instant::now();
        }
    })();
    sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    result
}

/// An unguessable session id: two hashes seeded from the OS random state, a counter, and time.
fn new_session_id() -> String {
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(counter);
    h.write_u128(nanos);
    let a = h.finish();
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(a);
    format!("{a:016x}{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::Shutdown;

    fn server(name: &str) -> Server {
        let dir = std::env::temp_dir().join(format!("tpt-mcp-http-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Server::new(&dir).unwrap()
    }

    /// Sets the stop flag when dropped, so a failing assertion cannot hang the scope.
    struct StopOnDrop<'a>(&'a AtomicBool);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    fn connect(addr: SocketAddr) -> TcpStream {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s
    }

    /// Read one SSE event as (event, data), skipping comments.
    fn read_event(reader: &mut impl BufRead) -> (String, String) {
        let mut event = String::new();
        let mut data = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0, "stream closed");
            let line = line.trim_end();
            if line.is_empty() {
                if !data.is_empty() {
                    return (event, data);
                }
            } else if let Some(v) = line.strip_prefix("event: ") {
                event = v.to_string();
            } else if let Some(v) = line.strip_prefix("data: ") {
                data = v.to_string();
            }
        }
    }

    fn skip_headers(reader: &mut impl BufRead) -> String {
        let mut status = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if status.is_empty() {
                status = line.trim_end().to_string();
            }
            if line == "\r\n" {
                return status;
            }
        }
    }

    fn status_of(addr: SocketAddr, raw: &str) -> String {
        let mut s = connect(addr);
        s.write_all(raw.as_bytes()).unwrap();
        s.shutdown(Shutdown::Write).ok();
        let mut reader = BufReader::new(s);
        skip_headers(&mut reader)
    }

    #[test]
    fn refuses_non_loopback_before_binding() {
        let s = server("public");
        let addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        assert!(serve_http(&s, addr).is_err());
    }

    #[test]
    fn host_and_origin_checks() {
        assert!(host_allowed(Some("localhost:8080")));
        assert!(host_allowed(Some("127.0.0.1:1")));
        assert!(host_allowed(Some("[::1]:9")));
        assert!(!host_allowed(Some("evil.example:8080")));
        assert!(!host_allowed(Some("127.0.0.1.evil.example")));
        assert!(!host_allowed(None));
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://localhost:5173")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("null")));
    }

    #[test]
    fn json_content_type_is_recognised() {
        assert!(is_json(Some("application/json")));
        assert!(is_json(Some("Application/JSON; charset=utf-8")));
        assert!(!is_json(Some("text/plain")));
        assert!(!is_json(None));
    }

    #[test]
    fn session_ids_are_distinct() {
        let a = new_session_id();
        let b = new_session_id();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn foreign_host_is_forbidden_and_text_posts_are_refused() {
        let s = server("reject");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let _guard = StopOnDrop(&stop);
            scope.spawn(|| serve_listener(&s, listener, &stop));
            let forbidden = status_of(addr, "GET /sse HTTP/1.1\r\nHost: evil.example\r\n\r\n");
            assert!(forbidden.contains("403"), "{forbidden}");
            let cross_origin = status_of(
                addr,
                "GET /sse HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\n\r\n",
            );
            assert!(cross_origin.contains("403"), "{cross_origin}");
            let text = status_of(
                addr,
                "POST /messages?sessionId=x HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{}",
            );
            assert!(text.contains("415"), "{text}");
            let unknown = status_of(
                addr,
                "POST /messages?sessionId=nope HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            );
            assert!(unknown.contains("404"), "{unknown}");
        });
    }

    #[test]
    fn sse_session_round_trip() {
        let s = server("roundtrip");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let _guard = StopOnDrop(&stop);
            scope.spawn(|| serve_listener(&s, listener, &stop));

            let mut sse = connect(addr);
            sse.write_all(b"GET /sse HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .unwrap();
            let mut events = BufReader::new(sse);
            assert!(skip_headers(&mut events).contains("200"));
            let (kind, endpoint) = read_event(&mut events);
            assert_eq!(kind, "endpoint");
            assert!(endpoint.starts_with("/messages?sessionId="));

            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
            let mut post = connect(addr);
            write!(
                post,
                "POST {endpoint} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            post.shutdown(Shutdown::Write).ok();
            let mut reply = BufReader::new(post);
            assert!(skip_headers(&mut reply).contains("202"));

            let (kind, data) = read_event(&mut events);
            assert_eq!(kind, "message");
            let v: Value = serde_json::from_str(&data).unwrap();
            assert_eq!(v["id"], 1);
            assert_eq!(v["result"], json!({}));
        });
    }

    #[test]
    fn sse_tool_call_reply_arrives_on_stream() {
        let s = server("tool");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let _guard = StopOnDrop(&stop);
            scope.spawn(|| serve_listener(&s, listener, &stop));
            let mut sse = connect(addr);
            sse.write_all(b"GET /sse HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut events = BufReader::new(sse);
            skip_headers(&mut events);
            let (_, endpoint) = read_event(&mut events);

            let body = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tpt_lean_format_text","arguments":{"text":"def x := 1   \n"}}}).to_string();
            let mut post = connect(addr);
            write!(
                post,
                "POST {endpoint} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            post.shutdown(Shutdown::Write).ok();
            let mut sink = String::new();
            BufReader::new(post).read_to_string(&mut sink).ok();

            let (kind, data) = read_event(&mut events);
            assert_eq!(kind, "message");
            let v: Value = serde_json::from_str(&data).unwrap();
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            let inner: Value = serde_json::from_str(text).unwrap();
            assert_eq!(inner["formatted"], "def x := 1\n");
        });
    }
}
