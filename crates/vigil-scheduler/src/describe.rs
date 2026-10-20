//! Turning errors into one-line messages for the check log and reports.

use std::error::Error;

/// An error and its causes on one line: `outer: inner: root`.
///
/// Many errors already include their cause in their own message (`#[error("...: {0}")]`
/// with `#[from]`); a cause whose text is already present is not repeated.
pub(crate) fn describe(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let inner_text = inner.to_string();
        if !text.contains(&inner_text) {
            text.push_str(": ");
            text.push_str(&inner_text);
        }
        source = inner.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::describe;

    #[derive(Debug)]
    struct Leaf(&'static str);
    impl fmt::Display for Leaf {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Leaf {}

    /// Mentions its cause in its own message, like `#[error("...: {0}")]` with `#[from]`.
    #[derive(Debug)]
    struct Embedding(Leaf);
    impl fmt::Display for Embedding {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "outer failed: {}", self.0)
        }
    }
    impl std::error::Error for Embedding {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    /// Leaves its cause to the source chain, like `FetchError::Http`.
    #[derive(Debug)]
    struct Chained(Leaf);
    impl fmt::Display for Chained {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("request failed")
        }
    }
    impl std::error::Error for Chained {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn a_plain_error_is_just_its_message() {
        assert_eq!(describe(&Leaf("boom")), "boom");
    }

    #[test]
    fn causes_are_appended_when_the_message_leaves_them_out() {
        assert_eq!(
            describe(&Chained(Leaf("connection refused"))),
            "request failed: connection refused"
        );
    }

    #[test]
    fn causes_already_in_the_message_are_not_repeated() {
        assert_eq!(
            describe(&Embedding(Leaf("inner boom"))),
            "outer failed: inner boom"
        );
    }
}
