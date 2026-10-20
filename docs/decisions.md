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

`Store::open` runs the embedded migrations. When an existing database (one that has applied at least one migration) has pending ones, `VACUUM INTO <db>.backup-v<latest applied>-<unix ms>` writes a consistent copy first (a plain file copy of a WAL database can miss the `-wal` file). A fresh or up-to-date database is not backed up, a failed backup aborts the open, and old backups are never deleted. A backup holds everything the database holds, including notification target secrets.

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
| `libfuzzer-sys` (fuzz crate only) | The fuzz target for the pipeline (PLAN.md Phase 2). Lives in `crates/vigil-core/fuzz`, outside the workspace, so nothing in the product depends on it. |
| `serde_json` (binary) | Read `--pipeline` and `--conditions` JSON and print `watch show`. Already in the tree through the core and store crates. |
| `regex` | Patterns for ignore/trigger/block conditions and the regex steps (PLAN.md section 2). Linear-time matching, so hostile patterns cannot stall a check. |
| `feed-rs` | Parse RSS, Atom and JSON Feed for the `feed` processor (PLAN.md section 2). Pure Rust (quick-xml); brings `chrono`, `url` and `mediatype`. |
| `scraper` | Parse HTML and select elements with CSS selectors for the `css` step (PLAN.md section 2). Pure Rust. Latest release uses html5ever 0.39 while `html2text` uses 0.40, so both versions are built; changing `html2text` to match would change every watch's text, so the duplicate stays until the two catch up. |
| `ego-tree` | Names the node ids of `scraper`'s tree (`scraper` does not re-export the type). Same version as `scraper` uses, so it adds no code. |
| `sxd-xpath`, `sxd-document` | XPath 1.0 evaluation for the `xpath` step (PLAN.md section 2). Pure Rust, no libxml2. Evaluates on a read-only copy of the page; see "XPath step". |
| `serde_json_path` | JSONPath (RFC 9535) for the `jsonpath` step (PLAN.md section 2), including filters and the standard functions. Pure Rust; pulls `nom` and `inventory` (function registration). |
| `jaq-core`, `jaq-std`, `jaq-json` | Pure-Rust jq for the `jq` step (PLAN.md section 2). `jaq-std` brings `jiff`, `base64`, `regex-bites` and a few small crates for dates, formats and regexes. |
| `serde_json` (core) | Parse JSON bodies for the `json` processor and the JSON steps, and (de)serialize pipeline steps in the store. Already a dependency of the store. |

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

## JSON processor

The `json` processor parses the body (any content type) and prints it with two-space indentation, object keys sorted at every level, arrays in their original order and one trailing newline, so key order and formatting never register as changes. Keys are sorted by our own printer rather than by the map type, because `serde_json` keeps insertion order as soon as any crate in the build enables `preserve_order`. Numbers go through `serde_json`: integers that fit 64 bits stay exact, larger ones become floating point and lose precision, and `1.0` stays `1.0`. Nesting is limited to 128 levels (the parser's own limit, applied to values built by later steps too) and any step's output to 16 MiB, so a small input cannot make a check use unbounded memory. A body that is not JSON fails the check with the parser's message.

## Feed processor

Each entry becomes up to three lines (title, link, date) and entries are separated by a blank line. The date is `published`, or `updated` when there is none, converted to UTC and printed as `2026-10-12T09:00:00Z`. Entries are sorted newest first; entries without a date come after the dated ones in the order the feed lists them (the sort is stable). Lines the feed leaves out are left out, and an entry with no title, link or date is skipped. Entry ids and summaries are not part of the text: feed software generates ids when a feed has none, and summaries are often rewritten without the item changing. Feed-level data (channel title, build date) is not part of the text either, so a feed that touches `lastBuildDate` on every request does not look changed. The link is the `alternate` one when an entry has several. Titles collapse inner whitespace so line wrapping in the source does not matter. The body has already been decoded by `decode`, so every `encoding` in the feed's XML declarations is rewritten to UTF-8 before parsing; otherwise the parser would decode the text a second time and garble it. All declarations are rewritten, not only the first, because fuzzing found that a second one naming another encoding makes `quick-xml` panic; a panic that still gets through is caught and reported as an unreadable feed.

## Pipeline and text steps

`Pipeline::render(bytes, content_type)` is the single path from a fetched body to the stored text: decode, processor, `normalize`, then each step, normalising after every step so each one sees tidy lines and the result always ends in exactly one newline. Checks and previews both call it, so a preview cannot differ from what a check would store. Steps are stored as plain JSON (`{"type": "regex_extract", ...}`, internally tagged, unknown fields rejected, variants only ever added) and compiled by `Pipeline::new`; a bad regex, pattern or capture group is reported there with the step's number and name, so the CLI can refuse it when a watch is saved and a check reports it without fetching anything useless. Text steps work on lines. `regex_extract` keeps each match (or one capture group) on its own line, skips empty matches, and leaves empty text when nothing matches (see "Empty selections"); `regex_replace` with no match is a no-op, and a replacement that names a capture group the pattern does not have is refused when the step is built: the `regex` crate reads `$1USD` as the group called `1USD`, which does not exist, and silently replaces it with nothing, so the text after the group would vanish (`${1}USD` is what was meant, and the error says so). `sort` is byte order, `unique` keeps the first occurrence anywhere in the text, `trim` strips both ends of each line, `collapse_whitespace` turns every run of whitespace into one space (so an indented line keeps one leading space), and `lowercase` is full Unicode lowercasing. The raw `regex_*` steps take plain regex syntax with `(?i)`-style inline flags; only `ignore_lines`, triggers and block text use the shared pattern syntax. A step's output is capped at 16 MiB, checked while it is produced.

## CSS step

`css` runs before the page becomes text, so it is only valid with the `text` processor and must come before every other kind of step; both rules are checked when the pipeline is built. The page is parsed as HTML whatever its `Content-Type` says, since asking for a selector is asking for HTML. `include` keeps only the selected elements, in document order; an element inside another selected element is skipped because its text is already part of the outer one. Each kept element goes in its own block, so inline matches such as prices end up on separate lines. Table rows and sections are put back into a table so the HTML parser does not throw their tags away, which means html2text draws them with borders; table cells are reduced to their content so `td.price` gives just the price. `include` that selects nothing leaves an empty page. `remove` detaches the selected elements and does nothing when there are none; later steps select by walking the tree from its root, because the library's own `select` walks every node ever allocated and would match elements inside the detached subtrees, bringing removed content back; removing `html` or `body` leaves an empty page. The kept or edited page is serialised once and converted with the same html2text settings as the plain `text` processor.

## XPath step

PLAN.md left the XPath crate open, so three designs were compared on what the step needs: pure Rust, tolerance for broken HTML, both `include` and `remove`, no extra HTML parser, and survival under fuzzing. `skyscraper` has its own tree and parser and its author warns the API will break before 1.0; selecting with it would mean a second HTML tree that cannot be edited or serialised for html2text. `sxd_html` (html5ever into `sxd-document`) was last released in August 2024, would add a third html5ever, and `remove` would need an XML-only serialiser whose self-closing tags change how HTML is parsed. The choice is `sxd-xpath` over a copy: the page stays a `scraper` tree, each `xpath` step builds a read-only `sxd-document` copy of its elements and text, evaluates the expression there, and maps the returned nodes back to the page's node ids (the node types hash by identity). CSS and XPath steps therefore edit one tree and share the same include and remove code, and html5ever parses everything. Costs: `sxd-xpath` has seen no release since 2020 and only speaks XPath 1.0, and the copy is rebuilt per XPath step. Expressions are evaluated without namespaces, so `xlink:href`-style names do not match. The copy refuses pages nested deeper than 256 elements because the evaluator recurses; real pages are far shallower. An expression that is slow on a large page is not interrupted here; the scheduler's per-check timeout bounds it.

`include` takes elements and text nodes as written, plus attribute values (`//a/@href`) and computed values (`count(//li)`, `string(//h1)`), which become text; a result that is empty text counts as nothing, like an empty selection. `remove` can delete elements and text nodes only and rejects expressions that yield attribute values or computed values. `/` selects the document element. Computed values are written as XPath writes them (`Infinity`, `-Infinity`, `NaN`, `true`, and a single `0`), not as Rust prints floats. Error messages from the library, which are bare variant names such as `EmptyPredicate`, are turned into words.

## JSON steps

JSON steps (`jsonpath`, and later `jq`) work on the parsed value of the `json` processor, come after any HTML steps and before text steps, and are rejected with other processors when the pipeline is built. A JSONPath step always produces an array of its matches, even for a single match, so the shape of the stored text does not flip between an object and a list as results come and go; finding nothing gives `[]`, like jq. The array is printed by the same canonical printer as the `json` processor. Matches can overlap (`$..*` returns every value inside every other one), so their approximate total size is checked against the 16 MiB output cap before anything is copied.

## jq step

A `jq` step runs a jaq program on the `json` processor's value and collects its outputs into an array, so one output and many outputs have the same shape; no output gives `[]`, because a jq program says what is empty (`select`, `empty`) and appending `| if length == 0 then error("nothing") else . end` makes it an error. Programs are user input, so the engine is fenced in. Programs over 2 KiB or nested deeper than 32 brackets are refused before parsing, since the parser recurses and a stack overflow cannot be caught (see "Input limits"). Filters that read the machine, the clock or other streams are removed, so a program gives the same answer for the same input everywhere: `env` (and so `$ENV`), `now`, `input`, `inputs`, `debug`, `stderr`, `input_filename`, `localtime` and `strflocaltime`. `halt` and `halt_error` are removed as well, and the library's `unwrap_valr` is not used, because it calls `std::process::exit` on `halt`. The definitions in the standard library that are built on the removed filters are dropped with them; a definition that uses a missing filter would fail to compile and take every program with it. Date functions that do not depend on the clock or the local time zone (`todate`, `fromdateiso8601`, `strftime`, `mktime`, ...) stay. At most 10,000 outputs and 16 MiB are accepted. Values cross between `serde_json` and jaq directly (jaq's own `serde` support in, a small converter out) rather than through text. Going out, what JSON cannot hold is mapped as jq prints it: NaN becomes `null`, infinity the largest finite number, and a byte string text with invalid bytes replaced; an object key that is not text is an error. jaq shares sub-values, so a few nodes can describe an enormous tree; the converter charges its size budget as it copies, so such a value stops at 16 MiB instead of exhausting memory. Compilation errors are reported when the pipeline is built, with the byte offset of a syntax error or the name and arity of an undefined filter; the program is compiled again for every run, because the compiled form borrows from its arena and the compile cost is small next to a fetch. Known gaps: a program that never produces output (`repeat(.)`, `def f: f; f`) is not interrupted by the engine, and one that recurses without bound may exhaust memory or the stack; the scheduler's per-check timeout covers the first, and `vigil preview` is the way to try a program before saving it.

## Empty selections

A selection step (`css`, `xpath`, `jsonpath`, `regex_extract`) that finds nothing gives an empty result: an empty page, `[]`, or empty text. The check succeeds and the empty text is stored like any other. This was first built as a failed check so that a site redesign would show up as an error instead of looking like every line was deleted, but failed checks back off (the wait doubles per consecutive failure, up to 6 hours) and a watch on something that is empty most of the time (new listings, a sold-out badge, items on sale) would fail, slow down, and then store its first real content as a baseline, so the moment the item appeared would go unreported. Each selection step takes `"required": true` to get the stricter behaviour back: when it finds nothing the check fails with `step N (type): matched nothing`. It applies to `remove` mode too. Without it, a redesign that empties the selection shows up as a change to empty text, which the trigger and block conditions can see. For `xpath`, an empty computed string (`string(//missing)`) counts as nothing; for `css`, a selector that matches an element with no text still counts as a match. The flag is omitted from the stored JSON when false, so existing configs keep their shape. `jq` has no flag: its program decides what is empty.

## Conditions

`conditions` only decide whether a change is announced, never whether it is stored: every change becomes a snapshot and a `check_log` row (`changed`) whatever the verdict, so history is complete and loosening a condition later does not lose anything. They produce a `Verdict { notify, reason, triggered_text }` from the line diff, and the scheduler hands it to the notification step. The shape of a change is `added` (lines were only added), `removed` (only removed) or `changed` (both), and `notify_on` switches each off; all three default to on, and `{}` notifies about everything. Evaluation order: a switched-off shape does not notify; an added line matching any `block_text` pattern does not notify; with `trigger_text` set, at least one added line must match one of its patterns, and the matching lines become `triggered_text`. Patterns only look at added lines, so a change that only removes lines never satisfies a trigger; a `scope` option to look at removed lines too can be added later as a defaulted field. Patterns use the shared syntax (substring or `/regex/flags`) and are compiled when the watch is saved, with the list name and position in the error (`trigger_text[1]: ...`). Unknown fields are rejected so a misspelt option cannot silently do nothing.

## Editing a watch

`Store::update_watch` replaces a watch's settings and keeps its pause flag, hash, snapshots and log. It also clears the failure streak and last error, so an edit that fixes a failing watch does not wait out its backoff. It clears the remembered `ETag` and `Last-Modified`: a server that answers 304 to the conditional request would never send the body again, and an edited pipeline would never run. A check that was already running when the edit happened would write its own validators back, from a response processed with the old settings; every check therefore carries the `updated_at` of the watch it was made with (`CheckSuccess::settings_version`, which `update_watch` always increases), and `record_success` drops the validators of a result whose version no longer matches. The cost is that the first check after an edit compares the new output with the old snapshot and may report a change; re-baselining on edit is deliberately not built, and `vigil preview` is how to see the new output before saving. `pipeline` and `conditions` are stored as JSON in the existing columns, with no migration. A row whose JSON does not parse (a step type from a newer version, say) is skipped by `list_watches`, as unreadable rows already are, and `get_watch` reports the error. A step that parses but does not compile (a bad regex or selector) does not hide the watch: the watch is still scheduled and each check fails with the step's number and name.

## Processing runs on its own thread

A check builds the watch's pipeline and renders the response on a dedicated `std::thread` and waits for it with a 30 second timeout (`Checker::with_render_timeout` changes it for tests). Processing is CPU work on untrusted input, so it stays off the async workers, and it does not use `spawn_blocking`: the runtime waits for blocking tasks when it shuts down, so a jq program that never ends would hang `vigil serve`. When the timeout passes, or the thread panics, the check fails with `processing timed out after 30s` or `processing panicked` and releases its host and global slots; the runtime does not wait for a detached thread. The cost is that a runaway thread keeps its CPU until the process exits, since nothing can interrupt it. So that retries do not add a runaway thread each, a watch whose earlier processing thread is still alive is refused a new one (`an earlier processing of this watch is still running`), which bounds a stuck watch at one thread; a cap of 32 live threads bounds many stuck watches, and the slot is released before the result is sent so a finished thread never refuses its own watch's next request. The thread has a 16 MiB stack (address space, not memory in use) because parsing and printing nested documents recurse. The pipeline is built on that thread, so nothing in it has to be `Send`. Conditions are compiled and the pipeline is built (on that thread) before the fetch, so a watch with an invalid pattern, selector, regex or program fails without making a request; a watch with a stuck thread does not fetch either. The conditions' verdict is carried on `CheckResult::Changed`; the check log still says `changed`, since the change was stored.

## Preview

`Checker::preview(&Watch)` fetches the page and renders it exactly as a check would (same `Pipeline::render`, same thread and timeout), then compares the text with the watch's newest snapshot and runs the conditions on that difference, without recording anything: no snapshot, no `check_log` row, no change to the watch's state or error count. It takes a `&Watch` rather than an id so that unsaved edits and command-line overrides can be previewed; only the id is used, to find the snapshot. The request carries no `If-None-Match` or `If-Modified-Since`, since a 304 has no body to show. A watch whose own headers make the request conditional can still get a 304, which is reported as a preview error. Problems with the page or the configuration come back in the report and never count against the watch. Previews fetch a fresh response every time; raw responses are not stored (PLAN.md allows "last raw response"), which keeps the database free of up to 10 MB per watch. The Phase 4 editor can hold the body it fetched while the user edits.

## Configuring watches from the command line

`watch add` and `watch edit` take `--processor`, `--pipeline` and `--conditions`; `--pipeline` and `--conditions` take JSON inline or `@path` to a file holding it, in the same shape that is stored, so what `watch show` prints can be pasted back. Each option replaces that part of the watch as a whole; there is no way to add one step, because the order of steps matters and a list edited in place is easy to get wrong. The merged result is validated before anything is saved (steps against the processor, selectors, regexes, patterns), so a typo is refused with the step's number and nothing is written. Changing only `--processor` on a watch whose steps do not fit it is refused with a hint to replace the steps too. `watch edit` with no option is an error rather than a silent no-op. `watch show` prints the settings as JSON (not the check state), which the REST API in Phase 5 will mirror.

## `vigil preview`

`vigil preview <id>` prints, in order: a status line (HTTP status, content type, size, time), the processed text, a unified diff against the newest snapshot (coloured on a terminal unless `NO_COLOR` is set) or a note that there is none or no difference, and whether the conditions would notify and why. `--processor`, `--pipeline` and `--conditions` override the saved settings for that run only, with the same validation as `watch add`, so a pipeline can be tried before `watch edit` saves it. It exits non-zero when the page or the settings do not work, so it can gate a script. `vigil check` now also says when a stored change would not notify (`no notification: no added line matches trigger_text`), since the change is stored either way and the output would otherwise read as if it were announced.

## Deeply nested pages

Parsing and rendering HTML take more than linear time in the nesting depth: the parser searches its stack of open elements for most tags, and html2text does more on top. Measured in a debug build, 40,000 nested `<div>`s took 37 seconds and 80,000 took over 100; a megabyte of nesting would keep a core busy for minutes, and the check's timeout can only abandon the thread, not stop it. A page nested more than 512 elements deep (not counting elements the parser closes by itself, such as unclosed `<li>` and `<p>`) therefore fails the check with `the page nests elements deeper than 512 levels` before it is parsed. The guard cannot use the parsed tree, since parsing is the slow part: it reads the markup in linear time the way the tokenizer does (comments, declarations, raw-text elements such as `<script>`, quoted attribute values), keeps a stack of open element names, ignores end tags that match nothing open as the parser does, and stops as soon as the stack passes the limit. Inside `<svg>` and `<math>` it follows the foreign-content rules, where a trailing `/>` closes the element (so thousands of `<path/>` siblings are not nesting), unless the tag is an HTML element that breaks out of foreign content or sits inside a place where HTML is parsed again (`foreignObject`, `title`, ...); outside them `<div/>` stays open, as in HTML. It follows every way the tokenizer ends a comment (`-->`, `--!>`, `<!-->`, `<!--->`). It applies to the plain `text` processor and to pipelines with HTML steps. Real pages are far shallower (rarely past 100); the XPath step's copy has its own, lower limit of 256 because the evaluator recurses. The guard is lexical, so a page crafted to fool it (an attribute value the tokenizer reads differently, say) falls back to the thread timeout.

## Fuzzing

`crates/vigil-core/fuzz` holds a `cargo fuzz` target, `pipeline_html`, that runs arbitrary bytes through ten fixed pipelines (the empty one, CSS and XPath include and remove, a long chain of text steps, JSONPath on the `json` processor, and the `feed` processor) with the content types that make sense for each. Errors are expected; panics, hangs and stack overflows are the findings. jq programs are not generated, because a valid program can run forever and the fuzzer could only call that a timeout. The crate is its own workspace so that it does not inherit `unsafe_code = "forbid"` (the fuzz macro generates `#[no_mangle]` items) and so that stable builds never see it; it needs a nightly toolchain and `cargo install cargo-fuzz`. Run it from `crates/vigil-core`: `mkdir -p fuzz/corpus/pipeline_html && cargo +nightly fuzz run pipeline_html fuzz/corpus/pipeline_html tests/fixtures -- -max_total_time=60 -max_len=16384 -rss_limit_mb=2048`. The first directory receives new inputs and is ignored by git; the fixtures are seeds only. `libfuzzer-sys` turns every panic into a crash, including ones the pipeline would catch in production, so a caught panic still counts as a finding. The first runs found a panic inside `quick-xml` (a second XML declaration in a feed) and the super-linear cost of deeply nested HTML; both are fixed and recorded above.

## Input limits

The CSS selector parser, the XPath parser and evaluator, and the jq parser recurse, and a stack overflow aborts the whole process, so it cannot be caught the way a panic can. A release build overflowed a 2 MiB stack at about 1,000 chained jq operators (`-----...`, `.|.|.|...`) or about 500 nested `:not(`; the command line checks steps on the 8 MiB main thread and checks run on a 16 MiB thread. Bracket nesting alone is not enough, since chains of operators recurse too, so every selector, expression and program is also limited in length: 1 KiB for CSS and XPath, 2 KiB for jq, and brackets nested at most 32 deep in all three. Real ones are a few dozen bytes. Errors quote only the start of an oversized input. This bounds parsing; it does not bound what a jq program does when it runs. A program that recurses without limit (`def f: 1 + f; f`) or builds a very deeply nested value overflows the stack at run time and aborts the process, and the compile-time limits cannot see it. Running jq in a child process that can be killed is the fix and is left as a follow-up.

## Rescheduling after an edit

The scheduler reloads the watch list every 30 seconds. A watch it already knows keeps its place in the schedule, unless what the store says about its timing has changed since the last reload: its interval, its jitter or its failure streak. Then its next check is recomputed from its last check time, as if it had just been seen, and the old queue entry is skipped. Otherwise `vigil watch edit --every 5m` on a daily watch would wait out the day, and an edit that fixes a failing watch (which resets the streak) would wait out the backoff, up to 6 hours. The comparison is with what was last loaded, not with the scheduler's own copy, so a check whose result could not be recorded (which the scheduler counts as a failure locally) is not mistaken for an edit. An edit that changes neither leaves the schedule alone: its pipeline applies to the next check at the usual time.
