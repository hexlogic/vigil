//! Values that must never reach a log line or an error message.

use std::fmt;

use url::Url;

/// A credential: a token, a password, or any part of a URL that grants access.
///
/// `Debug` and `Display` print `***`, so a secret that ends up in a log line, an error message
/// or a derived `Debug` does not leak. Read it with [`Secret::expose`] where it is sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The credential itself. Only call this to put it on the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A URL whose path or query may carry a credential, as webhook URLs often do.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretUrl(Url);

impl SecretUrl {
    /// Wrap a URL.
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    /// The URL itself. Only call this to make the request.
    pub fn expose(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretUrl(***)")
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}
