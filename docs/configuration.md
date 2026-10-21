# Configuration

Vigil reads configuration from, lowest to highest precedence:

1. Built-in defaults.
2. A TOML file: `--config <path>` or `VIGIL_CONFIG`, otherwise `./vigil.toml` if it exists.
3. `VIGIL_*` environment variables (nested keys are separated by `__`).

See [`vigil.example.toml`](../vigil.example.toml) for a complete example. The web UI is described in [web-ui.md](web-ui.md).

| Key | Env var | Default | Description |
|---|---|---|---|
| `listen` | `VIGIL_LISTEN` | `0.0.0.0:5800` | Address the HTTP server binds to. |
| `data_dir` | `VIGIL_DATA_DIR` | `data` | Directory for the SQLite database (`vigil.db`). Created if missing. |
| `base_url` | `VIGIL_BASE_URL` | unset | The address Vigil is reached at, such as `https://vigil.example.com` (an `http` or `https` URL, which may include a path but no query, fragment or user name). Notifications use it to link to the diff page of a change; without it they have no link. |
| `admin_password` | `VIGIL_ADMIN_PASSWORD` | unset | The password for the web UI. Set, it is the password at every start, whatever was saved before (a password changed in Settings is replaced). Unset, the first start makes one up and shows it once in the log; later starts keep it. Only a hash is stored. Prefer the environment variable to the file. |
| `no_auth` | `VIGIL_NO_AUTH` | `false` | Skip the sign-in: anyone who can reach the server can change everything. Also `vigil serve --no-auth`. For a trusted network only; see [Signing in](#signing-in-to-the-web-ui). |
| `log_format` | `VIGIL_LOG_FORMAT` | `text` | `text` or `json`. |
| `fetch.timeout_secs` | `VIGIL_FETCH__TIMEOUT_SECS` | `30` | Request timeout for watches that do not set their own. |
| `fetch.max_body_bytes` | `VIGIL_FETCH__MAX_BODY_BYTES` | `10485760` | Largest accepted response body, measured after decompression. Larger responses fail the check. |
| `fetch.user_agent` | `VIGIL_FETCH__USER_AGENT` | `vigil/<version>` | `User-Agent` header sent with every request. |
| `scheduler.concurrency` | `VIGIL_SCHEDULER__CONCURRENCY` | `8` | Checks running at once. |
| `scheduler.per_host` | `VIGIL_SCHEDULER__PER_HOST` | `2` | Checks running at once against the same host and port. |

Log verbosity is controlled by `RUST_LOG`. The default is `info` (stdout) for `vigil serve` and `warn` (stderr) for the one-shot commands, so their stdout carries only results.

## Signing in to the web UI

The web UI asks for a password. Sessions last 30 days after they were last used, and are kept in the database under a hash of the browser's cookie, so a copy of the database cannot be used to sign in. After five wrong passwords within a minute, signing in is refused for the rest of that minute, whoever asks. The password is hashed with Argon2id.

If `VIGIL_ADMIN_PASSWORD` is not set, the first `vigil serve` makes up a password, prints it once in the log (`WARN ... password=...`) and keeps only its hash. If you miss it, stop the server and start it with `VIGIL_ADMIN_PASSWORD` set to choose a new one.

With `no_auth`, pages are open to everyone who can reach the server, and every page says so. As a guard against a web page elsewhere reaching your server by a name it controls (DNS rebinding), Vigil then answers only requests addressed to an IP address, to `localhost`, or to the host in `base_url`; set `base_url` if you reach the server by a name such as `nas.lan`. `/healthz` is exempt. Do not turn sign-in off on a server that is reachable from the internet.

Behind a reverse proxy, set `base_url` to the public address. A path in it (`https://example.com/vigil`) serves the whole UI under that path, so the proxy must pass the path on and not strip it. With an `https` `base_url` the session cookie is marked `Secure`. A form post is accepted only when the browser says it comes from the page it posts to, or from `base_url`.

## Commands

| Command | What it does |
|---|---|
| `vigil serve [--no-auth]` | Start the HTTP server, the web UI and the scheduler. `GET /healthz` returns `200 ok` when the database is reachable. Ctrl-C or SIGTERM stops starting checks and waits up to 30 seconds for running ones. |
| `vigil watch add <url> [--every 1h] [--name <name>] [content options] [--notify <target>]... [--notify-template <TEXT\|@FILE>]` | Watch a page. The interval takes values like `30s`, `5m`, `1h 30m`, `1d` and must be between one second and 365 days. See [Content options](#content-options). |
| `vigil watch edit <id> [--every <interval>] [--name <name>] [content options] [--notify <target>]... [--no-notify <target>]... [--notify-template <TEXT\|@FILE>]` | Change a watch. Only the options given change. The remembered `ETag` and the failure streak are cleared, so the next check fetches the page afresh and may report the edit itself as a change. A new interval, or a cleared failure streak, moves the next check within about 30 seconds (when `vigil serve` reloads its watches); other edits apply at the next scheduled check. |
| `vigil watch show <id>` | Print a watch's settings as JSON, in the shape `--pipeline` and `--conditions` take, with the targets it notifies and its notification template. |
| `vigil preview <id> [content options]` | Fetch a watch and show what a check would store: the text, a diff against the newest snapshot and whether it would notify. Saves nothing. With content options, tries them in place of the watch's own. Exits non-zero if the page or the settings do not work. |
| `vigil watch list` | Show all watches with their id, interval, last check, last change and status (`pending`, `ok`, `paused` or `error xN`). |
| `vigil watch remove <id>` | Delete a watch with its snapshots and history. |
| `vigil notify add <name> <url> [--template <TEXT\|@FILE>]` | Add a notification target: a name for a notification URL such as `ntfy://ntfy.sh/my-topic` or `tgram://BOT_TOKEN/CHAT_ID`. The URL and the template are checked first; errors never repeat the URL, which usually holds credentials. Prints a warning when the URL would send a secret (a password, a token, an ntfy topic or a webhook path) over plain HTTP to another machine. |
| `vigil notify list` | Show the targets with their credentials replaced by `***`, whether they are enabled and how many watches use them. |
| `vigil notify remove <target>` | Delete a target. A `<target>` is its name, its id or the last six or more characters of it. |
| `vigil notify test <target>` | Send a test notification and say whether it arrived. Not retried; exits non-zero if it failed. |
| `vigil check <id> [--no-notify]` | Check a watch now. A change the watch's conditions want announced is sent to its targets, and the command waits for that (retries included) before it exits; `--no-notify` skips it. A delivery that fails is reported on stderr and does not change the exit code. Prints `baseline stored`, `unchanged` or `changed, +N -M lines` (with `no notification: <reason>` when the watch's conditions hold the change back); exits non-zero if the page could not be checked. |
| `vigil diff <id>` | Show the difference between the two newest snapshots as a unified diff, in color on a terminal (`NO_COLOR` turns it off). |

`--notify` links a watch to a target (by name or id), `--no-notify` unlinks it, and `--notify-template` sets the message template for the watch (empty text removes it). Linking and templates are not settings of the page check: unlike the other `watch edit` options they do not clear the remembered `ETag` or the failure streak, and need no wait for the scheduler.

An `<id>` is the full id from `vigil watch list` or its last six or more characters. The commands read and write the same database as `vigil serve` and can run while it is running.

### Content options

`watch add`, `watch edit` and `preview` take these. Each replaces that part of the watch as a whole, and the result is checked before anything is saved: a bad selector or regex is refused with the step's number.

| Option | Meaning |
|---|---|
| `--processor text\|json\|feed` | How the response becomes text. `text` (the default) turns HTML into readable text and uses other content as is; `json` prints JSON with sorted keys; `feed` lists the entries of an RSS, Atom or JSON feed. |
| `--pipeline <JSON\|@FILE>` | Steps applied after the processor: a JSON array, or `@path` to a file holding one. See [pipeline.md](pipeline.md). |
| `--conditions <JSON\|@FILE>` | Which changes notify: a JSON object, or `@path`. See [pipeline.md](pipeline.md#conditions). |

```sh
vigil watch add https://shop.example/widget --every 15m \
  --pipeline '[{"type": "css", "selector": "#price"}]' \
  --conditions '{"trigger_text": ["in stock"]}'
vigil preview <id>        # what would a check store, and would it notify?
```

## How a check works

The page is fetched (redirects followed up to 10 hops, responses over `fetch.max_body_bytes` rejected), decoded (the charset comes from a byte-order mark, the `Content-Type` header, or a declaration in the first kilobyte of an HTML or XML page, in that order), turned into text by the watch's processor, run through its pipeline steps, and normalised (line endings, trailing spaces and blank runs). Processing a response (building the steps and running them) runs on its own thread, and a check whose processing takes more than 30 seconds fails. A snapshot is stored only when the text differs from the previous one; the watch's conditions then decide whether the change would notify, but the change is stored either way. When the server sent an `ETag` or `Last-Modified`, the next request is conditional and a `304` counts as unchanged. A failed check is recorded on the watch and the next attempt waits `interval x 2^failures`, at most 6 hours. Each run is delayed by a random jitter of up to a tenth of the interval (at most 60 seconds). Only `http` and `https` URLs are fetched.
