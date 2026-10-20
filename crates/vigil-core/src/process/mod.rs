//! Turning a decoded body into the text that gets hashed and diffed.

mod feed;
mod json;

use crate::model::Processor;

/// Largest text a processor or step may produce, so a small input cannot expand without
/// bound (a regex replacement, a jq program, a deeply repeated structure).
pub(crate) const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Columns used when rendering HTML. Wide enough that lines are never re-wrapped, so a
/// layout tweak elsewhere on the page cannot reflow unrelated text.
const RENDER_WIDTH: usize = 10_000;

/// Why a body could not be processed.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// HTML could not be converted to text.
    #[error("converting html to text failed: {0}")]
    Html(#[from] html2text::Error),
    /// The body is not valid JSON.
    #[error("the response is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The body is not an RSS, Atom or JSON feed.
    #[error("the response is not a valid feed: {0}")]
    Feed(#[from] feed_rs::parser::ParseFeedError),
    /// The JSON nests deeper than [`json::MAX_DEPTH`] levels.
    #[error("the JSON nests deeper than {} levels", json::MAX_DEPTH)]
    TooDeep,
    /// The text grew past [`MAX_OUTPUT_BYTES`].
    #[error("the processed text is larger than {} MiB", MAX_OUTPUT_BYTES / (1024 * 1024))]
    TooLarge,
}

/// Run `processor` over `body`.
pub fn process(
    processor: Processor,
    body: &str,
    content_type: Option<&str>,
) -> Result<String, ProcessError> {
    match processor {
        Processor::Text => process_text(body, content_type),
        Processor::Json => process_json(body),
        Processor::Feed => process_feed(body),
    }
}

/// The `json` processor: the body is parsed and printed with two-space indentation and object
/// keys sorted at every level, so key order and formatting never register as changes.
///
/// Numbers go through the parser, so integers beyond 64 bits are read as floating point and
/// lose precision.
pub fn process_json(body: &str) -> Result<String, ProcessError> {
    json::render_canonical(&serde_json::from_str(body)?)
}

/// The `feed` processor: RSS, Atom or JSON Feed entries, one block of title, link and date
/// each, newest first. The content type is ignored.
pub fn process_feed(body: &str) -> Result<String, ProcessError> {
    feed::render_feed(body)
}

/// The `text` processor: HTML becomes readable text, anything else passes through unchanged.
pub fn process_text(body: &str, content_type: Option<&str>) -> Result<String, ProcessError> {
    if !looks_like_html(body, content_type) {
        return Ok(body.to_owned());
    }
    let text = html2text::config::plain_no_decorate()
        .no_link_wrapping()
        .string_from_read(body.as_bytes(), RENDER_WIDTH)?;
    Ok(text)
}

/// Trust the `Content-Type` when there is one; otherwise sniff the start of the body.
fn looks_like_html(body: &str, content_type: Option<&str>) -> bool {
    match content_type {
        Some(ct) => {
            let essence = ct.split(';').next().unwrap_or_default().trim();
            essence.eq_ignore_ascii_case("text/html")
                || essence.eq_ignore_ascii_case("application/xhtml+xml")
        }
        None => {
            let head: String = body
                .trim_start()
                .chars()
                .take(15)
                .collect::<String>()
                .to_ascii_lowercase();
            head.starts_with("<!doctype html") || head.starts_with("<html")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() -> Result<(), ProcessError> {
        let out = process_text("a  b\n<b>c</b>\n", Some("text/plain; charset=utf-8"))?;
        assert_eq!(out, "a  b\n<b>c</b>\n");
        Ok(())
    }

    #[test]
    fn html_is_converted_to_text() -> Result<(), ProcessError> {
        let out = process_text("<h1>Title</h1><p>Hello <b>world</b></p>", Some("text/html"))?;
        assert!(out.contains("Title"), "{out:?}");
        assert!(out.contains("Hello world"), "{out:?}");
        assert!(!out.contains('<'), "{out:?}");
        Ok(())
    }

    #[test]
    fn missing_content_type_sniffs_html() -> Result<(), ProcessError> {
        let out = process_text("  <!DOCTYPE html><p>hi</p>", None)?;
        assert!(!out.contains('<'), "{out:?}");
        Ok(())
    }

    #[test]
    fn missing_content_type_keeps_non_html_as_is() -> Result<(), ProcessError> {
        assert_eq!(process_text("{\"a\": 1}", None)?, "{\"a\": 1}");
        Ok(())
    }

    #[test]
    fn long_lines_are_not_wrapped() -> Result<(), ProcessError> {
        let words = vec!["word"; 80].join(" ");
        let out = process_text(&format!("<p>{words}</p>"), Some("text/html"))?;
        assert_eq!(out.trim(), words);
        Ok(())
    }

    #[test]
    fn script_and_style_content_is_dropped() -> Result<(), ProcessError> {
        let html = "<style>p{color:red}</style><script>var x=1;</script><p>shown</p>";
        let out = process_text(html, Some("text/html"))?;
        assert!(out.contains("shown"));
        assert!(!out.contains("color"));
        assert!(!out.contains("var x"));
        Ok(())
    }

    #[test]
    fn json_is_pretty_printed_with_sorted_keys() -> Result<(), ProcessError> {
        let out = process(
            Processor::Json,
            r#"{"b": [1, 2], "a": {"y": null, "x": true}}"#,
            Some("application/json"),
        )?;
        assert_eq!(
            out,
            "{\n  \"a\": {\n    \"x\": true,\n    \"y\": null\n  },\n  \"b\": [\n    1,\n    2\n  ]\n}\n"
        );
        Ok(())
    }

    #[test]
    fn the_content_type_does_not_matter_for_json() -> Result<(), ProcessError> {
        let a = process(Processor::Json, "[1]", None)?;
        let b = process(Processor::Json, "[1]", Some("text/html"))?;
        assert_eq!(a, b);
        Ok(())
    }

    #[test]
    fn invalid_json_is_an_error() {
        for body in ["", "not json", "{\"a\": }", "<html></html>"] {
            let error = process(Processor::Json, body, None)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                error.starts_with("the response is not valid JSON: "),
                "{body}: {error}"
            );
        }
    }
}
