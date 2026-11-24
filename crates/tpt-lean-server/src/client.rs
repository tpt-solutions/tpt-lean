// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal blocking JSON-RPC client over the server's TCP transport. Used by `status` and `stop`.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

/// Send one request to a server on 127.0.0.1:`port` and return the parsed response object.
pub fn call(port: u16, method: &str, params: Value, timeout: Duration) -> io::Result<Value> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{request}")?;
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
