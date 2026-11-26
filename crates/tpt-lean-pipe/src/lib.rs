// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) TPT Solutions
//! Named pipes for the API server on Windows.
//!
//! A server creates a pipe instance named `\\.\pipe\<name>`, waits for a client to connect, and
//! hands the connection out as a [`File`]. Each accepted connection is served by its own instance,
//! so the next client can connect while the previous one is being served.
//!
//! The pipes are created with the default security descriptor, which gives access to the creating
//! user, administrators and the system account, and not to other ordinary users. This is the same
//! protection the owner-only Unix socket gives.
//!
//! The first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second server with the
//! same name is refused while the first one runs.
//!
//! On other platforms the functions return an `Unsupported` error.

use std::fs::File;
use std::io;

/// The full pipe path for a short name: `\\.\pipe\<name>`.
pub fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

/// The client end of a pipe. When the server closes its end, a read returns the data the server
/// wrote and then end-of-stream, instead of Windows' broken-pipe error. Pipes have no half-close, so
/// this is how a client sees the end of a reply.
#[derive(Debug)]
pub struct Client(File);

impl io::Read for Client {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            other => other,
        }
    }
}

impl io::Write for Client {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Open a client connection to the pipe `name`. Fails when no server is listening.
#[cfg(windows)]
pub fn connect(name: &str) -> io::Result<Client> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path(name))
        .map(Client)
}

#[cfg(not(windows))]
pub fn connect(_name: &str) -> io::Result<Client> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "named pipes need Windows",
    ))
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

    use super::pipe_path;

    type Handle = isize;
    const INVALID_HANDLE_VALUE: Handle = -1;

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const BUFFER_SIZE: u32 = 64 * 1024;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer: u32,
            in_buffer: u32,
            default_timeout: u32,
            security: *const c_void,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// One pipe instance, wrapped as a file so it can be read and written.
    fn create_instance(path: &[u16], first: bool) -> io::Result<File> {
        let open_mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: `path` is a NUL-terminated UTF-16 string that outlives the call; the security
        // attributes are null, which selects the default descriptor.
        let handle = unsafe {
            CreateNamedPipeW(
                path.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: GetLastError has no preconditions.
            let code = unsafe { GetLastError() };
            return Err(if code == ERROR_ACCESS_DENIED && first {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "a server is already listening on this pipe",
                )
            } else {
                io::Error::from_raw_os_error(code as i32)
            });
        }
        // SAFETY: `handle` is a valid pipe handle that nothing else owns; File takes ownership and
        // closes it on drop.
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }

    /// The listening side of a named pipe.
    pub struct PipeListener {
        path: Vec<u16>,
        /// The instance clients connect to next.
        next: File,
    }

    impl PipeListener {
        pub fn bind(name: &str) -> io::Result<Self> {
            let path = wide(&pipe_path(name));
            let next = create_instance(&path, true)?;
            Ok(Self { path, next })
        }

        /// Wait for the next client. The connection is returned as a file; a fresh instance is
        /// created for the following client.
        pub fn accept(&mut self) -> io::Result<File> {
            let handle = self.next.as_raw_handle() as Handle;
            // SAFETY: `handle` belongs to `self.next`, which stays alive for the call.
            let ok = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            if ok == 0 {
                // SAFETY: GetLastError has no preconditions.
                let code = unsafe { GetLastError() };
                // A client that connected before ConnectNamedPipe was called is still connected.
                if code != ERROR_PIPE_CONNECTED {
                    return Err(io::Error::from_raw_os_error(code as i32));
                }
            }
            let fresh = create_instance(&self.path, false)?;
            Ok(std::mem::replace(&mut self.next, fresh))
        }
    }
}

#[cfg(windows)]
pub use win::PipeListener;

#[cfg(not(windows))]
pub struct PipeListener;

#[cfg(not(windows))]
impl PipeListener {
    pub fn bind(_name: &str) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "named pipes need Windows",
        ))
    }

    pub fn accept(&mut self) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "named pipes need Windows",
        ))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn unique(tag: &str) -> String {
        format!("tpt-lean-pipe-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn client_and_server_exchange_lines() {
        let name = unique("echo");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let conn = listener.accept().unwrap();
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut writer = conn;
            writeln!(writer, "echo: {}", line.trim()).unwrap();
        });
        let mut client = connect(&name).unwrap();
        writeln!(client, "hello").unwrap();
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        server.join().unwrap();
        assert_eq!(reply.trim(), "echo: hello");
    }

    #[test]
    fn a_second_server_with_the_same_name_is_refused() {
        let name = unique("exclusive");
        let _first = PipeListener::bind(&name).unwrap();
        let second = PipeListener::bind(&name);
        assert_eq!(second.err().map(|e| e.kind()), Some(io::ErrorKind::AddrInUse));
    }

    #[test]
    fn a_read_after_the_server_closes_ends_with_end_of_stream() {
        let name = unique("eof");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            writeln!(conn, "last line").unwrap();
            // Dropping the connection closes the server end.
        });
        let mut client = connect(&name).unwrap();
        let mut text = String::new();
        client.read_to_string(&mut text).unwrap();
        server.join().unwrap();
        assert_eq!(
            text,
            "last line
"
        );
    }

    #[test]
    fn connecting_without_a_server_fails() {
        assert!(connect(&unique("nobody")).is_err());
    }
}
