# distribution-rs

A high-performance OCI Distribution Spec v1.1 registry implementation in Rust.

## Status

Early development. Not yet usable.

## Goals

- Full OCI Distribution Spec v1.1 conformance
- Single binary, zero external dependencies
- Pluggable storage backends (local filesystem, S3-compatible)
- SQLite-based metadata (tags, repo-blob links, upload sessions)
- Built-in auth, TLS, garbage collection

## Building

```bash
cargo build
```

## License

MIT OR Apache-2.0
