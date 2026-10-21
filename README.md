# Vigil

A lightweight, self-hosted website change detection service written in Rust.
One static binary, SQLite, low memory.

> **Status:** early development (Phase 4: web UI). Everything works from the command line and from a web UI: extraction steps (CSS, XPath, JSONPath, jq, regex), JSON and feed processors, notification conditions, notifications to ntfy, Telegram, Discord, Slack, email, webhooks and Gotify, history and diffs. The REST API, importers and packaging come next.

## Quick start

```sh
cargo run -- watch add https://example.com --every 5m
cargo run -- watch list
cargo run -- check <id>     # <id>: full id or its last 6+ characters
cargo run -- diff <id>      # after the page has changed once

# watch just one part of a page, and try it before saving
cargo run -- watch add https://example.com --pipeline '[{"type": "css", "selector": "h1"}]'
cargo run -- preview <id>

# get notified
cargo run -- notify add phone ntfys://ntfy.sh/my-secret-topic
cargo run -- notify test phone
cargo run -- watch edit <id> --notify phone

cargo run -- serve          # check on schedule, serve the web UI on :5800
```

`vigil serve` makes up an admin password the first time it runs and prints it once in
its log; set `VIGIL_ADMIN_PASSWORD` to choose your own. Then open
<http://localhost:5800>: add watches with a live preview of the filters, see the
history and diffs, manage notification targets and tags. The UI is in English and
Ukrainian. See [docs/web-ui.md](docs/web-ui.md).

Configuration is read from `vigil.toml` (optional) and `VIGIL_*` environment
variables; see [docs/configuration.md](docs/configuration.md). Steps and conditions
are described in [docs/pipeline.md](docs/pipeline.md), notification channels and
message templates in [docs/notifications.md](docs/notifications.md), the web UI in
[docs/web-ui.md](docs/web-ui.md).

## Layout

See [PLAN.md](PLAN.md) for the roadmap and [docs/decisions.md](docs/decisions.md)
for design decisions.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
