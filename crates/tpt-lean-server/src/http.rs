// SPDX-License-Identifier: MIT OR Apache-2.0
//! Loopback HTTP transport: `POST /v1/rpc` carries one JSON-RPC message per request.
//!
//! - The body is a single JSON-RPC request or notification. A notification gets `204 No Content`.
//! - `GET /v1/events?run_id=ID` streams the events of a build as Server-Sent Events (the latest
//!   build when `run_id` is omitted). The stream ends with an `event: done` carrying the summary.
//! - Other paths get `404`, other methods `405`.
//! - The `Host` header must name the loopback address (`127.0.0.1` or `localhost`), which blocks
//!   DNS-rebinding attacks from a browser on another site.
//! - Each connection carries one request and is then closed.
//!
//! Methods, permissions and path checks are the same as on the TCP transport: both call
//! [`Server::handle`].

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use std::time::Duration;

use crate::builds::BuildState;
use crate::{lock, Server, MAX_REQUEST_BYTES};

/// Path of the JSON-RPC endpoint.
pub const RPC_PATH: &str = "/v1/rpc";
/// Path of the build event stream.
pub const EVENTS_PATH: &str = "/v1/events";

/// Serve HTTP on `listener` until the server shuts down. Each connection gets its own thread.
pub fn serve_http(server: Arc<Server>, listener: TcpListener) -> io::Result<()> {
    for stream in listener.incoming() {
        if server.is_shutdown_requested() {
            break;
        }
        let stream = stream?;
        let server = Arc::clone(&server);
        thread::spawn(move || {
            let _ = serve_http_connection(&server, stream);
        });
    }
    Ok(())
}

/// Read one request from `stream`, answer it, and return.
pub fn serve_http_connection(server: &Server, mut stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    let mut content_length: Option<usize> = None;
    let mut host: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().ok();
            } else if name.eq_ignore_ascii_case("host") {
                host = Some(value.to_string());
            }
        }
    }

    if !host.as_deref().is_some_and(is_loopback_host) {
        return respond(&mut stream, 403, "Forbidden", None, &json!({"error": "host must be loopback"}));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method == "GET" && path == EVENTS_PATH {
        return serve_events(server, &mut stream, query);
    }
    if path != RPC_PATH {
        return respond(&mut stream, 404, "Not Found", None, &json!({"error": "unknown path"}));
    }
    if method != "POST" {
        return respond(&mut stream, 405, "Method Not Allowed", Some("POST"), &json!({"error": "use POST"}));
    }
    let Some(length) = content_length else {
        return respond(&mut stream, 411, "Length Required", None, &json!({"error": "Content-Length required"}));
    };
    if length > MAX_REQUEST_BYTES {
        return respond(&mut stream, 413, "Payload Too Large", None, &json!({"error": "request too large"}));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;

    let message: Value = match serde_json::from_slice(&body) {
        Ok(m) => m,
        Err(e) => {
            let reply = json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": crate::PARSE_ERROR, "message": e.to_string() }
            });
            return respond(&mut stream, 400, "Bad Request", None, &reply);
        }
    };
    match server.handle(&message) {
        Some(reply) => respond(&mut stream, 200, "OK", None, &reply),
        None => {
            write!(stream, "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")?;
            stream.flush()
        }
    }
}

/// `Host` header values that name the loopback interface, with or without a port.
pub fn is_loopback_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) && !name.is_empty() => name,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

/// Stream the events of one build as Server-Sent Events, then a `done` event with its summary.
/// Events are read from the build as they are recorded, so the stream follows a running build.
fn serve_events(server: &Server, stream: &mut TcpStream, query: &str) -> io::Result<()> {
    let run_id = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "run_id")
        .map(|(_, value)| value.to_string());
    let Some(build) = server.build_stream(run_id.as_deref()) else {
        return respond(stream, 404, "Not Found", None, &json!({"error": "no such build"}));
    };
    stream.write_all(
        b"HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: close

",
    )?;
    let mut sent = 0usize;
    loop {
        // Read the state first: once a build is finished, all its events are already recorded.
        let finished = match &*lock(&build.state) {
            BuildState::Finished(summary) => Some(summary.clone()),
            BuildState::Running => None,
        };
        let batch: Vec<Value> = {
            let events = lock(&build.events);
            events[sent.min(events.len())..].to_vec()
        };
        for event in &batch {
            let name = event["event"].as_str().unwrap_or("message");
            write!(stream, "event: {name}
data: {event}

")?;
            sent += 1;
        }
        if let Some(summary) = finished {
            write!(stream, "event: done
data: {summary}

")?;
            stream.flush()?;
            break;
        }
        stream.flush()?;
        if server.is_shutdown_requested() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    allow: Option<&str>,
    body: &Value,
) -> io::Result<()> {
    let text = body.to_string();
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        text.len()
    );
    if let Some(allow) = allow {
        head.push_str(&format!("Allow: {allow}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_host_names() {
        assert!(is_loopback_host("127.0.0.1:7373"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("localhost:80"));
        assert!(is_loopback_host("[::1]:9"));
        assert!(!is_loopback_host("evil.example:7373"));
        assert!(!is_loopback_host("127.0.0.1.evil.example"));
        assert!(!is_loopback_host(""));
    }
}
