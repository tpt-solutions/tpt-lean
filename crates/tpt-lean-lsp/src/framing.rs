// SPDX-License-Identifier: MIT OR Apache-2.0
//! LSP base protocol: `Content-Length` headers followed by a UTF-8 JSON body.

use serde_json::Value;
use std::io::{BufRead, Write};
use tpt_lean_core::{Error, ErrorCode, Result};

/// Largest message accepted from an editor. Anything bigger is refused rather than buffered.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Read one framed message. `Ok(None)` at a clean end of input.
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return if saw_header {
                Err(protocol("input ended inside a message header"))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| protocol("invalid Content-Length"))?;
            length = Some(n);
        }
    }
    let n = length.ok_or_else(|| protocol("message without Content-Length"))?;
    if n > MAX_MESSAGE_BYTES {
        return Err(protocol("message too large"));
    }
    let mut body = vec![0u8; n];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).map_err(|e| protocol(&format!("invalid JSON: {e}")))?;
    Ok(Some(value))
}

/// Write one framed message and flush.
pub fn write_message<W: Write>(writer: &mut W, message: &Value) -> Result<()> {
    let body = message.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()?;
    Ok(())
}

fn protocol(detail: &str) -> Error {
    Error::new(ErrorCode::InvalidArgument, format!("LSP framing: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

    #[test]
    fn round_trip_two_messages() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "id": 1, "method": "a"})).unwrap();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "method": "é ☃"})).unwrap();
        let mut reader = BufReader::new(Cursor::new(buf));
        let first = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(first["method"], "a");
        let second = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(second["method"], "é ☃");
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn content_length_counts_bytes() {
        let body = "{\"x\":\"é\"}";
        let raw = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(Cursor::new(raw.into_bytes()));
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["x"], "é");
    }

    #[test]
    fn missing_length_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(b"X-Other: 1\r\n\r\n{}".to_vec()));
        assert!(read_message(&mut reader).is_err());
    }

    #[test]
    fn truncated_header_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(b"Content-Length: 4\r\n".to_vec()));
        assert!(read_message(&mut reader).is_err());
    }
}
