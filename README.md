# Vigil

A lightweight, self-hosted website change detection service written in Rust.
One static binary, SQLite, low memory.

> **Status:** early development (Phase 0: skeleton). Not usable yet.

## Quick start

```sh
cargo run -- serve
curl http://127.0.0.1:5800/healthz   # -> ok
```

Configuration is read from `vigil.toml` (optional) and `VIGIL_*` environment
variables; see [docs/configuration.md](docs/configuration.md).

## Layout

See [PLAN.md](PLAN.md) for the roadmap and [docs/decisions.md](docs/decisions.md)
for design decisions.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
