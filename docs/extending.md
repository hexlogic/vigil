# Extending Vigil

Notification channels and pipeline steps are the two things meant to be added by contributors. Both are small, self-contained changes.

## Adding a notification channel

A channel is a URL scheme that parses into a struct, a way to build the message in the channel's markup and a way to send it. Everything lives in `crates/vigil-notify`.

1. **Parse it.** Add `src/channel/<name>.rs` with a public struct for the channel (`Secret` for tokens and passwords, `SecretUrl` for a URL that may hold one, so a derived `Debug` is safe), a `parse` function and a `redacted` method. Use the helpers in `channel/parse.rs`: `split_url` for `scheme://host[:port]/path?query` schemes, `Query` for options (unknown and repeated options are errors), `decode`, `is_loopback`. Errors are `ParseError::Invalid` built with `invalid(scheme, "what is wrong")`; they must not repeat the input. `ntfy.rs` is the model for a URL scheme and `telegram.rs` for one that needs a hand-written splitter.
2. **Register it** in `channel/mod.rs`: a `Channel` variant, the scheme in `SCHEMES`, and the arms in `Channel::parse`, `redacted`, `kind`, `markup` (which markup its messages use), `limit` (the service's size limit, in characters or bytes) and `sends_credentials_in_cleartext`. The compiler points at every `match` that needs an arm.
3. **Send it.** For an HTTP service add a function to `src/request.rs` that returns an `HttpRequest` (build JSON with `serde_json::json!`, never by filling in text) and an arm in `http_request`. For anything else add a module like `smtp.rs` and call it from `Sender::send`. A refusal that retrying cannot fix is `SendError::permanent`; a failure that may pass is `transient`.
4. **Markup.** If the service understands formatting the existing markups do not (`Plain`, Telegram HTML, Discord Markdown, Slack `mrkdwn`), add a `Markup` variant in `src/escape.rs` (`escape`, `code_block`, `link`, `diff_link`) and a built-in template in `src/builtin.rs`. Escaping is what keeps page text from breaking or abusing the message, so test it.
5. **Test it.** Add the URL forms to `tests/parse.rs`, the refusals to `tests/parse_refused.rs`, and a URL carrying recognisable secrets to `SECRET_URLS` there (it checks that no `Debug`, `redacted` or error output leaks them). Add a test against a `wiremock` server to `tests/http_channels.rs`, using `Endpoints` if the service has a fixed address.
6. **Document it** with a row in `docs/notifications.md`, and in `docs/decisions.md` for any choice a reader would wonder about.

## Adding a pipeline step

Steps are the variants of `Step` in `crates/vigil-core/src/pipeline/step.rs`, which is also their JSON form (`{"type": "..."}`): variants are only ever added, never renamed. Each step has a module under `pipeline/` (`text.rs`, `html.rs`, `json.rs`, ...), a place in the validation in `pipeline/mod.rs`, a section in [pipeline.md](pipeline.md) and snapshot tests with a fixture in `crates/vigil-core/tests/`. `vigil-core` stays free of I/O.
