//! Turning a decoded body into the text that gets hashed and diffed.

use crate::model::Processor;

/// Columns used when rendering HTML. Wide enough that lines are never re-wrapped, so a
/// layout tweak elsewhere on the page cannot reflow unrelated text.
const RENDER_WIDTH: usize = 10_000;

/// Why a body could not be processed.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The processor is not implemented yet.
    #[error("the {0} processor is not supported yet")]
    Unsupported(Processor),
    /// HTML could not be converted to text.
    #[error("converting html to text failed: {0}")]
    Html(#[from] html2text::Error),
}

/// Run `processor` over `body`.
pub fn process(
    processor: Processor,
    body: &str,
    content_type: Option<&str>,
) -> Result<String, ProcessError> {
    match processor {
        Processor::Text => process_text(body, content_type),
        other => Err(ProcessError::Unsupported(other)),
    }
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
    fn unfinished_processors_report_unsupported() {
        for p in [Processor::Json, Processor::Feed] {
            assert!(matches!(
                process(p, "{}", None),
                Err(ProcessError::Unsupported(found)) if found == p
            ));
        }
    }
}
