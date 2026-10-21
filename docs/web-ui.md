# The web UI

`vigil serve` serves a web UI on the address it listens on (`:5800` by default). It needs no build step and no external service: the pages, the stylesheet and the one script it uses ([htmx](https://htmx.org), vendored) are in the binary.

## Signing in

The first run makes up an admin password and prints it once in the log; `VIGIL_ADMIN_PASSWORD` sets your own. See [Signing in to the web UI](configuration.md#signing-in-to-the-web-ui) for how sessions work, the limit on wrong passwords, `--no-auth` and what it changes.

## Pages

| Page | What it does |
|---|---|
| **Watches** (`/`) | All watches by name, with status, tags, last check and last change. Filter by tag. Per row: *Check now* (obeys the concurrency limits), *Pause*/*Resume*, *Edit*, *Delete* (asks first). |
| **Add / edit a watch** | Address, name, interval, how to read the response, a list of filters (CSS, XPath, JSONPath, jq, regex, ignore lines, sort, trim ...) added, removed and moved in place, which changes notify, targets, tags, a message template, request settings. Beside the form, a **live preview** shows what a check would store with the settings as typed, and how it differs from the last stored version. |
| **A watch** (`/watch/<id>`) | Settings, state, the last checks and the last notifications sent for it. |
| **History** | The stored versions, newest first; each can be read or compared with the one before. |
| **Changes** (`/watch/<id>/diff?from=<version>&to=<version>`) | The difference between two versions, inline or side by side, with the changed words marked. This is the address notifications link to. |
| **Notifications** | Targets: add, edit, turn on or off, delete, and *Send test*. Addresses are shown with their secrets replaced by `***`, and are never put back in a form. |
| **Tags** | Name, colour, a default interval for watches added with the tag, and default targets that are notified about all its watches. |
| **Settings** | Language, password, *Sign out everywhere*, version, and whether `base_url` is set. |

## Behind a reverse proxy

Set `base_url` to the address people use. If it has a path (`https://example.com/vigil`), the whole UI is served under that path, and the proxy must pass the path on and not strip it. With an `https` `base_url`, cookies are marked `Secure`. A form post is accepted only when the browser says it comes from the site, which is checked against `base_url` and the `Host` header.

```nginx
location /vigil/ {
    proxy_pass http://127.0.0.1:5800;   # no trailing path: /vigil/ is passed on as it is
    proxy_set_header Host $host;
}
```

```caddy
example.com {
    reverse_proxy /vigil/* 127.0.0.1:5800
}
```

`GET /healthz` stays at the root of the server, for container and load-balancer checks.

## Security headers

Every page carries a strict policy: scripts and styles only from the server itself, no inline script or style, no plugins, no framing, forms only to the same site. Pages are not cached by browsers; the stylesheet and script are cached for a year under a URL that carries the version. Cross-site requests that would change something are refused whether or not sign-in is on.

## Languages

The UI is available in English and Ukrainian. The language is the one picked in Settings (kept in a cookie), else the best match in the browser's `Accept-Language`, else English. Adding a language is described in [extending.md](extending.md#adding-a-language-to-the-web-ui).
