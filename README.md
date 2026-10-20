# Vigil

A lightweight, self-hosted website change detection service written in Rust.
One static binary, SQLite, low memory.

> **Status:** early development (Phase 1: core loop). The command line works; the web UI, notifications and filters are not built yet.

## Quick start

```sh
cargo run -- watch add https://example.com --every 5m
cargo run -- watch list
cargo run -- check <id>     # <id>: full id or its last 6+ characters
cargo run -- diff <id>      # after the page has changed once

cargo run -- serve          # check on schedule; GET /healthz -> ok
```

Configuration is read from `vigil.toml` (optional) and `VIGIL_*` environment
variables; see [docs/configuration.md](docs/configuration.md).

## Layout

See [PLAN.md](PLAN.md) for the roadmap and [docs/decisions.md](docs/decisions.md)
for design decisions.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
