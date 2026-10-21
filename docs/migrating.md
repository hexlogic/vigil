# Migrating to Vigil

Vigil can read watches from a list of addresses, from OPML, from a backup of another self-hosted change monitor, and from urlwatch and Distill exports. It tells you what it could not carry over; nothing is dropped silently.

```sh
vigil import backup.zip --dry-run     # read it, print the report, change nothing
vigil import backup.zip               # make the watches
```

The same is available in the web UI under **Import** (a preview, then a button) and as `POST /api/v1/import` ([api.md](api.md#importing)). The format is guessed from the content; `--format url-list|opml|backup|urlwatch|distill` says it.

- **History is not imported.** A watch made by an import has no versions, so its first check is its baseline and does not notify.
- A watch whose address is already watched is skipped, so running an import twice does not double everything. `--allow-duplicates` makes it anyway. Two watches in one file with the same address are both made: they may differ.
- Formats that have no interval of their own (lists, OPML, urlwatch, Distill) get `--every` (default `1h`).
- Imported watches start checking within 30 seconds if `vigil serve` is running.
- Files may be up to `import.max_upload_bytes` (256 MiB) for a backup and 16 MiB for the other formats, and hold at most 20,000 watches.

## The report

For every watch of the file, the report says whether it is made (`will be made`), made with something left out or changed (`will be made, with notes`), or `skipped`. Each note is one of:

- **skipped**: the option has no equivalent, and was left out.
- **approximated**: the option was carried over so that it behaves a little differently.
- **info**: a fact that changes nothing.

Notes never repeat a notification address, a header value, a request body, or the query string of an address: those often hold credentials. Options that only record how the last check went are ignored without a word, an option with its default value is not reported, and an option this importer has never heard of is named once for the whole import.

## A list of addresses

One address per line; a `#` starts a comment. After the address, separated by a space, may come tags separated by commas:

```text
https://example.com/pricing
https://example.com/blog  news, company
```

## OPML

An outline of feeds, as feed readers export. An `<outline>` with an `xmlUrl` becomes a watch that reads the response as a feed; one with only a `url` or `htmlUrl` becomes a page watch. The title of the nearest folder around it is its tag.

## A backup of another self-hosted change monitor

A ZIP with a folder per watch and per tag (`<id>/watch.json`, `<id>/tag.json`) next to a settings file, or its older single JSON file with every watch in a `watching` object. The files are recognised by their shape. Only the JSON files are read; snapshots, screenshots and history files are skipped, and nothing is extracted to disk. New backups still carry a stale copy of the older file; it is ignored when the per-watch files are there, and the report says so.

| In the source | In Vigil |
|---|---|
| address, title | address, name (the page title or the address if there is no title) |
| tags | tags, by name; a tag's notification addresses become its default targets; its filters are added to its watches, as in the source |
| `include_filters`: CSS | one `css` step; several selectors are joined with `, `, so matches come in page order (reported) |
| `include_filters`: `xpath:`, `xpath1:`, a leading `/` | one `xpath` step; joined with ` | `. A watch with both CSS and XPath keeps CSS (reported) |
| `include_filters`: `json:`, `jq:`, `jqraw:` | `jsonpath` and `jq` steps (`jqraw` is `jq` with raw output); the watch reads the response as JSON |
| `subtractive_selectors` | `css` / `xpath` steps that remove |
| `ignore_text`, and the global list | `ignore_lines` |
| `trigger_text`, `text_should_not_be_present` | `trigger_text`, `block_text`, looking at the whole text (`scope: text`), as the source does |
| `extract_text`, `extract_lines_containing` | `regex_extract` (group 1 when the pattern has a group; several patterns only when none has a group) |
| `trim_text_whitespace`, `remove_duplicate_lines`, `sort_text_alphabetically` | `trim`, `unique`, `sort` |
| lines added / removed / replaced | `notify_on`: added only becomes "added, and changed"; reported as approximated unless all three are on |
| check interval, or the default | interval; the global random delay becomes the jitter |
| `method`, `body`, `headers` | the same; the global headers are added, and a watch's own win |
| a notification message | rewritten for Vigil's templates (variables renamed, `diff_added_clean(...)` becomes `diff_added`, a title becomes a `{% set title %}` block) and checked; one that still does not work is left out with the reason |
| notification addresses | notification targets, reused when the address is the same; a watch with none of its own gets the global ones; an unsupported kind is reported by its scheme only |
| muted notifications | Vigil has no mute for one watch, so a muted watch is linked to no target and is left off every tag that notifies (a tag of the backup with addresses, or an existing tag with targets), and the report names each tag left off. The tag's filters stay in the watch. When all notifications are muted, tags are made without targets |
| the number of versions kept | the same |

Patterns: a `/regex/` with no flags is case-insensitive in the source, so `i` is added; flags `i m s x` are kept, `g` is dropped, others are reported. A pattern that does not build here (a look-behind, say) is dropped alone and reported; so is a filter or XPath that does not.

Not carried over, and reported when set: a proxy, check windows by day and time, rules on the page's contents, reporting only unseen lines, a different address to open, AI summaries, screenshots in notifications, running scripts in the page, steps in a browser (the watch is made paused), and ignoring error statuses. Price and stock tracking is imported as a text watch and paused; watching a page as an image is skipped; a watch fetched through a browser is fetched over HTTP. The source's default `User-Agent`, timeout, jitter, headers and similar settings are copied into each watch, and the report says so once.

## urlwatch

A `urls.yaml` (one or several YAML documents). Jobs with a `url` become watches; jobs with `navigate` or `command` are skipped. `filter` may be a list or the older comma string.

| In urlwatch | In Vigil |
|---|---|
| `css`, `xpath` (with `exclude`), `element-by-id` / `-class` / `-tag` | selection steps |
| `html2text`, `beautify` | nothing (text is the default) |
| `strip`, `striplines`, `sort`, `remove-duplicate-lines` | `trim`, `sort`, `unique` |
| `grep` / `grepi` | keep / ignore the lines that match |
| `re.sub`, `re.findall` | `regex_replace` (`\1` becomes `${1}`), `regex_extract` |
| `jq` | `jq` on a page read as JSON (the result is a JSON list here, where urlwatch printed one value per line) |
| `format-json` | the page is read as JSON |
| `method`, `data`, `json`, `headers`, `cookies`, `timeout` | the same (`cookies` become a `Cookie` header; a mapping `data` is sent as a form) |
| `tags`, `enabled: false` | tags; a paused watch |
| any other filter, proxies, `ssl_no_verify`, `max_tries` and the `ignore_*` options, and so on | reported, and left out |

urlwatch keeps its notification settings outside `urls.yaml`, so no notification targets are imported.

## Distill

A JSON export. The address, name, tags and CSS or XPath selections of each monitor are read. **This format was read from a description, not from an export,** so the schedule is not read and anything else is reported rather than guessed at. Run `--dry-run` first, and tell us what an export shows that is not handled.
