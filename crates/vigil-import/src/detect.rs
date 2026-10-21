//! Telling what a file is from its content.

use crate::{Format, MAX_TEXT_BYTES};

/// How much of the start of a file is looked at to tell a list from a job file.
const SNIFF_BYTES: usize = 8192;

/// The format of `bytes`, or `None` if they look like none.
pub(crate) fn detect(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Some(Format::Backup);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(SNIFF_BYTES)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with('<') {
        return head
            .to_ascii_lowercase()
            .contains("<opml")
            .then_some(Format::Opml);
    }
    if head.starts_with('{') {
        return json_format(bytes);
    }
    let mut lines = head
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let first = lines.next()?;
    if first.starts_with("http://") || first.starts_with("https://") {
        return Some(Format::UrlList);
    }
    let is_job_key = |line: &str| {
        let line = line.trim_start_matches("- ").trim_start();
        ["url:", "navigate:", "command:", "name:", "filter:", "kind:"]
            .iter()
            .any(|key| line.starts_with(key))
    };
    (first == "---" || is_job_key(first) || lines.any(is_job_key)).then_some(Format::Urlwatch)
}

/// A JSON file is a datastore (it has a `watching` object) or a Distill export (it has a
/// `data` list of items with a `uri`).
fn json_format(bytes: &[u8]) -> Option<Format> {
    if bytes.len() > MAX_TEXT_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value
        .get("watching")
        .is_some_and(serde_json::Value::is_object)
    {
        return Some(Format::Backup);
    }
    let first = value.get("data")?.as_array()?.first()?;
    first.get("uri").is_some().then_some(Format::Distill)
}

#[cfg(test)]
mod tests {
    use super::detect;
    use crate::Format;

    #[test]
    fn each_format_is_told_by_its_shape() {
        let cases: [(&[u8], Option<Format>); 11] = [
            (b"PK\x03\x04rest of a zip", Some(Format::Backup)),
            (
                b"https://a.example/\nhttps://b.example/ news\n",
                Some(Format::UrlList),
            ),
            (b"# my sites\n\nhttp://a.example/\n", Some(Format::UrlList)),
            (
                b"\xef\xbb\xbf<?xml version=\"1.0\"?><opml version=\"2.0\"></opml>",
                Some(Format::Opml),
            ),
            (b"<?xml version=\"1.0\"?><rss></rss>", None),
            (
                br#"{"watching": {"x": {}}, "settings": {}}"#,
                Some(Format::Backup),
            ),
            (
                br#"{"data": [{"uri": "https://a.example/", "name": "A"}]}"#,
                Some(Format::Distill),
            ),
            (br#"{"hello": "world"}"#, None),
            (
                b"name: Shop\nurl: https://a.example/\n---\nurl: https://b.example/\n",
                Some(Format::Urlwatch),
            ),
            (
                b"---\nkind: url\nurl: https://a.example/\n",
                Some(Format::Urlwatch),
            ),
            (b"just some words\n", None),
        ];
        for (bytes, want) in cases {
            assert_eq!(detect(bytes), want, "{}", String::from_utf8_lossy(bytes));
        }
        assert_eq!(detect(b""), None);
    }
}
