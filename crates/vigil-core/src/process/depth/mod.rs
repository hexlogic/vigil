//! A linear-time guard against pages nested too deeply to process.
//!
//! HTML parsing and rendering both take more than linear time in the nesting depth (the
//! parser looks through its stack of open elements for every tag), so the depth has to be
//! checked before either runs. This reads the markup the way the tokenizer does, keeps a
//! stack of open elements, and stops as soon as the stack passes the limit.

/// Elements that never have content or an end tag.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Elements the parser closes on its own (a new `<li>` ends the open one), so unclosed ones
/// do not nest, and that are therefore not counted. `html`, `head` and `body` are single.
const IMPLIED_END: &[&str] = &[
    "html", "head", "body", "p", "li", "dt", "dd", "tr", "td", "th", "thead", "tbody", "tfoot",
    "caption", "colgroup", "option", "optgroup", "rb", "rt", "rtc", "rp",
];

/// HTML elements that end foreign (SVG or MathML) content when they appear inside it, so they
/// are read by the HTML rules even inside `<svg>`.
const BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strong",
    "strike",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
    "font",
];

/// Elements inside `svg` and `math` in which HTML is parsed again.
const INTEGRATION_POINTS: &[&str] = &[
    "foreignobject",
    "desc",
    "title",
    "annotation-xml",
    "mi",
    "mo",
    "mn",
    "ms",
    "mtext",
];

/// Elements whose content is text up to their end tag.
const RAW_TEXT: &[&str] = &[
    "script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes",
];

/// True when `html` nests more than `limit` elements, not counting the ones the parser
/// closes by itself. Elements are matched by name; an end tag that matches nothing open is
/// ignored, as the parser ignores it, so stray end tags cannot hide the depth.
pub(super) fn nests_deeper_than(html: &str, limit: usize) -> bool {
    let bytes = html.as_bytes();
    let mut open: Vec<Open> = Vec::new();
    // How many open elements are `svg` or `math`, and how many are places inside them where
    // HTML is parsed again. Between the two, tags follow the foreign-content rules: a
    // trailing `/>` closes the element (`<path d="…"/>`), and nothing is raw text.
    let (mut foreign, mut island) = (0usize, 0usize);
    let mut at = 0;
    while let Some(found) = find(bytes, at, b'<') {
        let rest = &bytes[found + 1..];
        match rest.first() {
            Some(b'!') => {
                at = if rest.starts_with(b"!--") {
                    skip_comment(bytes, found + 4)
                } else if foreign > 0 && rest.starts_with(b"![CDATA[") {
                    skip_past(bytes, found + 9, b"]]>")
                } else {
                    skip_past(bytes, found + 2, b">")
                };
            }
            Some(b'?') => at = skip_past(bytes, found + 2, b">"),
            Some(b'/') => {
                let (name, after) = tag_name(bytes, found + 2);
                if let Some(index) = open.iter().rposition(|open| open.name == name) {
                    for closed in open.drain(index..) {
                        foreign -= usize::from(closed.foreign_root);
                        island -= usize::from(closed.island);
                    }
                }
                at = skip_tag(bytes, after).0;
            }
            Some(first) if first.is_ascii_alphabetic() => {
                let (name, after) = tag_name(bytes, found + 1);
                let (end, self_closing) = skip_tag(bytes, after);
                at = end;
                let in_foreign = foreign > 0 && island == 0 && !BREAKOUT.contains(&name.as_str());
                if in_foreign || (is_foreign_root(&name) && island == 0) {
                    if self_closing {
                        continue;
                    }
                } else {
                    if VOID.contains(&name.as_str()) || IMPLIED_END.contains(&name.as_str()) {
                        continue;
                    }
                    if RAW_TEXT.contains(&name.as_str()) {
                        // Everything up to the end tag is text, whatever it looks like.
                        match find_end_tag(bytes, at, &name) {
                            Some(end) => at = end,
                            None => return false,
                        }
                        continue;
                    }
                }
                let foreign_root = is_foreign_root(&name);
                foreign += usize::from(foreign_root);
                let island_start = foreign > 0 && INTEGRATION_POINTS.contains(&name.as_str());
                island += usize::from(island_start);
                open.push(Open {
                    name,
                    foreign_root,
                    island: island_start,
                });
                if open.len() > limit {
                    return true;
                }
            }
            _ => at = found + 1,
        }
        if at >= bytes.len() {
            break;
        }
    }
    false
}

/// An open element, with what it added to the foreign-content counters.
struct Open {
    name: String,
    foreign_root: bool,
    island: bool,
}

fn is_foreign_root(name: &str) -> bool {
    matches!(name, "svg" | "math")
}

/// The index just past the comment whose text starts at `from`. A comment ends at `-->` or
/// `--!>`, and also right away at `<!-->` and `<!--->`.
fn skip_comment(bytes: &[u8], from: usize) -> usize {
    let tail = bytes.get(from..).unwrap_or_default();
    if tail.starts_with(b">") {
        return from + 1;
    }
    if tail.starts_with(b"->") {
        return from + 2;
    }
    let mut i = 0;
    while let Some(dashes) = tail
        .get(i..)
        .and_then(|rest| rest.windows(2).position(|w| w == b"--"))
    {
        let after = i + dashes + 2;
        match tail.get(after) {
            Some(b'>') => return from + after + 1,
            Some(b'!') if tail.get(after + 1) == Some(&b'>') => return from + after + 2,
            // `--->`: the second and third dash start the ending.
            Some(b'-') => i += dashes + 1,
            _ => i = after,
        }
    }
    bytes.len()
}

fn find(bytes: &[u8], from: usize, wanted: u8) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|b| *b == wanted)
        .map(|i| from + i)
}

/// The index just past the next `needle`, or the end of the input.
fn skip_past(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    bytes
        .get(from..)
        .and_then(|tail| tail.windows(needle.len()).position(|w| w == needle))
        .map_or(bytes.len(), |i| from + i + needle.len())
}

/// The lowercase tag name starting at `from`, and the index after it.
fn tag_name(bytes: &[u8], from: usize) -> (String, usize) {
    let tail = bytes.get(from..).unwrap_or_default();
    let len = tail
        .iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
        .unwrap_or(tail.len());
    let name = String::from_utf8_lossy(&tail[..len]).to_ascii_lowercase();
    (name, from + len)
}

/// The index just past the `>` that ends the tag whose attributes start at `from`, and
/// whether the tag ends in `/>`. Quoted attribute values may contain `>`.
fn skip_tag(bytes: &[u8], from: usize) -> (usize, bool) {
    let mut i = from;
    let mut slash = false;
    while i < bytes.len() {
        let byte = bytes[i];
        match byte {
            b'>' => return (i + 1, slash),
            b'=' => {
                slash = false;
                i += 1;
                while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                    i += 1;
                }
                match bytes.get(i) {
                    Some(&quote @ (b'"' | b'\'')) => {
                        i = find(bytes, i + 1, quote).map_or(bytes.len(), |end| end + 1);
                    }
                    _ => {
                        while bytes
                            .get(i)
                            .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                        {
                            i += 1;
                        }
                    }
                }
            }
            _ => {
                slash = byte == b'/';
                i += 1;
            }
        }
    }
    (bytes.len(), false)
}

/// The index of the `<` of the end tag for the raw-text element `name`, searching from `from`.
fn find_end_tag(bytes: &[u8], from: usize, name: &str) -> Option<usize> {
    let wanted = name.as_bytes();
    let mut at = from;
    while let Some(found) = find(bytes, at, b'<') {
        let tail = &bytes[found + 1..];
        if tail.first() == Some(&b'/')
            && tail.len() > wanted.len() + 1
            && tail[1..=wanted.len()].eq_ignore_ascii_case(wanted)
            && matches!(
                tail[wanted.len() + 1],
                b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | 0x0c
            )
        {
            return Some(found);
        }
        at = found + 1;
    }
    None
}

#[cfg(test)]
mod tests;
