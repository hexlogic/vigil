//! Configuration: built-in defaults < TOML file < `VIGIL_*` environment variables.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

/// Config file used when `--config` is not given and the file exists.
const DEFAULT_CONFIG_FILE: &str = "vigil.toml";

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable lines.
    Text,
    /// One JSON object per line.
    Json,
}

/// Settings for fetching pages (`[fetch]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchSection {
    /// Request timeout in seconds, for watches that do not set their own.
    pub timeout_secs: u64,
    /// Largest accepted response body in bytes.
    pub max_body_bytes: u64,
    /// `User-Agent` header; `vigil/<version>` when unset.
    pub user_agent: Option<String>,
}

impl Default for FetchSection {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            max_body_bytes: 10 * 1024 * 1024,
            user_agent: None,
        }
    }
}

/// Settings for importing watches (`[import]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSection {
    /// Largest file the web UI and the API accept for an import, in bytes.
    pub max_upload_bytes: u64,
}

impl Default for ImportSection {
    fn default() -> Self {
        Self {
            max_upload_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Settings for the check scheduler (`[scheduler]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerSection {
    /// Checks running at once.
    pub concurrency: usize,
    /// Checks running at once against one host.
    pub per_host: usize,
}

impl Default for SchedulerSection {
    fn default() -> Self {
        Self {
            concurrency: 8,
            per_host: 2,
        }
    }
}

/// A value that must not end up in a log: it prints as `***`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The value itself.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    /// Directory holding the SQLite database.
    pub data_dir: PathBuf,
    /// The address Vigil is reached at, such as `https://vigil.example.com`. Notifications use
    /// it for links to diff pages; without it they have none.
    pub base_url: Option<String>,
    /// The password for the web UI. Without it one is made up the first time the server runs
    /// and shown once in the log; with it, this is the password whatever was set before.
    pub admin_password: Option<Secret>,
    /// Let anyone who can reach the server in without a password. For a trusted network only.
    pub no_auth: bool,
    /// Log output format.
    pub log_format: LogFormat,
    /// Page fetching.
    pub fetch: FetchSection,
    /// Check scheduling.
    pub scheduler: SchedulerSection,
    /// Importing watches.
    pub import: ImportSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 5800)),
            data_dir: PathBuf::from("data"),
            base_url: None,
            admin_password: None,
            no_auth: false,
            log_format: LogFormat::Text,
            fetch: FetchSection::default(),
            scheduler: SchedulerSection::default(),
            import: ImportSection::default(),
        }
    }
}

impl Config {
    /// Location of the SQLite database inside the data directory.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("vigil.db")
    }

    /// The fetcher settings these options describe.
    pub fn fetch_config(&self) -> vigil_fetch::FetchConfig {
        let defaults = vigil_fetch::FetchConfig::default();
        vigil_fetch::FetchConfig {
            timeout: Duration::from_secs(self.fetch.timeout_secs),
            max_body_bytes: self.fetch.max_body_bytes,
            user_agent: self.fetch.user_agent.clone().unwrap_or(defaults.user_agent),
        }
    }

    /// The scheduler settings these options describe.
    pub fn scheduler_settings(&self) -> vigil_scheduler::Settings {
        vigil_scheduler::Settings {
            concurrency: self.scheduler.concurrency,
            per_host: self.scheduler.per_host,
            ..vigil_scheduler::Settings::default()
        }
    }

    /// Layer defaults, the TOML file (if any) and `VIGIL_*` environment variables.
    ///
    /// With `path == None`, `./vigil.toml` is used when it exists. `VIGIL_CONFIG` is
    /// ignored here: it selects the file (via the CLI) rather than configuring the app.
    pub fn load(path: Option<&Path>) -> Result<Self, Box<figment::Error>> {
        let file = match path {
            Some(path) => Some(path),
            None => Path::new(DEFAULT_CONFIG_FILE)
                .exists()
                .then_some(Path::new(DEFAULT_CONFIG_FILE)),
        };
        let mut figment = Figment::from(Serialized::defaults(Self::default()));
        if let Some(file) = file {
            figment = figment.merge(Toml::file(file));
        }
        figment
            .merge(Env::prefixed("VIGIL_").ignore(&["config"]).split("__"))
            .extract::<Self>()
            .map_err(Box::new)
            .and_then(Self::checked)
    }

    /// Refuse settings that cannot work.
    fn checked(self) -> Result<Self, Box<figment::Error>> {
        if let Some(url) = self
            .base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
        {
            check_base_url(url.trim())
                .map_err(|error| Box::new(figment::Error::from(format!("base_url: {error}"))))?;
        }
        Ok(self)
    }
}

/// `base_url` is put in front of `/watch/<id>/diff?...` in every notification, so it must be a
/// plain address: an `http(s)` URL with no query, fragment or credentials (which would be
/// sent to Telegram, Discord and Slack).
fn check_base_url(url: &str) -> Result<(), String> {
    vigil_fetch::check_url(url).map_err(|error| error.to_string())?;
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.contains('@') {
        return Err("must not contain a user name or password".into());
    }
    if url.contains('?') {
        return Err("must not have a query (?...)".into());
    }
    if url.contains('#') {
        return Err("must not have a fragment (#...)".into());
    }
    Ok(())
}

#[cfg(test)]
// `Jail::expect_with` fixes the closure error type to the (large) `figment::Error`.
#[allow(clippy::result_large_err)]
mod tests;
