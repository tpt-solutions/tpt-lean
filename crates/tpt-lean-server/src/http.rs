// SPDX-License-Identifier: MIT OR Apache-2.0
//! Loopback HTTP transport: `POST /v1/rpc` carries one JSON-RPC message per request.
//!
//! - The body is a single JSON-RPC request or notification. A notification gets `204 No Content`.
//! - Only `POST /v1/rpc` is served. Other paths get `404`, other methods `405`.
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

use crate::{Server, MAX_REQUEST_BYTES};

/// Path of the JSON-RPC endpoint.
pub const RPC_PATH: &str = "/v1/rpc";

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
    if target != RPC_PATH {
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
