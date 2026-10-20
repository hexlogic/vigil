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

/// Runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    /// Directory holding the SQLite database.
    pub data_dir: PathBuf,
    /// Log output format.
    pub log_format: LogFormat,
    /// Page fetching.
    pub fetch: FetchSection,
    /// Check scheduling.
    pub scheduler: SchedulerSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 5800)),
            data_dir: PathBuf::from("data"),
            log_format: LogFormat::Text,
            fetch: FetchSection::default(),
            scheduler: SchedulerSection::default(),
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
            .extract()
            .map_err(Box::new)
    }
}

#[cfg(test)]
// `Jail::expect_with` fixes the closure error type to the (large) `figment::Error`.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use figment::Jail;

    fn load(path: Option<&Path>) -> Result<Config, figment::Error> {
        Config::load(path).map_err(|err| *err)
    }

    #[test]
    fn defaults_when_nothing_is_set() {
        Jail::expect_with(|_jail| {
            assert_eq!(load(None)?, Config::default());
            Ok(())
        });
    }

    #[test]
    fn toml_overrides_defaults_and_env_overrides_toml() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "vigil.toml",
                r#"
                listen = "127.0.0.1:6000"
                data_dir = "/srv/vigil"
                log_format = "json"
                "#,
            )?;
            let config = load(None)?;
            assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 6000)));
            assert_eq!(config.data_dir, PathBuf::from("/srv/vigil"));
            assert_eq!(config.log_format, LogFormat::Json);

            jail.set_env("VIGIL_LISTEN", "127.0.0.1:7000");
            jail.set_env("VIGIL_DATA_DIR", "/tmp/elsewhere");
            let config = load(None)?;
            assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 7000)));
            assert_eq!(config.data_dir, PathBuf::from("/tmp/elsewhere"));
            assert_eq!(config.log_format, LogFormat::Json);
            Ok(())
        });
    }

    #[test]
    fn explicit_path_is_used() {
        Jail::expect_with(|jail| {
            jail.create_file("custom.toml", r#"data_dir = "from-custom""#)?;
            let config = load(Some(Path::new("custom.toml")))?;
            assert_eq!(config.data_dir, PathBuf::from("from-custom"));
            Ok(())
        });
    }

    #[test]
    fn vigil_config_env_var_is_not_a_config_key() {
        Jail::expect_with(|jail| {
            jail.set_env("VIGIL_CONFIG", "/some/where.toml");
            assert_eq!(load(None)?, Config::default());
            Ok(())
        });
    }

    #[test]
    fn nested_sections_come_from_toml_and_env() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "vigil.toml",
                r#"
                [fetch]
                timeout_secs = 5
                user_agent = "probe/1"

                [scheduler]
                concurrency = 3
                "#,
            )?;
            jail.set_env("VIGIL_FETCH__MAX_BODY_BYTES", "2048");
            jail.set_env("VIGIL_SCHEDULER__PER_HOST", "1");
            let config = load(None)?;
            assert_eq!(config.fetch.timeout_secs, 5);
            assert_eq!(config.fetch.max_body_bytes, 2048);
            assert_eq!(config.fetch.user_agent.as_deref(), Some("probe/1"));
            assert_eq!(config.scheduler.concurrency, 3);
            assert_eq!(config.scheduler.per_host, 1);
            Ok(())
        });
    }

    #[test]
    fn partial_sections_keep_the_other_defaults() {
        Jail::expect_with(|jail| {
            jail.set_env("VIGIL_FETCH__TIMEOUT_SECS", "9");
            let config = load(None)?;
            assert_eq!(config.fetch.timeout_secs, 9);
            assert_eq!(
                config.fetch.max_body_bytes,
                FetchSection::default().max_body_bytes
            );
            assert_eq!(config.scheduler, SchedulerSection::default());
            Ok(())
        });
    }

    #[test]
    fn derived_settings_follow_the_options() {
        let mut config = Config::default();
        config.fetch.timeout_secs = 7;
        config.scheduler.concurrency = 4;
        let fetch = config.fetch_config();
        assert_eq!(fetch.timeout, Duration::from_secs(7));
        assert!(fetch.user_agent.starts_with("vigil/"));
        config.fetch.user_agent = Some("custom".into());
        assert_eq!(config.fetch_config().user_agent, "custom");
        assert_eq!(config.scheduler_settings().concurrency, 4);
        assert_eq!(config.db_path(), PathBuf::from("data/vigil.db"));
    }

    #[test]
    fn invalid_values_are_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env("VIGIL_LISTEN", "not-an-address");
            assert!(load(None).is_err());
            Ok(())
        });
    }
}
