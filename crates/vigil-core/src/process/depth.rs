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

/// Elements whose content is text up to their end tag.
const RAW_TEXT: &[&str] = &[
    "script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes",
];

/// True when `html` nests more than `limit` elements, not counting the ones the parser
/// closes by itself. Elements are matched by name; an end tag that matches nothing open is
/// ignored, as the parser ignores it, so stray end tags cannot hide the depth.
pub(super) fn nests_deeper_than(html: &str, limit: usize) -> bool {
    let bytes = html.as_bytes();
    let mut open: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(found) = find(bytes, at, b'<') {
        let rest = &bytes[found + 1..];
        match rest.first() {
            Some(b'!') => {
                at = if rest.starts_with(b"!--") {
                    skip_past(bytes, found + 4, b"-->")
                } else {
                    skip_past(bytes, found + 2, b">")
                };
            }
            Some(b'?') => at = skip_past(bytes, found + 2, b">"),
            Some(b'/') => {
                let (name, after) = tag_name(bytes, found + 2);
                if let Some(index) = open.iter().rposition(|open| *open == name) {
                    open.truncate(index);
                }
                at = skip_tag(bytes, after);
            }
            Some(first) if first.is_ascii_alphabetic() => {
                let (name, after) = tag_name(bytes, found + 1);
                at = skip_tag(bytes, after);
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
                open.push(name);
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

/// The index just past the `>` that ends the tag whose attributes start at `from`. Quoted
/// attribute values may contain `>`.
fn skip_tag(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'>' => return i + 1,
            b'=' => {
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
            _ => i += 1,
        }
    }
    bytes.len()
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
mod tests {
    use super::nests_deeper_than;

    fn nested(tag: &str, depth: usize) -> String {
        format!("<{tag}>").repeat(depth) + "x" + &format!("</{tag}>").repeat(depth)
    }

    #[test]
    fn plain_nesting_is_counted() {
        assert!(nests_deeper_than(&nested("div", 513), 512));
        assert!(!nests_deeper_than(&nested("div", 512), 512));
        assert!(nests_deeper_than(&nested("DIV", 600), 512));
        assert!(
            nests_deeper_than(&"<b>".repeat(2_000), 512),
            "unclosed b nests"
        );
    }

    #[test]
    fn a_huge_page_is_refused_without_reading_all_of_it() {
        let started = std::time::Instant::now();
        assert!(nests_deeper_than(&nested("div", 1_000_000), 512));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn stray_end_tags_do_not_hide_the_depth() {
        assert!(nests_deeper_than(&"<div></x>".repeat(2_000), 512));
        assert!(nests_deeper_than(
            &"<div></span></p></div2>".repeat(2_000),
            512
        ));
    }

    #[test]
    fn balanced_siblings_do_not_add_up() {
        assert!(!nests_deeper_than(&"<div>x</div>".repeat(50_000), 512));
    }

    #[test]
    fn elements_the_parser_closes_itself_are_not_counted() {
        assert!(!nests_deeper_than(&"<li>item".repeat(5_000), 512));
        assert!(!nests_deeper_than(&"<p>para".repeat(5_000), 512));
        assert!(!nests_deeper_than(&"<tr><td>a<td>b".repeat(5_000), 512));
        assert!(!nests_deeper_than(
            &"<br><img src=x><input>".repeat(5_000),
            512
        ));
    }

    #[test]
    fn comments_declarations_and_raw_text_are_skipped() {
        let tags = "<div>".repeat(1_000);
        for wrapped in [
            format!("<!-- {tags} -->"),
            format!("<script>{tags}</script>"),
            format!("<style>{tags}</STYLE >"),
            format!("<textarea>{tags}</textarea>"),
            format!("<title>{tags}</title>"),
            format!("<script>{tags}"),
        ] {
            assert!(!nests_deeper_than(&wrapped, 512), "{}", &wrapped[..20]);
        }
        assert!(nests_deeper_than(&format!("<script>x</script>{tags}"), 512));
        // Like the tokenizer, a declaration or processing instruction ends at the first `>`.
        assert!(!nests_deeper_than(
            &"<!DOCTYPE html><?xml version=\"1.0\"?>".repeat(5_000),
            512
        ));
        assert!(nests_deeper_than(&format!("<!DOCTYPE html {tags}"), 512));
    }

    #[test]
    fn quoted_attribute_values_may_contain_angle_brackets() {
        let tricky = r#"<div title="a > <div> b" data-x='<<'>"#.repeat(600);
        assert!(nests_deeper_than(&tricky, 512));
        let balanced = r#"<a href="x>y">text</a>"#.repeat(5_000);
        assert!(!nests_deeper_than(&balanced, 512));
    }

    #[test]
    fn malformed_markup_does_not_panic() {
        for html in [
            "",
            "<",
            "<<",
            "</",
            "</>",
            "<!",
            "<!-",
            "<!--",
            "<?",
            "<a",
            "<a ",
            "<a b",
            "<a b=",
            "<a b='",
            "<a b=\"",
            "<a b=c",
            "<script",
            "<script>",
            "<script></",
            "<script></scrip",
            "<title>",
            "</div",
            "< div>",
            "<1>",
            "<a/>",
            "<a / >",
            "\u{0}<\u{0}>",
        ] {
            let _ = nests_deeper_than(html, 512);
        }
    }
}
