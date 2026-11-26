# Changelog

All notable changes to `tpt-lean-pipe` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published (`publish = false`); see
[PUBLISHING.md](../../PUBLISHING.md) for the blocker it places on `tpt-lean-server`.

### Added
- Windows named-pipe server and client for the API server: bind, accept, connect, and exchange
  messages.
- Default pipe name `\\.\pipe\tpt-lean-<hash>`, with the default security descriptor.
- Refusal of a second server on the same pipe name.
- Tested on Windows: connect, exchange, exclusive bind, and a clean end of a reply.

### Known limitations
- Windows only. On other platforms the server uses a Unix domain socket instead.
