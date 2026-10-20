# Vigil: implementation plan for Claude Code

> Working name: **Vigil** (rename freely; search crates.io, GitHub and Docker Hub for collisions before the first public release).
> Goal: a lightweight, open-source, self-hosted website change detection service written in Rust. Single static binary, low memory, runs well on a Raspberry Pi or a $4 VPS. Covers the core 80% of what people use changedetection.io for.

---

## 0. How to use this plan (instructions for Claude Code)

- Work **phase by phase** (section 9). Do not start a phase before the previous one passes its acceptance criteria.
- At the end of each phase: run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. All must pass.
- Commit at least once per phase with a conventional commit message (`feat:`, `fix:`, `chore:`, `docs:`, `test:`).
- After each phase, stop and print a short summary: what was built, what is stubbed, open questions. Wait for review before the next phase.
- When a decision is not covered here, pick the simplest option that keeps the single-binary goal, write it down in `docs/decisions.md` (one short paragraph per decision), and continue.
- **Clean-room rule:** this is an independent implementation. Studying changedetection.io's behaviour, docs and public API is fine. Do not copy its source code, templates, CSS or text.

Kickoff prompt:

```
Read PLAN.md fully. Then create CLAUDE.md from section 10, and start Phase 0.
Stop when Phase 0 acceptance criteria pass.
```

---

## 1. Goals and non-goals

### Goals (v1.0)
1. Watch web pages, JSON APIs, RSS/Atom feeds and plain text files for changes on a schedule.
2. Extract only the part you care about (CSS selector, XPath, JSONPath, jq, regex) and ignore noise.
3. Keep a history of snapshots and show readable diffs.
4. Notify through common channels (ntfy, Telegram, Discord, Slack, email, generic webhook) with templated messages.
5. Web UI, REST API, and a migration path from changedetection.io.
6. One binary, embedded UI assets, SQLite, no external services required.
7. Idle RAM target: **< 30 MB** with 100 watches in plain HTTP mode.

### Non-goals for v1.0 (roadmap, see section 11)
- Visual/screenshot diffs, "browser steps" (clicking, logging in), AI summaries.
- Multi-user accounts and permissions (single admin user only).
- Clustering or horizontal scaling.

---

## 2. Tech stack

| Concern | Crate(s) | Notes |
|---|---|---|
| Async runtime | `tokio` | multi-thread runtime |
| HTTP client | `reqwest` (rustls, gzip, brotli, cookies, socks) | no OpenSSL, keeps musl builds easy |
| Web server | `axum`, `tower-http` | compression, tracing, timeouts |
| UI templates | `askama` | compile-time templates |
| UI interactivity | htmx (vendored JS file) | no Node toolchain, no SPA |
| Asset embedding | `rust-embed` | CSS/JS/icons inside the binary |
| Database | `sqlx` (sqlite, runtime-tokio, macros) | migrations in `migrations/` |
| HTML parsing / CSS | `scraper` | |
| XPath | evaluate options in Phase 2 (`sxd-xpath` on cleaned HTML, or `skyscraper`); record choice in `docs/decisions.md` | must not need libxml2 |
| HTML to text | `html2text` | |
| JSONPath | `serde_json_path` | RFC 9535 |
| jq | `jaq-core`, `jaq-std`, `jaq-json` | pure Rust jq |
| Regex | `regex` | |
| Feeds | `feed-rs` | RSS/Atom/JSON Feed |
| Diff | `similar` | line + inline word diff |
| Compression | `zstd` | snapshot storage |
| Hashing | `blake3` | change detection by content hash |
| Templates for notifications | `minijinja` | |
| Email | `lettre` (rustls) | |
| Cron / schedule | `croner` | optional cron expressions in addition to intervals |
| Config | `figment` (TOML + env) | |
| CLI | `clap` (derive) | |
| Auth | `argon2`, `tower-sessions` (sqlite store) | |
| Errors | `thiserror` in libs, `anyhow` in bin | |
| Logging | `tracing`, `tracing-subscriber` | JSON logs optional |
| Metrics | `metrics`, `metrics-exporter-prometheus` | `/metrics` behind a flag |
| Testing | `wiremock`, `insta`, `tempfile`, `tokio::test` | |

Rust edition 2024, MSRV = latest stable at project start, pinned in `rust-toolchain.toml`.

---

## 3. Repository layout (cargo workspace)

```
vigil/
├── Cargo.toml                # workspace
├── rust-toolchain.toml
├── crates/
│   ├── vigil-core/           # domain types, pipeline (extract, filter, normalize), diff. No I/O.
│   ├── vigil-fetch/          # HTTP fetcher, rate limiting, retries, (later) browser fetcher
│   ├── vigil-store/          # sqlx repository, migrations, snapshot storage
│   ├── vigil-notify/         # notification URL parsing, channels, templating
│   ├── vigil-scheduler/      # job queue, per-host concurrency, backoff
│   ├── vigil-web/            # axum app: REST API + UI (askama + htmx)
│   └── vigil/                # binary: CLI, config, wiring
├── migrations/
├── assets/                   # css, htmx.min.js, icons (embedded)
├── docs/
│   ├── decisions.md
│   ├── api.md
│   ├── configuration.md
│   └── migrating-from-changedetection.md
├── deploy/
│   ├── Dockerfile
│   ├── docker-compose.yml
│   └── vigil.service         # systemd unit
└── .github/workflows/
```

Rule: `vigil-core` must stay pure (no network, no DB, no tokio) so the pipeline is easy to unit-test and fuzz.

---

## 4. Domain model

### Watch
- `id` (UUID v7), `name`, `url`, `tags: Vec<TagId>`, `paused: bool`
- `fetch`: `method` (GET/POST/...), `headers`, `body`, `timeout`, `user_agent`, `proxy`, `fetcher` (`http` | `browser`, browser = later)
- `schedule`: `Interval(Duration)` | `Cron(String)`, plus `jitter_secs`
- `processor`: `text` (default) | `json` | `feed`
- `pipeline: Vec<Step>` (ordered, see section 5)
- `conditions`: `trigger_text: Vec<Pattern>` (notify only if new text matches), `block_text: Vec<Pattern>` (suppress if matches), `notify_on: {added, removed, changed}`
- `notifications: Vec<NotificationTargetId>`, `notification_template_override`
- state: `last_checked_at`, `last_changed_at`, `last_error`, `consecutive_errors`, `last_hash`

### Snapshot
- `id`, `watch_id`, `taken_at`, `hash` (blake3 of processed text), `content` (zstd), `raw_size`, `http_status`, `content_type`
- Store only when hash differs from the previous snapshot. Keep a retention policy per watch (default: last 100 snapshots).

### NotificationTarget
- `id`, `name`, `url` (Apprise-style URL, section 7), `enabled`, `default_template`

### Tag
- `id`, `name`, `color`; tags can carry default notification targets and default interval.

### CheckLog (bounded)
- `watch_id`, `at`, `duration_ms`, `outcome` (`unchanged` | `changed` | `error`), `error_message`. Keep last N=500 per watch.

SQLite: WAL mode, `foreign_keys=ON`, one migration per change, never edit an applied migration.

---

## 5. Processing pipeline

```
fetch -> decode (charset) -> [processor] -> steps... -> normalize -> hash -> compare -> diff -> conditions -> notify
```

Processors:
- `text`: HTML is converted to readable text after selection steps; non-HTML is used as is.
- `json`: response parsed as JSON, pretty-printed with sorted keys before diffing.
- `feed`: parsed with `feed-rs`, each item rendered as one stable block (`title`, `link`, `published`), sorted newest first. Diff then shows new items naturally.

Steps (ordered, each a variant of `enum Step`):
- `Css { selector, mode: Include | Remove }`
- `XPath { expr, mode: Include | Remove }`
- `JsonPath { expr }`
- `Jq { program }`
- `RegexExtract { pattern, group }` (keep only matches)
- `RegexReplace { pattern, replacement }`
- `IgnoreLines { patterns }` (drop lines matching substring or `/regex/`)
- `Sort`, `Unique`, `Trim`, `CollapseWhitespace`, `Lowercase`

Patterns: plain substring (case-insensitive) or `/regex/flags`, same convention everywhere.

Comparison: compare blake3 of the final text. On change compute a line diff with `similar` and classify lines as added/removed. Evaluate `conditions` against the diff (trigger/block text checked against **added** lines by default).

Every step must have unit tests with `insta` snapshots on real-world-like fixtures in `crates/vigil-core/tests/fixtures/`.

Add a "preview" function: run a watch's pipeline on a fresh fetch (or last raw response) without saving, for the UI's live filter editor.

---

## 6. Scheduler and fetching

- One tokio task owns the schedule: a min-heap of `(next_run_at, watch_id)`. Workers pull due jobs from a bounded channel.
- Global concurrency limit (default 8) and **per-host** limit (default 2) via a `HashMap<host, Semaphore>`.
- Jitter on every run to avoid thundering herds. Exponential backoff on errors (cap at 6h), reset on success.
- "Check now" from UI/API bypasses the schedule but respects concurrency limits.
- HTTP: follow redirects (max 10), respect `timeout`, send `If-None-Match` / `If-Modified-Since` when the server supported them before; a `304` counts as unchanged without downloading.
- Response size cap (default 10 MB), configurable.
- Proxy per watch and a global default (HTTP and SOCKS5).
- Graceful shutdown: stop scheduling, wait for in-flight checks up to 30s.

---

## 7. Notifications

Apprise-style URLs so users migrating from changedetection.io can reuse what they have. Implement natively in Rust (no Apprise dependency):

| Scheme | Example |
|---|---|
| ntfy | `ntfy://host/topic`, `ntfys://host/topic`, optional `?priority=&tags=` and auth |
| Telegram | `tgram://bot_token/chat_id` |
| Discord | `discord://webhook_id/webhook_token` |
| Slack (webhook) | `slack://T000/B000/XXXX` |
| Email | `mailto://user:pass@smtp.host:587?to=a@b.c` and `mailtos://` |
| Generic webhook | `json://host/path`, `jsons://`, plus `form://` |
| Gotify | `gotify://host/token` |
| Matrix | `matrix://` (Phase 6, optional) |

- Parse URLs into `enum Channel` with strict validation and helpful errors. Unknown schemes must produce a clear "not supported yet" error, not a panic.
- Message templates via `minijinja`. Variables: `watch_name`, `watch_url`, `diff_added`, `diff_removed`, `diff_full`, `current_snapshot`, `triggered_text`, `checked_at`, `vigil_base_url`, `diff_url`.
- Per channel formatting: Markdown for Telegram/Discord/Slack, plain text for email (HTML email optional), JSON body for webhooks.
- Messages over a channel's size limit get truncated with a link to the diff page.
- "Send test notification" endpoint and UI button.
- Delivery failures are logged in CheckLog and retried 3 times with backoff; they never crash a worker.

---

## 8. Web UI and API

### REST API (`/api/v1`, token auth via `x-api-key` header)
Aim for compatibility with changedetection.io's public API where the semantics match, so existing scripts mostly keep working. Verify their current API docs before Phase 5 and record any intentional differences in `docs/api.md`.

- `GET/POST /watch`, `GET/PUT/DELETE /watch/{id}`, `POST /watch/{id}/check`
- `GET /watch/{id}/history`, `GET /watch/{id}/history/{snapshot_id}`, `GET /watch/{id}/diff?from=&to=`
- `GET/POST/DELETE /tags`, `GET/POST/DELETE /notifications`
- `POST /import` (list of URLs or changedetection.io export)
- `GET /healthz`, `GET /metrics` (optional)
- OpenAPI spec generated with `utoipa`, served at `/api/docs`.

### UI (server-rendered, askama + htmx, no build step)
Pages: watch list (status, last change, tags, quick pause/check), add/edit watch with **live preview** of the pipeline output, diff view (side by side + inline, toggle), history list, notification targets, tags, settings, login.
- Light/dark theme via `prefers-color-scheme`. Mobile-friendly.
- Total CSS/JS shipped < 100 KB.
- i18n-ready: all UI strings in a `locales/en.toml`; add `uk.toml` early (Ukrainian), contributors add the rest.

### Auth
- First run: generate an admin password if `VIGIL_ADMIN_PASSWORD` is unset and print it once to logs. Allow `--no-auth` for trusted LANs with a loud warning.
- API tokens created in settings, stored hashed.

---

## 9. Phases and acceptance criteria

### Phase 0: Skeleton
- Workspace, crates, CI (fmt, clippy, test on Linux/macOS), `rust-toolchain.toml`, licenses (MIT OR Apache-2.0), README stub, CONTRIBUTING.md, CODE_OF_CONDUCT.md.
- `vigil serve` starts axum on `:5800`, `/healthz` returns `ok`. Config via TOML + `VIGIL_*` env.
- SQLite with first migration (watches, snapshots, tags, notification_targets, check_log).
- **Done when:** `cargo run -- serve` works, CI green, `curl /healthz` returns 200.

### Phase 1: Core loop (CLI only)
- HTTP fetcher, `text` processor, snapshot storage with hash dedupe, line diff.
- Scheduler with interval schedules, concurrency limits, backoff.
- CLI: `vigil watch add <url> --every 5m`, `vigil watch list`, `vigil check <id>`, `vigil diff <id>`.
- **Done when:** integration test with `wiremock` serves page v1 then v2, and the store has exactly two snapshots and a correct diff.

### Phase 2: Pipeline
- All steps from section 5, `json` and `feed` processors, trigger/block conditions, preview function.
- Fixtures: news page, e-commerce product page, JSON API, RSS feed, Prozorro-like JSON listing.
- **Done when:** every step has snapshot tests; a fuzz target (`cargo fuzz`) runs the pipeline on arbitrary HTML for 60s without panics.

### Phase 3: Notifications
- All schemes from section 7 except Matrix, templating, retries, test endpoint.
- **Done when:** each channel has a test against a `wiremock` endpoint (SMTP via a local test server crate or mock transport), and an end-to-end test triggers ntfy on change.

### Phase 4: Web UI
- All pages from section 8, auth, live preview, diff view, embedded assets.
- **Done when:** a user can add a watch, set a CSS filter with live preview, receive a notification and view the diff entirely from the browser. Manual checklist in `docs/qa-checklist.md` passes.

### Phase 5: API and migration
- REST API + OpenAPI, API tokens.
- Importer for a changedetection.io backup/datastore (inspect their current backup format first, map watches, filters, tags and notification URLs; unsupported options are reported, not silently dropped). Document in `docs/migrating-from-changedetection.md`.
- Plain URL list import and OPML import.
- **Done when:** importing a sample changedetection.io backup with 20 varied watches produces equivalent watches and a report of anything skipped.

### Phase 6: Packaging and release
- Static musl builds for `x86_64`, `aarch64`, `armv7` (via `cargo-zigbuild` or `cross`).
- Docker image `FROM scratch` (with CA certs) or distroless, multi-arch, target < 20 MB. Volume `/data`.
- `docker-compose.yml`, systemd unit, release workflow (`cargo-dist` or GitHub Actions) with checksums and changelog (`git-cliff`).
- Benchmarks in README: RAM and CPU with 100 and 1000 watches on a Raspberry Pi 4 and a small VPS, compared fairly with changedetection.io in HTTP mode (same URLs, same intervals). Publish the method, not just numbers.
- **Done when:** `docker run -p 5800:5800 -v vigil:/data ghcr.io/<owner>/vigil` works on amd64 and arm64.

### Phase 7: Browser fetcher (post-1.0, optional feature flag `browser`)
- Fetch via Chrome DevTools Protocol (`chromiumoxide`) against an external browser (`VIGIL_BROWSER_WS=ws://...`, e.g. browserless or a sidecar chromium container). Vigil itself never bundles a browser.
- Wait-for-selector, wait time, custom JS before extraction.

---

## 10. CLAUDE.md content (create this file in Phase 0)

```markdown
# Vigil: rules for working in this repo

- Read PLAN.md before starting any phase. Follow the phase order.
- Before every commit: cargo fmt --all; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace.
- No `unwrap()`/`expect()` outside tests and `main` startup code. Use `thiserror` in library crates, `anyhow` in the binary.
- vigil-core is pure: no tokio, no network, no database.
- Every new pipeline step, notification channel or API endpoint ships with tests.
- Prefer small, focused modules (< 400 lines). Public items get doc comments.
- No new dependency without a one-line reason in docs/decisions.md. Prefer pure-Rust crates; no OpenSSL, no libxml2.
- Never edit an applied SQL migration; add a new one.
- Clean-room: do not copy code, templates or text from changedetection.io.
- Keep UI strings in locales/*.toml, never hardcode them in templates.
- Conventional commits. One logical change per commit.
```

---

## 11. Roadmap after 1.0
1. Browser fetcher (Phase 7).
2. Price and restock detection processor (structured data: JSON-LD `Product`/`Offer`, microdata, common selectors).
3. Screenshot capture and visual diff (browser fetcher required).
4. Browser steps (login, click, type) as a declarative list.
5. Multi-user with roles.
6. RSS output feed of detected changes per tag.
7. Plugins via WASM (`wasmtime`) for custom processors.

---

## 12. Open-source hygiene
- License: dual MIT OR Apache-2.0.
- Issue templates (bug, feature, new notification channel), PR template with checklist.
- `good first issue` labels: new notification channels and new pipeline steps are ideal starter tasks, so keep those interfaces small and documented in `docs/extending.md`.
- Security policy (`SECURITY.md`): SSRF is the main risk. Add an option to block private IP ranges for fetches (`VIGIL_BLOCK_PRIVATE_NETWORKS`, default off for self-hosters, documented clearly) and never fetch `file://`.
- Semantic versioning; DB migrations must be forward-only and automatic on startup with a backup copy of the SQLite file before migrating.
