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
| `reqwest` (rustls, http2, gzip, brotli; default features off) | HTTP client (PLAN.md section 2). Default features are off so native-tls/OpenSSL never enters the tree. |
| `wiremock` (dev) | Mock HTTP server for fetcher and end-to-end tests (PLAN.md section 2). |
| `flate2` (dev) | Build gzip bodies to test transparent decompression and the decompressed size cap. |
| `tokio` (rt, sync, time, macros) in the scheduler | Timers, semaphores, `JoinSet` and `select!` for the scheduler loop. |
| `tokio-util` | `CancellationToken` shared by the scheduler and the HTTP server for graceful shutdown. |
| `fastrand` | Cheap uniform random numbers for jitter; no need for a cryptographic RNG. |
| `sqlx` (dev, scheduler) | Tests flip a watch to paused until the API has a pause operation. |
| `humantime` | Parse `--every 5m` and print intervals and RFC 3339 timestamps without pulling in a date library. |
| `tokio-util` (binary) | Share one `CancellationToken` between the HTTP server and the scheduler. |
| `wiremock`, `tempfile` (dev, binary) | End-to-end tests that run the real binary against a mock server and a throwaway data directory. |
| `regex` | Patterns for ignore/trigger/block conditions and the regex steps (PLAN.md section 2). Linear-time matching, so hostile patterns cannot stall a check. |

## Text processor (Phase 1)

HTML is rendered with `html2text` in plain mode at a width of 10,000 columns so text is never re-wrapped, and without link decoration, which means link targets are not part of the compared text yet. Non-HTML bodies pass through untouched. A response is treated as HTML when its `Content-Type` says so, or by sniffing a doctype or `<html` when the header is missing. All text is normalised (LF endings, trailing whitespace trimmed, blank runs collapsed) before hashing so cosmetic differences never register as changes.

## Charset decoding

The charset comes from the `Content-Type` header and defaults to UTF-8. A byte-order mark in the body wins over the header, malformed bytes decode to U+FFFD instead of failing the check, and charsets that encoding_rs maps to its REPLACEMENT encoding (iso-2022-kr, iso-2022-cn, hz-gb-2312) are treated as unknown, because REPLACEMENT turns every page into a single U+FFFD and would hide all changes. When the header gives no usable charset, the first 1024 bytes of an HTML or XML body (or of a body with no content type) are searched for a declaration: `<meta charset>`, `<meta http-equiv="content-type">`, and the `encoding` of a leading `<?xml?>` declaration. `<meta>` is only read in HTML, never in other XML, and nothing is read in JSON or plain text. As in browsers, a declared UTF-16 means UTF-8 (the page is readable as ASCII, so it cannot be UTF-16) and `x-user-defined` means windows-1252. Order: BOM, header, declaration, UTF-8. Accepted one-time effect: a page that declares a charset the header leaves out is decoded differently than before, so its hash changes once on the first check after upgrading.

## Snapshot storage and check recording

Both `record_success` and `record_error` run in a `BEGIN IMMEDIATE` transaction so a `vigil check` run next to `vigil serve` queues for the write lock instead of failing with `SQLITE_BUSY_SNAPSHOT`. A snapshot is stored only when the blake3 of the processed text differs from `watches.last_hash`. A 304 or an identical body only updates `last_checked_at`. The first successful check is logged as `changed` in `check_log` because its outcome column only allows `unchanged`, `changed` and `error`. Retention keeps at least one snapshot even if a watch says 0, and the check log is trimmed to the newest 500 rows per watch on every write.

## Watch ids on the command line

Ids are UUID v7, whose leading 48 bits are a millisecond timestamp, so watches created in the same sitting share long prefixes. The CLI therefore accepts a full id or a unique *suffix* of at least 6 characters, never a prefix.

## Runtime SQL queries (revisited)

Phase 1 adds about a dozen queries. They stay runtime-checked `sqlx::query(...)` calls and are covered by integration tests against a real SQLite file, which keeps builds free of `DATABASE_URL` and offline query data.

## Fetcher behaviour

TLS is rustls through reqwest's `rustls` feature, which uses the aws-lc-rs crypto provider and the platform certificate verifier; `cargo tree` shows no OpenSSL or native-tls. Revisit armv7 musl builds in Phase 6. Redirects stop after 10 hops, cookies are off, and proxies are off too: the client calls `no_proxy()` because reqwest otherwise reads `HTTP_PROXY`/`ALL_PROXY` from the environment, which would silently reroute every fetch (proxy support comes later, per watch and as a global setting), and only `http`/`https` URLs are accepted, so `file://` is never fetched. The size cap applies to the decompressed body, checked chunk by chunk, so a small gzip bomb cannot slip past a `Content-Length` check. `If-None-Match`/`If-Modified-Since` are sent only when the watch has stored validators, and a 304 is accepted only in that case; an unsolicited 304 is reported as an error. Bodies are returned as `Vec<u8>` rather than `bytes::Bytes` to avoid a direct dependency.

## Scheduler design

PLAN.md describes workers pulling from a bounded channel. The scheduler instead spawns one task per due check into a `JoinSet`. Each task first takes its host slot and then a global slot, so a check queued behind a busy host never holds a global slot, which a fixed worker pool would not avoid without head-of-line blocking. One task owns a min-heap of `(due, watch)`; heap items that no longer match their entry are skipped when popped. The scheduler talks to its watches and checks through the `Backend` trait, which lets tests drive it with a fake and a paused clock, and the real `Checker` implements it. The watch list is reloaded every 30 seconds, so adds, removals and pauses take effect without a signal. Delays are `interval * 2^errors` capped at 6 hours and never below the interval, plus random jitter, either the watch's `jitter_secs` or a tenth of the interval up to 60 seconds. Intervals are kept between one second and 365 days (`vigil watch add` rejects values outside that, and the scheduler clamps whatever is stored), all waits are saturating and bounded so a corrupt or absurd value cannot overflow time arithmetic, and a last-check time in the future (the clock was set back) delays a watch by at most one interval. On shutdown the scheduler stops starting checks (a check still waiting for a concurrency slot is dropped, not started) and waits up to 30 seconds for the ones already running.

## Check errors

A page that cannot be fetched or processed is not an error of the checker: it is recorded on the watch and in the check log, and `Checker::check` returns it as `CheckResult::Failed`. Only store failures, including a deleted watch, are returned as `Err`. Error messages include the source chain on one line, for example `request failed: error sending request for url (...): client error (Connect): ...`.

## Command-line behaviour

`vigil watch add` defaults to checking every hour (`--every 1h`) and rejects intervals under one second. `vigil watch remove` is not in PLAN.md but is the only way to undo a mistyped add before the UI exists. Commands accept a full watch id or the last six or more characters of it. `vigil check` exits non-zero when the page could not be checked, so it works in scripts; `vigil diff` with fewer than two snapshots prints "nothing to compare yet" and exits zero. Diffs are colored only when stdout is a terminal and `NO_COLOR` is unset. One-shot commands log at `warn` to stderr so stdout only carries their results; `serve` logs at `info` to stdout. CLI output is plain English in the binary and is not covered by the `locales/*.toml` rule, which is about web templates. `vigil serve` runs the scheduler next to the HTTP server and shares one cancellation token with the server's graceful shutdown.

## Unreadable watch rows

`list_watches` logs and skips a row it cannot interpret instead of failing, because the scheduler and `vigil watch list` both depend on it and a single bad row would otherwise stop all checking. `get_watch` still returns the error for that row.

## Scheduler failure stops the server

`vigil serve` watches the scheduler task. If it ends before a shutdown was requested (for example after a panic), `serve` exits with an error instead of carrying on answering `/healthz` while nothing is checked, so systemd or Docker can restart it.

## Request headers

Per-watch headers replace a default of the same name (`User-Agent`, `If-None-Match`, `If-Modified-Since`) instead of being sent beside it. A header that makes the request conditional also makes a 304 acceptable. Errors about an unusable header name the header but never include its value, since values often hold tokens and these messages end up in the check log. Custom headers are still forwarded if a page redirects to another host; restricting that is future work.

## Patterns

A pattern is a plain substring, matched case-insensitively with Unicode simple case folding (so Cyrillic works), or `/regex/flags`. The regex form needs a non-empty body between two slashes and only the flags `i m s x U` after the last slash; anything else is plain text. That makes `//`, `/` and `/foo/g` plain, and `/api/v1/` the regex `api/v1`, which also matches a path with that text in it. Wrap a path in a character class or add a flag (`/\/api\/v1\//i`) to be exact. Empty patterns are rejected because they would match every line. All regexes, including the raw ones in the regex steps, are built with a 1 MiB size limit, and error messages keep only the one-line reason so they fit the check log.
