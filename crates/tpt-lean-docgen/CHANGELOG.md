# Changelog

All notable changes to `tpt-lean-docgen` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Static HTML site generation from syntax facts; no Lean process and no network access.
- Per-module pages, source pages with line anchors, namespace tree, and import graph.
- Client-side search index (`search-index.json`) and dark mode.
- JSON export of the API (`api.json`).
- Import edges to undocumented modules reported in the summary.
- Optional documentation of local dependency packages.

### Known limitations
- Output reflects syntax only; no types or elaborated statements.
