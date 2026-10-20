//! Text normalisation applied before hashing, so cosmetic differences never count as changes.

/// Normalise line endings and whitespace.
///
/// CRLF and lone CR become LF, trailing whitespace is trimmed from every line, runs of
/// blank lines collapse to one, and leading and trailing blank lines are dropped. Non-empty
/// output always ends with exactly one newline; text with no content normalises to `""`.
pub fn normalize(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len());
    let mut pending_blank = false;
    for line in unified.split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            pending_blank = !out.is_empty();
            continue;
        }
        if pending_blank {
            out.push('\n');
            pending_blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn converts_line_endings() {
        assert_eq!(normalize("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn trims_trailing_whitespace_but_keeps_indentation() {
        assert_eq!(normalize("  a \t\n b  \n"), "  a\n b\n");
    }

    #[test]
    fn collapses_blank_runs() {
        assert_eq!(normalize("a\n\n\n\nb\n"), "a\n\nb\n");
    }

    #[test]
    fn whitespace_only_lines_count_as_blank() {
        assert_eq!(normalize("a\n \t \nb\n"), "a\n\nb\n");
    }

    #[test]
    fn trims_leading_and_trailing_blank_lines() {
        assert_eq!(normalize("\n\n a\n\n\n"), " a\n");
    }

    #[test]
    fn adds_missing_final_newline() {
        assert_eq!(normalize("a"), "a\n");
    }

    #[test]
    fn empty_and_blank_input_normalise_to_empty() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize(" \n\r\n\t"), "");
    }

    #[test]
    fn is_idempotent() {
        let once = normalize("x \r\n\r\n\r\n y\t\n");
        assert_eq!(normalize(&once), once);
    }
}
