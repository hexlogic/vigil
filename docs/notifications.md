# Notifications

Vigil sends a message when a watch changes and the watch's [conditions](pipeline.md#conditions) say the change is worth announcing. A **target** is a name for a notification URL; a watch can notify any number of targets.

```sh
vigil notify add phone ntfys://ntfy.sh/my-secret-topic
vigil notify test phone                      # send a test message
vigil watch edit <id> --notify phone         # use it for this watch
vigil check <id>                             # changed pages notify here too (--no-notify to skip)
vigil notify list                            # credentials are shown as ***
```

## Notification URLs

Add an `s` for HTTPS where the scheme has a plain and a secure form. Plain HTTP is fine on a trusted network, but a topic, a token, a password or a webhook's path is sent in the clear: Vigil warns when you add such a URL for another machine, and `ntfys://ntfy.sh/...` is the right form for a public server.

| Scheme | URL | Notes |
|---|---|---|
| ntfy | `ntfy://[user:password@]host[:port][/prefix]/topic` / `ntfys://` | Options: `priority` (1-5, or min, low, default, high, max), `tags` (comma separated), `token` (a bearer token, instead of `user:password@`). The topic is treated as a secret. The message links to the diff page. |
| Telegram | `tgram://BOT_TOKEN/CHAT_ID` | The chat id is a number (negative for groups and channels) or `@channelname`. Sent as Telegram HTML. |
| Discord | `discord://WEBHOOK_ID/WEBHOOK_TOKEN` | Option: `username`. Messages cannot ping anyone. |
| Slack | `slack://T000/B000/XXXX` | The three parts of an incoming-webhook path. |
| Email | `mailto://[user:password@]host[:port]?to=a@b.c` / `mailtos://` | See [Email](#email). |
| Webhook | `json://[user:password@]host[:port]/path[?query]` / `jsons://`, `form://` / `forms://` | POSTs the whole event as JSON or as form fields. See [Webhooks](#webhooks). |
| Gotify | `gotify://host[:port][/prefix]/APP_TOKEN` / `gotifys://` | Option: `priority` (0-10). |

An option that is unknown or given twice is an error, and so is anything else wrong with the URL; the message says what, and never repeats the URL. Characters that are special in a URL (`@`, `:`, `/`) in a user name or password are written as `%40`, `%3A`, `%2F`; in an option value (after `?`) a space is `%20`, and a `+` is just a plus. `matrix://` and other schemes are not supported yet.

### Email

- `mailto://` connects in plain text and **requires** an upgrade with STARTTLS. Default port 587.
- `mailtos://` uses TLS from the first byte. Default port 465.
- `mailto://...?tls=none` sends in plain text, for a relay on this machine or network (default port 25). With a password it is only accepted for `localhost` and loopback addresses.
- `to` takes several addresses, separated by commas or repeated. `from` is the sender; it defaults to the user name if that is an email address, and `to` defaults to the sender. `name` sets the sender's display name. Addresses may be written `Name <a@b.c>`.
- The message is plain text, with the title as the subject.

```sh
vigil notify add mail 'mailto://me%40example.com:app-password@smtp.example.com?to=me@example.com'
vigil notify add relay 'mailto://mail.lan?tls=none&from=vigil@lan.example&to=me@lan.example'
```

### Webhooks

`json://` sends `Content-Type: application/json`:

```json
{
  "version": 1,
  "title": "Example changed",
  "message": "the rendered message",
  "watch": {"id": "...", "name": "Example", "url": "https://example.com"},
  "checked_at": "2026-01-01T00:00:00Z",
  "diff_url": "https://vigil.example.com/watch/.../diff?from=...&to=...",
  "added": ["lines that were added"],
  "removed": ["lines that were removed"],
  "triggered_text": ["added lines that matched the trigger patterns"],
  "truncated": false
}
```

`form://` sends the same fields as `application/x-www-form-urlencoded`, with `added`, `removed` and `triggered_text` joined by newlines and the watch fields flattened (`watch_id`, `watch_name`, `watch_url`). The lists are cut at 256 KiB each, which sets `truncated`. `user:password@` becomes HTTP Basic authentication. The query string is sent as written.

## Messages

Each channel has a built-in message: a title ("*name* changed"), the page URL, the text that matched a trigger pattern, the diff and a link to the diff page. Write your own with a template (`vigil notify add ... --template`, or `vigil watch edit --notify-template` for one watch). A watch's template wins over its target's, which wins over the built-in one.

Templates use the [minijinja](https://docs.rs/minijinja) syntax (`{{ variable }}`, `{% if %}`, `{% for %}`), with its built-in filters (`join`, `length`, `lines`, `first`, `replace`, `trim`, `default` and so on) except `safe`, which is not available because it would switch the escaping off. Variables:

| Variable | Value |
|---|---|
| `watch_id`, `watch_name`, `watch_url` | The watch. |
| `diff_added`, `diff_removed` | The added and removed lines, one per line. |
| `diff_full` | The change as a unified diff with 3 lines of context. |
| `current_snapshot` | The page text after the change. |
| `triggered_text` | The added lines that matched a trigger pattern, as a list (`{{ triggered_text \| join(", ") }}`). |
| `checked_at` | When the check ran, e.g. `2026-01-01T00:00:00Z`. |
| `vigil_base_url`, `diff_url` | The `base_url` setting and the link to the diff page; empty when `base_url` is not set. |
| `truncated` | `true` when the message had to be shortened. |
| `is_test` | `true` for a test notification. |

Two filters add markup that suits the channel and is not escaped again: `{{ diff_full \| code }}` puts text in a preformatted block, and `{{ watch_url \| link }}` writes a link. `{% set title = "..." %}` sets the title, which email (subject), ntfy, Gotify and webhooks show separately; it defaults to "*name* changed".

```
{% set title = watch_name ~ ": " ~ (triggered_text | length) ~ " match(es)" %}
{{ watch_url }}
{% if triggered_text %}Matched: {{ triggered_text | join(", ") }}{% endif %}
```

A template is checked when it is saved: an unknown variable or filter, or a loop that runs out of its 100,000 units of work, is refused. Because a page can print anything, a template that renders in the check but fails on a real change falls back to the built-in message, and the delivery log says so.

### Escaping

Whatever a template prints is escaped for the channel; the template's own text is not. A page that contains `<script>`, `_` or `<!channel>` therefore cannot break a Telegram message or ping a Slack or Discord channel. Telegram messages are HTML (`<b>`, `<a href>`, `<pre>`), Discord messages Markdown (`**bold**`), Slack messages `mrkdwn` (`*bold*`); in a custom template write the markup of the channel it is for. A target that receives different channels' markup needs a template per target.

### Length

Messages are shortened to what each service accepts: Telegram 4096 characters, Discord 2000 characters, ntfy 4096 bytes, Slack 4000 characters, Gotify 32,000 characters, email and webhooks 256 KiB. The long values a template prints (the diff, the lines, the snapshot) are cut on a line or character boundary, sharing the room, and "… truncated." with a link to the diff page ends the message.

## Delivery

A change is delivered to each target on its own, after the check has been recorded, so a slow or failing service never delays a check and never makes it fail. A failed delivery is tried again after 2, 10 and 30 seconds (a connection problem, a timeout, a server error, `408`, `429`, honouring `Retry-After` up to a minute); a refusal such as a wrong token fails at once. Redirects are not followed and proxy variables are ignored. Every delivery is written to the delivery log (`sent`, or `failed` with the reason) with the number of attempts. `vigil serve` gives deliveries 30 seconds to finish when it stops.

A notification is saved before it is delivered and removed once it is over, whether it was sent, refused for good, or ran out of retries. One that a shutdown or a crash interrupts stays saved, and `vigil serve` sends it when it starts again, with the same message and link (unless its target has been removed or disabled meanwhile, or it is more than a day old, which is recorded as failed). `vigil check` does not send other runs' leftovers; `vigil serve` does.

## Security

- Notification URLs are stored as typed, in the database and in the backup copies made before migrations. Protect the data directory like any file that holds passwords.
- `vigil notify list`, error messages and logs show only the redacted form. A service that echoes the request path in an error page is scrubbed too.
- The URL is a command-line argument, so it is visible in the shell history and in the process list while the command runs.
