//! How the web UI is set up by whoever runs the server.

use std::path::PathBuf;
use std::time::Duration;

/// Whether the UI asks for a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// People sign in with the admin password.
    Password,
    /// Anyone who can reach the server is let in. For a trusted network only.
    Disabled,
}

/// Settings of the web UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    /// Whether people must sign in.
    pub auth: Auth,
    /// The address Vigil is served at, as configured (`base_url`), without a trailing slash.
    pub base_url: Option<String>,
    /// Version shown in the footer, and in asset URLs so a new release is fetched afresh.
    pub version: &'static str,
    /// How long a wrong password is made to wait for its answer. Tests set it to zero.
    pub failed_login_delay: Duration,
    /// The password is set by `VIGIL_ADMIN_PASSWORD`, which wins at every start, so changing it
    /// in the UI would be undone by the next one. The settings page then does not offer it.
    pub password_from_env: bool,
    /// The largest file an import accepts, in bytes.
    pub max_upload_bytes: u64,
    /// Where an uploaded file is kept while it is read. It should be on the disk the data is on,
    /// not in memory: a backup can be large.
    pub upload_dir: PathBuf,
}

impl WebConfig {
    /// Settings for a server reached at `base_url`, if it is known.
    pub fn new(auth: Auth, base_url: Option<&str>) -> Self {
        Self {
            auth,
            base_url: base_url
                .map(|url| url.trim().trim_end_matches('/').to_owned())
                .filter(|url| !url.is_empty()),
            version: env!("CARGO_PKG_VERSION"),
            failed_login_delay: Duration::from_secs(1),
            password_from_env: false,
            max_upload_bytes: 256 * 1024 * 1024,
            upload_dir: std::env::temp_dir(),
        }
    }

    /// The path part of the base URL (`/vigil` for `https://example.com/vigil`), which every
    /// route lives under; empty when Vigil is served at the root.
    pub fn base_path(&self) -> String {
        let Some(url) = &self.base_url else {
            return String::new();
        };
        let after_scheme = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        after_scheme
            .find('/')
            .map_or_else(String::new, |at| after_scheme[at..].to_owned())
    }

    /// The origin (`https://example.com`) the UI is reached at, if the base URL is known.
    pub fn origin(&self) -> Option<String> {
        let url = self.base_url.as_ref()?;
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest.split('/').next()?;
        Some(format!("{scheme}://{authority}"))
    }

    /// Whether cookies should be marked `Secure`: when the UI is served over https.
    pub fn secure_cookies(&self) -> bool {
        self.base_url
            .as_deref()
            .is_some_and(|url| url.to_ascii_lowercase().starts_with("https://"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_url: Option<&str>) -> WebConfig {
        WebConfig::new(Auth::Password, base_url)
    }

    #[test]
    fn the_base_path_is_what_follows_the_host() {
        assert_eq!(config(None).base_path(), "");
        assert_eq!(config(Some("https://vigil.example.com")).base_path(), "");
        assert_eq!(config(Some("https://vigil.example.com/")).base_path(), "");
        assert_eq!(
            config(Some("https://example.com/vigil")).base_path(),
            "/vigil"
        );
        assert_eq!(
            config(Some("http://10.0.0.2:5800/a/b/")).base_path(),
            "/a/b"
        );
        assert_eq!(config(Some("  ")).base_path(), "");
    }

    #[test]
    fn the_origin_leaves_out_the_path() {
        assert_eq!(config(None).origin(), None);
        assert_eq!(
            config(Some("https://example.com/vigil"))
                .origin()
                .as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            config(Some("http://10.0.0.2:5800")).origin().as_deref(),
            Some("http://10.0.0.2:5800")
        );
    }

    #[test]
    fn cookies_are_secure_only_over_https() {
        assert!(config(Some("https://example.com")).secure_cookies());
        assert!(config(Some("HTTPS://example.com")).secure_cookies());
        assert!(!config(Some("http://example.com")).secure_cookies());
        assert!(!config(None).secure_cookies());
    }
}
