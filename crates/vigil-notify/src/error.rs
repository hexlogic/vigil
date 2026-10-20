//! Errors from parsing notification URLs.

/// Why a notification URL was refused.
///
/// The messages say what is wrong, never what the URL contained: it usually holds credentials
/// and the message ends up in terminals and logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The text has no `scheme://` in front.
    #[error("not a notification URL: expected scheme://... (for example ntfy://ntfy.sh/my-topic)")]
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
