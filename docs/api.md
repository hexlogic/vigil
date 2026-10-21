# REST API

Everything the web UI does, a program can do through the REST API under `/api/v1` (under the path of `base_url`, if it has one). The description of the API is generated from the code and served at `/api/docs/openapi.json`; `/api/docs` points at it. It needs no token, so tools such as `openapi-generator` or Postman can read it directly. There is no Swagger UI: any viewer can read the JSON.

## Authentication

Make a token on the **Settings** page, or with `vigil token add <name>`, and send it in the `x-api-key` header:

```sh
TOKEN=$(vigil token add backup-script)       # shown once; only its hash is kept
curl -H "x-api-key: $TOKEN" http://localhost:5800/api/v1/version
```

A cookie never authenticates the API, so a page on another site cannot make a signed-in browser call it. Tokens do not expire and have no scopes: a token can do what the admin can. Revoke one on the Settings page or with `vigil token remove <name>`.

With sign-in turned off (`--no-auth`) no token is needed, and the API answers only requests addressed to an IP address, `localhost` or the `base_url` host, and not ones that a browser says come from another site. See [Signing in](configuration.md#signing-in-to-the-web-ui).

## Conventions

- Bodies are JSON (`Content-Type: application/json`), except `POST /import`, which takes the file itself.
- Times are RFC 3339 in UTC. Ids are UUIDs, written in full.
- `PUT` replaces the whole thing: a field that is left out gets its default.
- Unknown fields are refused, so a misspelt option cannot silently do nothing.
- Lists are returned in an object (`{"watches": [...]}`), sorted by name without regard to case.
- A request body is limited to 1 MiB, except the import, which is limited by `import.max_upload_bytes`.

### Errors

Every error, including the ones the web framework makes (a wrong method, a timeout), has this shape:

```json
{"error": {"code": "invalid", "message": "the interval must be between 1 second and 365 days", "field": "schedule"}}
```

| Status | `code` | Meaning |
|---|---|---|
| 400 | `bad_request` | The request cannot be read (broken JSON, a query value of the wrong kind). |
| 401 | `unauthorized` | No valid token. |
| 403 | `forbidden` | Refused because of where the request comes from (sign-in off). |
| 404 | `not_found`, `no_such_endpoint`, `nothing_to_compare` | No such thing; or no such path. |
| 405 | `method_not_allowed` | The path exists, the method does not (`Allow` lists the ones that do). |
| 409 | `conflict` | A name is taken. |
| 413 | `too_large` | The body or upload is over the limit. |
| 415 | `unsupported_media_type` | Not JSON. |
| 422 | `invalid`, `unknown_format`, `unreadable_file` | The request is well formed but not acceptable; `field` says which part. |
| 503 | `unavailable` | The server is shutting down. |

A 500 says only that something went wrong; the details are in the server log.

## Endpoints

| Method and path | What it does |
|---|---|
| `GET /version` | The version, and whether a token is needed (`access`: `token` or `open`). A client can use it to check its token. |
| `GET /watch?tag=<id>` | List the watches, optionally those with a tag. |
| `POST /watch` | Add a watch. `201` with a `Location`; its first check starts at once. |
| `GET /watch/{id}` | One watch. |
| `PUT /watch/{id}` | Replace a watch. The remembered `ETag` and the failure streak are cleared only if the fetch or processing settings changed. |
| `DELETE /watch/{id}` | Delete a watch and its history (`204`). |
| `POST /watch/{id}/check?wait=true` | Check now, even if paused. `202` and returns at once; with `wait=true`, waits up to a minute and answers `200` with the outcome and the watch. |
| `GET /watch/{id}/history?limit=` | The stored versions, newest first (100 by default, at most 500), without their text. |
| `GET /watch/{id}/history/{snapshot}` | One version with its text. |
| `GET /watch/{id}/diff?from=&to=` | How two versions differ: added and removed lines, their counts and a unified diff. Defaults to the newest two. |
| `GET/POST /tags`, `GET/PUT/DELETE /tags/{id}` | Tags. |
| `GET/POST /notifications`, `GET/PUT/DELETE /notifications/{id}` | Notification targets. Addresses are written but never read back: answers carry them with secrets replaced by `***`. |
| `POST /notifications/{id}/test` | Send one test message and say whether it arrived. |
| `POST /import?format=&dry_run=&allow_duplicates=&every=` | Import watches from a file. See [migrating.md](migrating.md). |

### A watch

`GET /watch/{id}` and what `POST` and `PUT` accept use the same shape as `vigil watch show`:

```json
{
  "id": "0198a5c2-7a3e-7c01-9f1d-2b6f3c8d9e10",
  "name": "Widget price",
  "url": "https://shop.example/widget",
  "paused": false,
  "schedule": {"interval": 900},
  "jitter_secs": 0,
  "fetch": {"timeout_secs": null, "user_agent": null, "headers": {}},
  "processor": "text",
  "pipeline": [{"type": "css", "selector": "#price", "mode": "include"}, {"type": "trim"}],
  "conditions": {"trigger_text": [], "block_text": [], "notify_on": {"added": true, "removed": true, "changed": true}},
  "snapshot_retention": 100,
  "tags": [],
  "targets": [],
  "notification_template": null,
  "created_at": "2026-10-21T08:00:00Z",
  "updated_at": "2026-10-21T08:00:00Z",
  "state": {"last_checked_at": null, "last_changed_at": null, "last_error": null, "consecutive_errors": 0}
}
```

Only `url` is required to make a watch. `name` defaults to the address and `schedule` to the shortest default interval of the watch's tags, else an hour. `fetch` may also hold `method` (`GET`, `POST`, `PUT`, `PATCH` or `DELETE`) and `body`; `conditions` may hold `"scope": "text"`. The steps of `pipeline` and the conditions are described in [pipeline.md](pipeline.md). `state` is read-only.

```sh
curl -H "x-api-key: $TOKEN" -H 'Content-Type: application/json' \
  -d '{"url": "https://shop.example/widget", "schedule": {"interval": 900},
       "pipeline": [{"type": "css", "selector": "#price"}]}' \
  http://localhost:5800/api/v1/watch
```

A watch is checked as it is built: a selector that does not compile, a regex that does not build, a template that fails or steps in the wrong order is a `422` naming the `field` (`url`, `pipeline`, `conditions`, `fetch.body`, ...) and, for a step, which one.

### Importing

```sh
curl -H "x-api-key: $TOKEN" --data-binary @backup.zip \
  'http://localhost:5800/api/v1/import?dry_run=true'
```

The answer is `{"dry_run", "report", "made"}`. Run it with `dry_run=true` first to read the report; without it, the watches are made and `made` lists their ids.
