//! `vigil serve` sets up the web UI password and says so in its log (which goes to stdout).

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use vigil_store::Store;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A running `vigil serve` whose log lines can be read.
struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(
    dir: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<Server, std::io::Error> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vigil"));
    command
        .arg("serve")
        .args(args)
        .env("VIGIL_DATA_DIR", dir.join("data"))
        .env("VIGIL_LISTEN", "127.0.0.1:0")
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .env_remove("VIGIL_CONFIG")
        .env_remove("VIGIL_BASE_URL")
        .env_remove("VIGIL_ADMIN_PASSWORD")
        .env_remove("VIGIL_NO_AUTH")
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn()?;
    let log = child.stdout.take().ok_or(std::io::ErrorKind::BrokenPipe)?;
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(log).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    Ok(Server { child, lines })
}

impl Server {
    /// The first log line that contains `needle`, waiting up to 20 seconds for it.
    fn line_with(&self, needle: &str) -> Option<String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.checked_duration_since(std::time::Instant::now())?;
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return Some(line),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}

#[test]
fn the_first_start_makes_up_a_password_and_shows_it_once() -> TestResult {
    let dir = tempfile::tempdir()?;

    let first = start(dir.path(), &[], &[])?;
    let line = first
        .line_with("this one was made up")
        .ok_or("no password line")?;
    let password = line
        .split("password=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(|| format!("no password in {line:?}"))?
        .to_owned();
    assert!(password.len() >= 20, "{password}");
    drop(first);

    // The database holds only a hash of it.
    let hash = tokio_block(async {
        let store = Store::open(&dir.path().join("data").join("vigil.db")).await?;
        Ok::<_, Box<dyn std::error::Error>>(store.get_setting("admin_password_hash").await?)
    })?
    .ok_or("no hash stored")?;
    assert!(hash.starts_with("$argon2id$") && !hash.contains(&password));

    // The second start keeps it, and does not show a password again.
    let second = start(dir.path(), &[], &[])?;
    second
        .line_with("the one saved earlier")
        .ok_or("the second start did not keep the password")?;
    Ok(())
}

#[test]
fn a_password_in_the_environment_is_used_and_never_printed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = start(
        dir.path(),
        &[],
        &[("VIGIL_ADMIN_PASSWORD", "s3cret-from-the-env")],
    )?;
    let said = server
        .line_with("the one in VIGIL_ADMIN_PASSWORD")
        .ok_or("the start did not say where the password is from")?;
    assert!(!said.contains("s3cret"), "{said}");
    // Whatever else the start logs, the password is not in it.
    let mut rest = Vec::new();
    while let Ok(line) = server.lines.recv_timeout(Duration::from_secs(2)) {
        rest.push(line);
    }
    assert!(rest.iter().all(|line| !line.contains("s3cret")), "{rest:?}");
    Ok(())
}

#[test]
fn no_auth_is_announced_loudly() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = start(dir.path(), &["--no-auth"], &[])?;
    let line = server.line_with("SIGN-IN IS OFF").ok_or("no warning")?;
    assert!(
        line.contains("WARN") && line.contains("127.0.0.1"),
        "{line}"
    );
    assert!(
        !line.contains("loopback"),
        "a loopback-only server is not shouted at: {line}"
    );
    Ok(())
}

#[test]
fn no_auth_on_a_public_address_is_warned_about_more() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = start(
        dir.path(),
        &[],
        &[("VIGIL_NO_AUTH", "true"), ("VIGIL_LISTEN", "0.0.0.0:0")],
    )?;
    let line = server.line_with("SIGN-IN IS OFF").ok_or("no warning")?;
    assert!(
        line.contains("not listening on a loopback address only"),
        "{line}"
    );
    Ok(())
}

fn tokio_block<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_or_else(
            |error| unreachable!("a runtime: {error}"),
            |rt| rt.block_on(future),
        )
}
