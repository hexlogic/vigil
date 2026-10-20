//! The `feed` processor: RSS, Atom and JSON Feed entries as stable text blocks.

use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use feed_rs::model::{Entry, Link};

use super::{MAX_OUTPUT_BYTES, ProcessError};

/// Render each entry of the feed in `body` as a block of up to three lines (title, link,
/// date), newest first, with a blank line between blocks.
///
/// The date is `published`, or `updated` when there is none, in UTC as `2026-10-12T09:00:00Z`.
/// Entries without a date follow the dated ones in the order the feed lists them. Lines a
/// feed leaves out are left out, and entries with none of the three are skipped. Feed-level
/// metadata (title, build date) is not part of the text, so touching it changes nothing.
pub(super) fn render_feed(body: &str) -> Result<String, ProcessError> {
    // The XML parser asserts instead of failing on some malformed input (a second `<?xml?>`
    // declaration with another encoding, found by fuzzing), so a panic is an unreadable feed.
    let feed = match catch_unwind(AssertUnwindSafe(|| {
        feed_rs::parser::parse(declare_utf8(body).as_bytes())
    })) {
        Ok(parsed) => parsed?,
        Err(_) => return Err(ProcessError::FeedUnreadable),
    };
    let mut entries: Vec<_> = feed.entries.iter().map(Block::of).collect();
    // Newest first. The sort is stable and `None` is the smallest date, so undated entries end
    // up last, in source order.
    entries.sort_by_key(|block| std::cmp::Reverse(block.date));
    let mut out = String::new();
    for block in entries.iter().filter(|block| !block.is_empty()) {
        if !out.is_empty() {
            out.push('\n');
        }
        block.write_to(&mut out);
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(ProcessError::TooLarge);
        }
    }
    Ok(out)
}

struct Block {
    title: Option<String>,
    link: Option<String>,
    date: Option<i64>,
    date_text: Option<String>,
}

impl Block {
    fn of(entry: &Entry) -> Self {
        let when = entry.published.or(entry.updated);
        Self {
            title: entry
                .title
                .as_ref()
                .map(|title| one_line(&title.content))
                .filter(|title| !title.is_empty()),
            link: best_link(&entry.links)
                .map(|link| one_line(&link.href))
                .filter(|link| !link.is_empty()),
            date: when.map(|when| when.timestamp()),
            date_text: when.map(|when| when.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        }
    }

    fn is_empty(&self) -> bool {
        self.title.is_none() && self.link.is_none() && self.date_text.is_none()
    }

    fn write_to(&self, out: &mut String) {
        for line in [&self.title, &self.link, &self.date_text]
            .into_iter()
            .flatten()
        {
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// The alternate link (or one with no `rel`), else the first link.
fn best_link(links: &[Link]) -> Option<&Link> {
    links
        .iter()
        .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or_else(|| links.first())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Make every XML declaration say UTF-8.
///
/// The body has already been decoded to a string, but the parser would still honour an
/// `encoding="windows-1251"` in a declaration and decode the UTF-8 bytes a second time.
/// All declarations are rewritten, not just the first: a second one that names another
/// encoding makes the parser panic.
fn declare_utf8(body: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut rest = body;
    let mut changed = false;
    while let Some(start) = rest.find("<?xml") {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);
        let after_name = &tail["<?xml".len()..];
        let is_declaration = after_name.starts_with(|c: char| c.is_whitespace() || c == '?');
        let Some(end) = tail.find("?>").filter(|_| is_declaration) else {
            // `<?xml-stylesheet ...?>`, or no end: leave it alone and read on.
            out.push_str("<?xml");
            rest = after_name;
            continue;
        };
        let (declaration, after) = tail.split_at(end);
        match with_utf8(declaration) {
            Some(replaced) => {
                out.push_str(&replaced);
                changed = true;
            }
            None => out.push_str(declaration),
        }
        out.push_str("?>");
        rest = &after["?>".len()..];
    }
    if !changed {
        return Cow::Borrowed(body);
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// `declaration` (an XML declaration without its closing `?>`) with its encoding set to
/// `utf-8`, or `None` if it names none.
fn with_utf8(declaration: &str) -> Option<String> {
    let at = declaration.find("encoding")? + "encoding".len();
    let open = at + declaration[at..].find(['"', '\''])?;
    let quote = declaration[open..].chars().next()?;
    let value_start = open + 1;
    let len = declaration[value_start..].find(quote)?;
    Some(format!(
        "{}utf-8{}",
        &declaration[..value_start],
        &declaration[value_start + len..]
    ))
}

#[cfg(test)]
mod tests {
    use super::{declare_utf8, render_feed};
    use crate::process::ProcessError;

    #[test]
    fn the_declared_encoding_is_replaced() {
        assert_eq!(
            declare_utf8("<?xml version=\"1.0\" encoding=\"windows-1251\"?><rss/>"),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><rss/>"
        );
        assert_eq!(
            declare_utf8("<?xml encoding='koi8-r' version='1.0'?>x"),
            "<?xml encoding='utf-8' version='1.0'?>x"
        );
    }

    #[test]
    fn every_declaration_is_rewritten_and_the_rest_is_kept() {
        assert_eq!(
            declare_utf8(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a/><?xml encoding='cp1251'?><b/>"
            ),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><a/><?xml encoding='utf-8'?><b/>"
        );
        let stylesheet = "<?xml-stylesheet href=\"a.xsl\" encoding=\"x\"?><rss/>";
        assert_eq!(declare_utf8(stylesheet), stylesheet);
    }

    #[test]
    fn bodies_without_a_declared_encoding_are_untouched() {
        for body in [
            "<rss/>",
            "<?xml version=\"1.0\"?><rss/>",
            "<?xml encoding",
            "<?xml encoding=\"x?>",
            "<?xml encoding=?>",
            "{\"version\": \"https://jsonfeed.org/version/1.1\"}",
        ] {
            assert_eq!(declare_utf8(body), body);
        }
    }

    #[test]
    fn entries_with_nothing_to_show_are_skipped() -> Result<(), ProcessError> {
        let rss = "<rss version=\"2.0\"><channel><title>t</title>\
                   <item><description>only a summary</description></item>\
                   <item><title>Kept</title></item></channel></rss>";
        assert_eq!(render_feed(rss)?, "Kept\n");
        Ok(())
    }

    #[test]
    fn a_second_xml_declaration_does_not_panic_the_parser() {
        // Found by fuzzing: quick-xml asserts when a later declaration names another encoding.
        let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                    <?xml version=\"1.0\" encoding=\"windows-1251\"?>\
                    <rss version=\"2.0\"><channel><title>t</title>\
                    <item><title>one</title></item></channel></rss>";
        // Either outcome is fine; what matters is that it comes back.
        let _ = render_feed(body);
        let broken = format!("{body}{body}");
        let _ = render_feed(&broken);
    }

    #[test]
    fn an_empty_feed_renders_as_nothing() -> Result<(), ProcessError> {
        assert_eq!(
            render_feed("<rss version=\"2.0\"><channel><title>t</title></channel></rss>")?,
            ""
        );
        Ok(())
    }

    #[test]
    fn something_that_is_not_a_feed_is_an_error() {
        for body in ["", "plain text", "<html><body>hi</body></html>", "{}"] {
            assert!(render_feed(body).is_err(), "{body:?}");
        }
    }
}
