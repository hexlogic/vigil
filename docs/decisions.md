# Decisions

One short paragraph per decision. Dependencies get a one-line reason (see CLAUDE.md).

## Toolchain pinned to an exact stable release

`rust-toolchain.toml` pins `1.99.1` (latest stable at project start, 2026-10) with the `minimal` profile plus `rustfmt` and `clippy`. `rust-version` in the workspace is `1.99`. CI reads the same file, so local and CI toolchains match.

## `unwrap`/`expect` enforced by lint, not just by convention

`clippy::unwrap_used` and `clippy::expect_used` are `warn` in `[workspace.lints]`; with `-D warnings` in CI they fail the build. `clippy.toml` allows them inside tests. Binary startup code that genuinely needs one uses a targeted `#[allow]` with a reason.

## `publish = false` until the name is cleared

The workspace sets `publish = false` so nothing can reach crates.io by accident. Remove it only after searching crates.io, GitHub and Docker Hub for name collisions (PLAN.md header).

## Storage conventions

IDs are UUID v7 stored as `TEXT` (sortable by creation time, no extra index tricks). Timestamps are `INTEGER` unix milliseconds. Nested watch configuration (`fetch`, `schedule`, `pipeline`, `conditions`) is stored as JSON `TEXT`: these structures evolve with the pipeline and are always read and written whole, so columns would only add migrations. Hashes (blake3) are `BLOB`. Snapshot content is a zstd `BLOB`.

## Runtime SQL queries, not `sqlx::query!`

Phase 0 uses runtime-checked queries so builds need no `DATABASE_URL` or offline query data. Revisit when the query surface grows (Phase 1+).

## Migrations run on open

`Store::open` runs the embedded migrations. The pre-migration backup of the SQLite file required by PLAN.md section 12 is deferred until a second migration exists (nothing to protect yet).

## Configuration layering

`figment`: built-in defaults, then an optional TOML file (`--config` / `VIGIL_CONFIG`, default `./vigil.toml` if present), then `VIGIL_*` environment variables (nested keys use `__`). `VIGIL_CONFIG` is excluded from the config keys because it selects the file rather than configuring the app.

## Dependencies

| Crate | Reason |
|---|---|
| `sqlx` (sqlite, runtime-tokio, migrate, macros) | Async SQLite with embedded migrations; bundled libsqlite3, no system dependency. |
| `thiserror` | Typed errors in library crates (CLAUDE.md). |
| `tempfile` (dev) | Throwaway databases in tests. |
| `tokio` (dev) | `#[tokio::test]` runtime for async tests. |
| `tokio` | Async runtime (multi-thread, signals, net) for the server. |
| `axum` | Web server framework (PLAN.md section 2). |
| `tower-http` (trace) | Request tracing layer for the HTTP server. |
| `tracing`, `tracing-subscriber` (env-filter, json) | Structured logging with `RUST_LOG` filtering and optional JSON output. |
| `clap` (derive, env) | CLI parsing; `--config` also reads `VIGIL_CONFIG`. |
| `figment` (toml, env) | Layered TOML + `VIGIL_*` env configuration. |
| `serde` (derive) | Config (de)serialization. |
| `anyhow` | Error handling in the binary (CLAUDE.md). |
| `tower` (util, dev), `http-body-util` (dev) | Drive the axum router in-process (`oneshot`) and read response bodies in tests. |
