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
| `blake3` | Fast content hash for change detection (PLAN.md section 2). |
| `similar` | Line diffs between snapshots (PLAN.md section 2). |
| `html2text` | HTML to readable text for the `text` processor (PLAN.md section 2). |
| `encoding_rs` | Charset decoding of fetched bodies; also handles BOM sniffing. |
| `uuid` (serde) | Typed watch ids; the store adds the `v7` feature to generate them. |
| `insta` (dev) | Snapshot tests of the text pipeline on fixture pages. |
| `serde_json` (dev) | Check the stored JSON shape of core types in tests. |
| `serde_json` | Read and write the JSON `TEXT` columns (fetch settings, schedule). |
| `uuid` (v7) | Time-ordered ids for watches and snapshots (PLAN.md section 4). |
| `zstd` | Snapshot compression (PLAN.md section 2). Binds the C library; if cross builds get painful in Phase 6, `ruzstd` is the pure-Rust fallback. |

## Text processor (Phase 1)

HTML is rendered with `html2text` in plain mode at a width of 10,000 columns so text is never re-wrapped, and without link decoration, which means link targets are not part of the compared text yet. Non-HTML bodies pass through untouched. A response is treated as HTML when its `Content-Type` says so, or by sniffing a doctype or `<html` when the header is missing. All text is normalised (LF endings, trailing whitespace trimmed, blank runs collapsed) before hashing so cosmetic differences never register as changes.

## Charset decoding

The charset comes from the `Content-Type` header and defaults to UTF-8. A byte-order mark in the body wins over the header, and malformed bytes decode to U+FFFD instead of failing the check. `<meta charset>` sniffing is deferred to Phase 2.

## Snapshot storage and check recording

Both `record_success` and `record_error` run in a `BEGIN IMMEDIATE` transaction so a `vigil check` run next to `vigil serve` queues for the write lock instead of failing with `SQLITE_BUSY_SNAPSHOT`. A snapshot is stored only when the blake3 of the processed text differs from `watches.last_hash`. A 304 or an identical body only updates `last_checked_at`. The first successful check is logged as `changed` in `check_log` because its outcome column only allows `unchanged`, `changed` and `error`. Retention keeps at least one snapshot even if a watch says 0, and the check log is trimmed to the newest 500 rows per watch on every write.

## Watch ids on the command line

Ids are UUID v7, whose leading 48 bits are a millisecond timestamp, so watches created in the same sitting share long prefixes. The CLI therefore accepts a full id or a unique *suffix* of at least 6 characters, never a prefix.

## Runtime SQL queries (revisited)

Phase 1 adds about a dozen queries. They stay runtime-checked `sqlx::query(...)` calls and are covered by integration tests against a real SQLite file, which keeps builds free of `DATABASE_URL` and offline query data.
