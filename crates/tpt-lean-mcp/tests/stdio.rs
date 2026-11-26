// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stdio transport while a tool call is running: a cancellation sent during the call stops it
//! and drops its reply, and the next request is still answered. Needs Lean; skipped without it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tpt_lean_mcp::{serve_stdio, Server};

/// Input fed from a channel, so the test can send lines while the server is busy.
struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            match self.rx.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// Output sent to a channel, so the test can read replies as they are written.
struct ChannelWriter(mpsc::Sender<Vec<u8>>);

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.send(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn lean_available() -> bool {
    tpt_lean_adapter::discover_toolchain().lean.is_some()
}

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-mcp-stdio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Slow.lean"),
        "#eval (List.range 400000000).foldl (· + ·) 0\n",
    )
    .unwrap();
    dir
}

#[test]
fn cancelling_a_running_check_stops_it_and_its_reply_is_dropped() {
    if !lean_available() {
        eprintln!("skipped: lean not found");
        return;
    }
    let dir = project();
    let server = Server::new(&dir).unwrap();

    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();
    let runner = std::thread::spawn(move || {
        let input = BufReader::new(ChannelReader {
            rx: in_rx,
            pending: Vec::new(),
        });
        serve_stdio(&server, input, ChannelWriter(out_tx)).unwrap();
    });

    let send = |v: Value| {
        let mut line = v.to_string();
        line.push('\n');
        in_tx.send(line.into_bytes()).unwrap();
    };
    let started = Instant::now();
    send(json!({"jsonrpc":"2.0","id":10,"method":"tools/call",
        "params":{"name":"tpt_lean_check_file","arguments":{"file":"Slow.lean"}}}));
    // Give the check time to start, then cancel it, then ask something quick.
    std::thread::sleep(Duration::from_millis(800));
    send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10}}));
    send(json!({"jsonrpc":"2.0","id":11,"method":"ping"}));
    drop(in_tx);

    let mut replies: Vec<Value> = Vec::new();
    let mut text = Vec::new();
    while let Ok(bytes) = out_rx.recv_timeout(Duration::from_secs(120)) {
        text.extend(bytes);
    }
    runner.join().unwrap();
    for line in BufReader::new(&text[..]).lines() {
        replies.push(serde_json::from_str(&line.unwrap()).unwrap());
    }

    assert!(
        replies.iter().all(|r| r["id"] != 10),
        "the cancelled call must not reply: {replies:?}"
    );
    assert!(
        replies.iter().any(|r| r["id"] == 11 && r["result"] == json!({})),
        "the request after the cancel must be answered: {replies:?}"
    );
    // A check of an unbounded evaluation finishes in minutes; a cancelled one must not.
    assert!(
        started.elapsed() < Duration::from_secs(90),
        "the cancelled check kept running for {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
