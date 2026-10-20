//! Configuration: built-in defaults < TOML file < `VIGIL_*` environment variables.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

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

/// Runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    /// Directory holding the SQLite database.
    pub data_dir: PathBuf,
    /// Log output format.
    pub log_format: LogFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 5800)),
            data_dir: PathBuf::from("data"),
            log_format: LogFormat::Text,
        }
    }
}

impl Config {
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
    fn invalid_values_are_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env("VIGIL_LISTEN", "not-an-address");
            assert!(load(None).is_err());
            Ok(())
        });
    }
}
