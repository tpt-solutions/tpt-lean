# tpt-lean-pipe

The Windows named-pipe transport for the [tpt-lean](https://github.com/tpt-solutions/tpt-lean)
API server. It has a server side (bind a pipe and accept connections) and a client side (connect
to a running server).

This is the only crate in the workspace that calls Win32 directly. It is the only place where
`unsafe` is allowed for pipe handling; the rest of the workspace forbids it.

## Status

Early development (`0.1.0`). Not published: `publish = false`. `tpt-lean-server` depends on it
on Windows only. See [PUBLISHING.md](../../PUBLISHING.md) for why that blocks publishing the
server.

Tested on Windows: connect, exchange, exclusive bind, and a clean end of a reply.

## Pipe name

The API server listens on `\\.\pipe\tpt-lean-<hash>` by default. The `<hash>` is derived from the
project root, so each project gets its own pipe. The pipe uses the default security descriptor,
and a second server cannot bind the same name while the first is running.

## Usage

This crate is used through `tpt-lean-server`; it is not a public entry point. The server selects it
on Windows, and `tpt-lean server start`, `status` and `stop` work with it through the server's state
file.

## Testing

```
cargo test -p tpt-lean-pipe
```

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
