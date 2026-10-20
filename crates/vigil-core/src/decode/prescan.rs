//! Looking for a charset declared inside the page: `<meta charset>`, `<meta http-equiv>` and
//! the encoding of the `<?xml?>` declaration.

use encoding_rs::{Encoding, REPLACEMENT, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED};

/// How many leading bytes are searched for a declared charset.
const PRESCAN_LIMIT: usize = 1024;

/// What the `Content-Type` says about where a charset declaration may be found.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Markup {
    /// HTML: `<meta>` tags, and the XML declaration of XHTML.
    Html,
    /// Other XML (feeds and the like): the XML declaration only.
    Xml,
    /// Anything else (JSON, plain text, ...): nothing is looked for.
    Other,
}

impl Markup {
    fn of(content_type: Option<&str>) -> Self {
        let Some(content_type) = content_type else {
            return Self::Html;
        };
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "" | "text/html" | "application/xhtml+xml" => Self::Html,
            "text/xml" | "application/xml" => Self::Xml,
            other if other.ends_with("+xml") => Self::Xml,
            _ => Self::Other,
        }
    }
}

/// Look for a charset declared in the first [`PRESCAN_LIMIT`] bytes.
pub(super) fn prescan(bytes: &[u8], content_type: Option<&str>) -> Option<&'static Encoding> {
    let markup = Markup::of(content_type);
    if markup == Markup::Other {
        return None;
    }
    let head = &bytes[..bytes.len().min(PRESCAN_LIMIT)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    xml_encoding(&head).or_else(|| {
        if markup == Markup::Html {
            meta_charset(&head)
        } else {
            None
        }
    })
}

/// A declared label turned into an encoding that is safe to apply to ASCII-compatible bytes.
///
/// A page that declares UTF-16 but is readable as ASCII cannot be UTF-16, and browsers treat
/// the declaration as UTF-8; `x-user-defined` is treated as windows-1252 for the same reason.
fn declared(label: &str) -> Option<&'static Encoding> {
    let encoding = Encoding::for_label(label.trim().as_bytes())?;
    if encoding == REPLACEMENT {
        None
    } else if encoding == UTF_16LE || encoding == UTF_16BE {
        Some(UTF_8)
    } else if encoding == X_USER_DEFINED {
        Some(WINDOWS_1252)
    } else {
        Some(encoding)
    }
}

/// The `encoding` of an `<?xml ...?>` declaration at the very start of `head`.
fn xml_encoding(head: &str) -> Option<&'static Encoding> {
    let declaration = head.strip_prefix("<?xml")?;
    let declaration = &declaration[..declaration.find("?>")?];
    let after = declaration[declaration.find("encoding")? + "encoding".len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = &after[1..];
    declared(&value[..value.find(quote)?])
}

/// The first usable charset declared by a `<meta>` tag in `head`.
fn meta_charset(head: &str) -> Option<&'static Encoding> {
    let mut rest = head;
    while let Some(start) = rest.find("<meta") {
        rest = &rest[start + "<meta".len()..];
        let boundary = rest.chars().next()?;
        if !(boundary.is_whitespace() || boundary == '/' || boundary == '>') {
            continue;
        }
        // A tag cut off by the end of the prescan window is ignored.
        let end = rest.find('>')?;
        if let Some(encoding) = charset_of_meta(&attributes(&rest[..end])) {
            return Some(encoding);
        }
        rest = &rest[end..];
    }
    None
}

fn charset_of_meta(attributes: &[(&str, &str)]) -> Option<&'static Encoding> {
    let get = |wanted: &str| {
        attributes
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, value)| *value)
    };
    if let Some(label) = get("charset") {
        return declared(label);
    }
    if get("http-equiv")?.trim() != "content-type" {
        return None;
    }
    let content = get("content")?;
    let after = content[content.find("charset")? + "charset".len()..].trim_start();
    let label = after
        .strip_prefix('=')?
        .trim_start()
        .trim_start_matches(['"', '\'']);
    declared(
        label
            .split(|c: char| c == ';' || c == '"' || c == '\'' || c.is_whitespace())
            .next()
            .unwrap_or_default(),
    )
}

/// Name/value pairs of the attributes in the inside of a start tag. Names and values are
/// returned as written (the caller has lowercased the text); valueless attributes get "".
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let separators = |c: char| c.is_whitespace() || c == '/';
    let mut found = Vec::new();
    let mut rest = tag.trim_start_matches(separators);
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| separators(c) || c == '=')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        let Some(after) = rest.strip_prefix('=') else {
            if name.is_empty() {
                break;
            }
            found.push((name, ""));
            rest = rest.trim_start_matches(separators);
            continue;
        };
        let after = after.trim_start();
        let (value, tail) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let inner = &after[1..];
                match inner.find(quote) {
                    Some(close) => (&inner[..close], &inner[close + 1..]),
                    None => (inner, ""),
                }
            }
            _ => after.split_at(after.find(char::is_whitespace).unwrap_or(after.len())),
        };
        found.push((name, value));
        rest = tail.trim_start_matches(separators);
    }
    found
}
