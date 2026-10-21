use super::*;
use figment::Jail;

fn load(path: Option<&Path>) -> Result<Config, figment::Error> {
    Config::load(path).map_err(|err| *err)
}

#[test]
fn base_url_comes_from_the_file_or_the_environment() {
    Jail::expect_with(|jail| {
        assert_eq!(load(None)?.base_url, None);
        jail.create_file("vigil.toml", "base_url = \"https://vigil.example.com\"")?;
        assert_eq!(
            load(None)?.base_url.as_deref(),
            Some("https://vigil.example.com")
        );
        jail.set_env("VIGIL_BASE_URL", "http://nas.lan:5800/vigil");
        assert_eq!(
            load(None)?.base_url.as_deref(),
            Some("http://nas.lan:5800/vigil")
        );
        Ok(())
    });
}

#[test]
fn the_password_and_no_auth_come_from_the_environment_and_stay_out_of_logs() {
    Jail::expect_with(|jail| {
        let config = load(None)?;
        assert_eq!(
            (config.admin_password.clone(), config.no_auth),
            (None, false)
        );

        jail.set_env("VIGIL_ADMIN_PASSWORD", "hunter2 and more");
        jail.set_env("VIGIL_NO_AUTH", "true");
        let config = load(None)?;
        assert_eq!(
            config.admin_password.as_ref().map(Secret::expose),
            Some("hunter2 and more")
        );
        assert!(config.no_auth);
        let printed = format!("{config:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("Secret(***)"), "{printed}");
        Ok(())
    });
}

#[test]
fn a_base_url_that_is_not_http_is_refused() {
    Jail::expect_with(|jail| {
        for bad in ["vigil.example.com", "ftp://example.com", "file:///tmp"] {
            jail.set_env("VIGIL_BASE_URL", bad);
            let error = load(None).expect_err("refused").to_string();
            assert!(error.contains("base_url"), "{bad}: {error}");
        }
        for bad in [
            "https://vigil.example.com?x=1",
            "https://vigil.example.com/vigil?x=1",
            "https://user:pw@vigil.example.com",
            "https://vigil.example.com/#top",
        ] {
            jail.set_env("VIGIL_BASE_URL", bad);
            let error = load(None).expect_err("refused").to_string();
            assert!(error.contains("base_url: must not"), "{bad}: {error}");
        }
        // A path is fine (behind a reverse proxy), and an @ in it is not a login.
        jail.set_env("VIGIL_BASE_URL", "https://example.com/vigil@home/");
        assert!(load(None).is_ok());
        // Blank means unset.
        jail.set_env("VIGIL_BASE_URL", "");
        assert!(load(None).is_ok());
        Ok(())
    });
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
fn the_import_limit_comes_from_toml_and_env() {
    Jail::expect_with(|jail| {
        assert_eq!(load(None)?.import.max_upload_bytes, 256 * 1024 * 1024);
        jail.create_file("vigil.toml", "[import]\nmax_upload_bytes = 1000\n")?;
        assert_eq!(load(None)?.import.max_upload_bytes, 1000);
        jail.set_env("VIGIL_IMPORT__MAX_UPLOAD_BYTES", "2000");
        assert_eq!(load(None)?.import.max_upload_bytes, 2000);
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
