//! Helpers shared by the end-to-end tests of the `vigil` binary.

#![allow(dead_code)]

use std::path::Path;
use std::process::Output;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub const V1: &str =
    "<html><body><h1>Price list</h1><p>Widget: $10</p><p>Gadget: $20</p></body></html>";
pub const V2: &str =
    "<html><body><h1>Price list</h1><p>Widget: $12</p><p>Gadget: $20</p></body></html>";

/// Result of one `vigil` invocation.
pub struct Run {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for Run {
    fn from(output: Output) -> Self {
        Self {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Proxy variables a developer machine or CI runner might have set.
const PROXY_VARS: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// Run the binary with its data in `dir`, without blocking the async runtime.
pub async fn vigil(dir: &Path, args: &[&str]) -> Result<Run, Box<dyn std::error::Error>> {
    vigil_with_env(dir, args, &[]).await
}

/// Like [`vigil`], with extra environment variables set.
pub async fn vigil_with_env(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Run, Box<dyn std::error::Error>> {
    let dir = dir.to_path_buf();
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let envs: Vec<(String, String)> = envs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_vigil"));
        command
            .args(&args)
            .current_dir(&dir)
            .env("VIGIL_DATA_DIR", dir.join("data"))
            .env_remove("VIGIL_CONFIG")
            .env_remove("RUST_LOG")
            .env_remove("NO_COLOR");
        for var in PROXY_VARS {
            command.env_remove(var);
        }
        command.envs(envs);
        command.output()
    })
    .await??;
    Ok(output.into())
}

pub async fn serve_page(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(server)
        .await;
}

/// Add a watch and return its full id, taken from the `added <id> ...` line.
pub async fn add_watch(dir: &Path, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let run = vigil(dir, &["watch", "add", url, "--every", "5m"]).await?;
    assert!(run.ok, "watch add failed: {}", run.stderr);
    let id = run
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id in output")?;
    Ok(id.to_owned())
}
