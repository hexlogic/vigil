//! The `feed` processor: RSS, Atom and JSON Feed entries as stable text blocks.

use std::borrow::Cow;

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
    let feed = feed_rs::parser::parse(declare_utf8(body).as_bytes())?;
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

/// Make the XML declaration say UTF-8.
///
/// The body has already been decoded to a string, but the parser would still honour an
/// `encoding="windows-1251"` in the declaration and decode the UTF-8 bytes a second time.
fn declare_utf8(body: &str) -> Cow<'_, str> {
    let Some(rest) = body.strip_prefix("<?xml") else {
        return Cow::Borrowed(body);
    };
    let Some(end) = rest.find("?>") else {
        return Cow::Borrowed(body);
    };
    let declaration = &rest[..end];
    let Some(at) = declaration.find("encoding") else {
        return Cow::Borrowed(body);
    };
    let after = &declaration[at + "encoding".len()..];
    let Some(open) = after.find(['"', '\'']) else {
        return Cow::Borrowed(body);
    };
    let Some(quote) = after[open..].chars().next() else {
        return Cow::Borrowed(body);
    };
    let value_start = at + "encoding".len() + open + 1;
    let Some(len) = declaration[value_start..].find(quote) else {
        return Cow::Borrowed(body);
    };
    Cow::Owned(format!(
        "<?xml{}utf-8{}{}",
        &declaration[..value_start],
        &declaration[value_start + len..],
        &rest[end..]
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
