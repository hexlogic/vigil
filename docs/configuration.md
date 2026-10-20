# Configuration

Vigil reads configuration from, lowest to highest precedence:

1. Built-in defaults.
2. A TOML file: `--config <path>` or `VIGIL_CONFIG`, otherwise `./vigil.toml` if it exists.
3. `VIGIL_*` environment variables (nested keys are separated by `__`).

See [`vigil.example.toml`](../vigil.example.toml) for a complete example.

| Key | Env var | Default | Description |
|---|---|---|---|
| `listen` | `VIGIL_LISTEN` | `0.0.0.0:5800` | Address the HTTP server binds to. |
| `data_dir` | `VIGIL_DATA_DIR` | `data` | Directory for the SQLite database (`vigil.db`). Created if missing. |
| `log_format` | `VIGIL_LOG_FORMAT` | `text` | `text` or `json`. |
| `fetch.timeout_secs` | `VIGIL_FETCH__TIMEOUT_SECS` | `30` | Request timeout for watches that do not set their own. |
| `fetch.max_body_bytes` | `VIGIL_FETCH__MAX_BODY_BYTES` | `10485760` | Largest accepted response body, measured after decompression. Larger responses fail the check. |
| `fetch.user_agent` | `VIGIL_FETCH__USER_AGENT` | `vigil/<version>` | `User-Agent` header sent with every request. |
| `scheduler.concurrency` | `VIGIL_SCHEDULER__CONCURRENCY` | `8` | Checks running at once. |
| `scheduler.per_host` | `VIGIL_SCHEDULER__PER_HOST` | `2` | Checks running at once against the same host and port. |

Log verbosity is controlled by `RUST_LOG`. The default is `info` (stdout) for `vigil serve` and `warn` (stderr) for the one-shot commands, so their stdout carries only results.

## Commands

| Command | What it does |
|---|---|
| `vigil serve` | Start the HTTP server and the scheduler. `GET /healthz` returns `200 ok` when the database is reachable. Ctrl-C or SIGTERM stops starting checks and waits up to 30 seconds for running ones. |
| `vigil watch add <url> [--every 1h] [--name <name>]` | Watch a page. The interval takes values like `30s`, `5m`, `1h 30m`, `1d` and must be between one second and 365 days. |
| `vigil watch list` | Show all watches with their id, interval, last check, last change and status (`pending`, `ok`, `paused` or `error xN`). |
| `vigil watch remove <id>` | Delete a watch with its snapshots and history. |
| `vigil check <id>` | Check a watch now. Prints `baseline stored`, `unchanged` or `changed, +N -M lines`; exits non-zero if the page could not be checked. |
| `vigil diff <id>` | Show the difference between the two newest snapshots as a unified diff, in color on a terminal (`NO_COLOR` turns it off). |

An `<id>` is the full id from `vigil watch list` or its last six or more characters. The commands read and write the same database as `vigil serve` and can run while it is running.

## How a check works

The page is fetched (redirects followed up to 10 hops, responses over `fetch.max_body_bytes` rejected), decoded, converted to text (HTML becomes readable text, other content is used as is) and normalised. A snapshot is stored only when the text differs from the previous one. When the server sent an `ETag` or `Last-Modified`, the next request is conditional and a `304` counts as unchanged. A failed check is recorded on the watch and the next attempt waits `interval x 2^failures`, at most 6 hours. Each run is delayed by a random jitter of up to a tenth of the interval (at most 60 seconds). Only `http` and `https` URLs are fetched.
