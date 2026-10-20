# Vigil

A lightweight, self-hosted website change detection service written in Rust.
One static binary, SQLite, low memory.

> **Status:** early development (Phase 2: pipeline). The command line works, including extraction steps (CSS, XPath, JSONPath, jq, regex), JSON and feed processors, and notification conditions; the web UI and notification channels are not built yet.

## Quick start

```sh
cargo run -- watch add https://example.com --every 5m
cargo run -- watch list
cargo run -- check <id>     # <id>: full id or its last 6+ characters
cargo run -- diff <id>      # after the page has changed once

# watch just one part of a page, and try it before saving
cargo run -- watch add https://example.com --pipeline '[{"type": "css", "selector": "h1"}]'
cargo run -- preview <id>

cargo run -- serve          # check on schedule; GET /healthz -> ok
```

Configuration is read from `vigil.toml` (optional) and `VIGIL_*` environment
variables; see [docs/configuration.md](docs/configuration.md). Steps and conditions
are described in [docs/pipeline.md](docs/pipeline.md).

## Layout

See [PLAN.md](PLAN.md) for the roadmap and [docs/decisions.md](docs/decisions.md)
for design decisions.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
