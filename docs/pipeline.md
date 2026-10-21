# Pipelines and conditions

A watch turns each response into text, and a change is a difference between two such texts. The **processor** makes the first text; **pipeline steps** narrow it to what you care about; **conditions** decide whether a change is worth a notification. Try any of it with `vigil preview <id> --pipeline ...` before saving it.

## Processors

| Processor | What it does |
|---|---|
| `text` | HTML becomes readable text (scripts and styles dropped, links shown as `[text]`); anything else is used as is. |
| `json` | The body is parsed and printed with two-space indentation and object keys sorted at every level, so key order and formatting never count as changes. Numbers beyond 64 bits lose precision. |
| `feed` | RSS, Atom or JSON Feed. Each entry becomes a block of title, link and date (UTC, `2026-10-12T09:00:00Z`), newest first; entries without a date come last in feed order. Feed-level data such as a build date is left out, so it cannot cause changes. |

## Steps

A pipeline is a JSON array of steps, run in order. Each step is an object with a `type`. Unknown types and unknown fields are rejected. Steps must be listed in this order: **HTML steps** (`css`, `xpath`; `text` processor only), then **JSON steps** (`jsonpath`, `jq`; `json` processor only), then **text steps**.

| Step | Fields | What it does |
|---|---|---|
| `css` | `selector`, `mode` (`include` default, or `remove`), `required` | Keep only the elements a CSS selector matches, or delete them. The page is parsed as HTML whatever its content type. |
| `xpath` | `expr`, `mode`, `required` | The same with an XPath 1.0 expression. `include` can also select text nodes (`//ul/li/text()`), attribute values (`//a/@href`) and computed values (`count(//li)`, `string(//h1)`); `remove` deletes elements and text nodes. |
| `jsonpath` | `expr`, `required` | A JSONPath query ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535), filters and functions included). The result is always an array of the matches. |
| `jq` | `program`, `raw` | A jq program ([jaq](https://github.com/01mf02/jaq)). The outputs are collected into an array; no output gives `[]`. With `"raw": true` they are printed as lines of text instead, as `jq -r` does (a string without its quotes, anything else as compact JSON); the result is then text, so a raw step must be the last of the JSON steps and text steps may follow it. `env`, `now`, `input`, `halt`, `debug` and other filters that read the machine or the clock are not available, so the same input always gives the same output. At most 10,000 outputs. |
| `regex_extract` | `pattern`, `group` (default 0), `required` | Keep only what the regex matches, one match per line, or one capture group of each. |
| `regex_replace` | `pattern`, `replacement` | Replace every match. `$1` and `${name}` refer to capture groups, `$$` is a dollar sign. A name runs as far as letters, digits and underscores go, so text right after a group needs braces (`${1}USD`, not `$1USD`); a group the pattern does not have is refused. |
| `ignore_lines` | `patterns` | Drop lines that match any pattern. |
| `sort`, `unique`, `trim`, `collapse_whitespace`, `lowercase` | none | Sort the lines (byte order); drop repeated lines, keeping the first; strip both ends of every line; turn every run of whitespace into one space; lowercase everything. |

Regexes use the [`regex` crate's syntax](https://docs.rs/regex/latest/regex/#syntax); put flags inline, such as `(?i)`. They are matched in linear time and limited in size.

### When a selection finds nothing

`css`, `xpath`, `jsonpath` and `regex_extract` give an empty result when they find nothing (an empty page, `[]`, empty text); the check succeeds and the empty text is stored. That suits a watch on something that is empty most of the time, such as new listings or a sold-out badge: when an item appears, the change is reported. Set `"required": true` on a step to make the check fail instead, with `step N (type): matched nothing`, which is how to notice that a redesign broke a selector. Failed checks are retried less and less often (the wait doubles per failure, up to 6 hours), so use `required` only where that is what you want.

### Examples

```json
[{"type": "css", "selector": "#price"}]
```
```json
[{"type": "css", "selector": "nav, footer, .ad", "mode": "remove"},
 {"type": "regex_replace", "pattern": "\\d+ minutes? ago", "replacement": "recently"}]
```
With `--processor json`:
```json
[{"type": "jsonpath", "expr": "$.items[?@.price < 20].name"}]
```
```json
[{"type": "jq", "program": ".items[] | select(.stock > 0) | {id, name}"}]
```

## Patterns

`ignore_lines`, `trigger_text` and `block_text` take patterns: a plain substring, matched without regard to case, or `/regex/flags` with the flags `i`, `m`, `s`, `x` and `U`. Anything that does not have that form is plain text, so `//` and `/foo/g` are plain. A pattern written `/api/v1/` is the regex `api/v1`, which also matches a path containing that text; add a flag or escape the slashes to be exact. Empty patterns are rejected.

## Conditions

Conditions decide whether a change would notify. They never decide whether it is stored: every change becomes a snapshot.

```json
{"trigger_text": ["in stock"], "block_text": ["/sold out/i"], "notify_on": {"added": true, "removed": false, "changed": true}}
```

| Field | Meaning |
|---|---|
| `trigger_text` | Notify only if a line in `scope` matches one of these patterns. Empty means no restriction. |
| `block_text` | Never notify if a line in `scope` matches one of these. Wins over `trigger_text`. |
| `notify_on` | `added` (lines were only added), `removed` (only removed) and `changed` (both) each switch on or off; all default to on. |
| `scope` | `added` (the default): the two lists look at added lines only. `text`: they look at every line of the new text, changed or not. |

`{}` notifies about every change. With the default scope, patterns look at added lines only, so a change that only removes lines never satisfies a trigger. `vigil preview` and `vigil check` say whether a change would notify and why.

## Limits

Output of any step is capped at 16 MiB. HTML nested more than 512 elements deep (not counting elements the parser closes itself, such as unclosed `<li>`) fails the check, because parsing it takes too long; the `xpath` step's limit is 256. CSS selectors and XPath expressions may be at most 1 KiB, jq programs at most 2 KiB, and brackets in all of them nested at most 32 deep, because the parsers recurse. A jq program that recurses without limit can still crash the server; test new programs with `vigil preview` first. A check that takes more than 30 seconds fails.
