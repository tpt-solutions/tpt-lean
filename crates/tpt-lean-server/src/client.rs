// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal blocking JSON-RPC client over the server's transports (TCP and, on Unix, a domain
//! socket). Used by `status` and `stop`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

/// Where a running server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl Endpoint {
    /// Loopback TCP endpoint for `port`.
    pub fn localhost(port: u16) -> Self {
        Endpoint::Tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
    }
}

/// Send one request to a server on 127.0.0.1:`port` and return the parsed response object.
pub fn call(port: u16, method: &str, params: Value, timeout: Duration) -> io::Result<Value> {
    call_endpoint(&Endpoint::localhost(port), method, params, timeout)
}

/// Send one request to the server at `endpoint` and return the parsed response object.
pub fn call_endpoint(endpoint: &Endpoint, method: &str, params: Value, timeout: Duration) -> io::Result<Value> {
    match endpoint {
        Endpoint::Tcp(addr) => {
            let stream = TcpStream::connect_timeout(addr, timeout)?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            exchange(stream, method, params)
        }
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            let stream = UnixStream::connect(path)?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            exchange(stream, method, params)
        }
    }
}

fn exchange<S: Read + Write>(mut stream: S, method: &str, params: Value) -> io::Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no response"));
    }
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// True when a response object carries a `result` member.
pub fn is_ok_response(response: &Value) -> bool {
    response.get("result").is_some()
}
