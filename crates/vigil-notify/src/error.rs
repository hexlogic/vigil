//! Errors from parsing notification URLs.

/// Why a notification URL was refused.
///
/// The messages say what is wrong, never what the URL contained: it usually holds credentials
/// and the message ends up in terminals and logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The text has no `scheme://` in front.
    #[error("not a notification URL: expected scheme://... (for example ntfys://ntfy.sh/my-topic)")]
    NotAUrl,
    /// The scheme is not one of the supported channels.
    #[error("{scheme}:// notifications are not supported yet; supported schemes: {supported}")]
    Unsupported {
        /// The scheme that was given.
        scheme: String,
        /// The schemes that are supported, comma separated.
        supported: String,
    },
    /// The scheme is known but something else is wrong with the URL.
    #[error("{scheme}:// {problem}")]
    Invalid {
        /// The scheme of the URL.
        scheme: &'static str,
        /// What is wrong, without quoting the URL.
        problem: String,
    },
}

/// Build a [`ParseError::Invalid`].
pub(crate) fn invalid(scheme: &'static str, problem: impl Into<String>) -> ParseError {
    ParseError::Invalid {
        scheme,
        problem: problem.into(),
    }
}

/// A message template that does not compile or does not render.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("template: {0}")]
pub struct TemplateError(pub(crate) String);

impl From<minijinja::Error> for TemplateError {
    fn from(error: minijinja::Error) -> Self {
        // Without minijinja's `debug` feature the message holds the problem and its position
        // but not the template source.
        Self(error.to_string())
    }
}

/// Why a notification could not be delivered.
///
/// The message never holds the target URL or any credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SendError {
    /// What went wrong.
    pub message: String,
    /// Whether trying again later may help: a network problem or a busy or failing server, as
    /// opposed to a refused request.
    pub transient: bool,
    /// How long the server asked to wait before the next attempt.
    pub retry_after: Option<std::time::Duration>,
}

impl SendError {
    /// An error that trying again will not fix.
    pub(crate) fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: false,
            retry_after: None,
        }
    }

    /// An error that may go away.
    pub(crate) fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: true,
            retry_after: None,
        }
    }
}

/// An error and its causes on one line: `outer: inner: root`.
///
/// Many errors already include their cause in their own message (`#[error("...: {0}")]`
/// with `#[from]`); a cause whose text is already present is not repeated. The scheduler uses
/// this for the check log too, so there is one definition.
pub fn describe_error(error: &dyn std::error::Error) -> String {
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

    use super::describe_error as describe;

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
