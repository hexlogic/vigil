//! End-to-end tests of the `vigil` binary against a mock web server.

use std::path::Path;
use std::process::Output;

use vigil_store::Store;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const V1: &str =
    "<html><body><h1>Price list</h1><p>Widget: $10</p><p>Gadget: $20</p></body></html>";
const V2: &str =
    "<html><body><h1>Price list</h1><p>Widget: $12</p><p>Gadget: $20</p></body></html>";

/// Result of one `vigil` invocation.
struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
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

/// Run the binary with its data in `dir`, without blocking the async runtime.
async fn vigil(dir: &Path, args: &[&str]) -> Result<Run, Box<dyn std::error::Error>> {
    let dir = dir.to_path_buf();
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_vigil"))
            .args(&args)
            .current_dir(&dir)
            .env("VIGIL_DATA_DIR", dir.join("data"))
            .env_remove("VIGIL_CONFIG")
            .env_remove("RUST_LOG")
            .env_remove("NO_COLOR")
            .output()
    })
    .await??;
    Ok(output.into())
}

async fn serve_page(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(server)
        .await;
}

/// Add a watch and return its full id, taken from the `added <id> ...` line.
async fn add_watch(dir: &Path, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let run = vigil(dir, &["watch", "add", url, "--every", "5m"]).await?;
    assert!(run.ok, "watch add failed: {}", run.stderr);
    let id = run
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id in output")?;
    Ok(id.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_check_and_diff_follow_a_page_through_two_versions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, V1).await;
    let id = add_watch(dir.path(), &server.uri()).await?;
    // Commands accept the tail of the id.
    let short = &id[id.len() - 8..];

    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(list.ok, "{}", list.stderr);
    assert!(list.stdout.contains(&id), "{}", list.stdout);
    assert!(list.stdout.contains("5m"), "{}", list.stdout);
    assert!(list.stdout.contains("pending"), "{}", list.stdout);

    let before = vigil(dir.path(), &["diff", short]).await?;
    assert!(before.ok);
    assert!(
        before.stdout.contains("nothing to compare yet"),
        "{}",
        before.stdout
    );

    let first = vigil(dir.path(), &["check", short]).await?;
    assert!(first.ok, "{}", first.stderr);
    assert!(first.stdout.contains("baseline stored"), "{}", first.stdout);

    let second = vigil(dir.path(), &["check", short]).await?;
    assert!(second.ok, "{}", second.stderr);
    assert!(second.stdout.contains("unchanged"), "{}", second.stdout);

    server.reset().await;
    serve_page(&server, V2).await;
    let third = vigil(dir.path(), &["check", short]).await?;
    assert!(third.ok, "{}", third.stderr);
    assert!(
        third.stdout.contains("changed, +1 -1 lines"),
        "{}",
        third.stdout
    );

    let diff = vigil(dir.path(), &["diff", short]).await?;
    assert!(diff.ok, "{}", diff.stderr);
    assert!(
        diff.stdout.contains("-Widget: $10\n+Widget: $12\n"),
        "{}",
        diff.stdout
    );
    assert!(diff.stdout.contains("@@ "), "{}", diff.stdout);
    assert!(
        !diff.stdout.contains('\x1b'),
        "piped output must not be colored"
    );

    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(list.stdout.contains("ok"), "{}", list.stdout);
    assert!(!list.stdout.contains("pending"), "{}", list.stdout);

    // Exactly two snapshots made it into the database.
    let store = Store::open(&dir.path().join("data").join("vigil.db")).await?;
    let watch_id = store.resolve_watch_id(&id).await?;
    assert_eq!(store.list_snapshots(watch_id, 10).await?.len(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_page_makes_check_exit_non_zero() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let id = add_watch(dir.path(), &server.uri()).await?;

    let run = vigil(dir.path(), &["check", &id]).await?;
    assert!(!run.ok);
    assert!(
        run.stderr.contains("server answered http 500"),
        "{}",
        run.stderr
    );

    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(list.stdout.contains("error x1"), "{}", list.stdout);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_deletes_the_watch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let id = add_watch(dir.path(), "https://example.com/").await?;

    let removed = vigil(dir.path(), &["watch", "remove", &id[id.len() - 6..]]).await?;
    assert!(removed.ok, "{}", removed.stderr);
    assert!(removed.stdout.contains("removed"), "{}", removed.stdout);

    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(list.stdout.contains("no watches yet"), "{}", list.stdout);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_input_is_rejected_with_a_message() -> TestResult {
    let dir = tempfile::tempdir()?;

    let scheme = vigil(dir.path(), &["watch", "add", "ftp://example.com/file"]).await?;
    assert!(!scheme.ok);
    assert!(scheme.stderr.contains("http"), "{}", scheme.stderr);

    let not_a_url = vigil(dir.path(), &["watch", "add", "example.com"]).await?;
    assert!(!not_a_url.ok);

    let zero = vigil(
        dir.path(),
        &["watch", "add", "https://example.com", "--every", "0s"],
    )
    .await?;
    assert!(!zero.ok);
    assert!(zero.stderr.contains("at least 1s"), "{}", zero.stderr);

    let unknown = vigil(dir.path(), &["check", "abcdef12"]).await?;
    assert!(!unknown.ok);
    assert!(unknown.stderr.contains("not found"), "{}", unknown.stderr);

    let short = vigil(dir.path(), &["check", "abc"]).await?;
    assert!(!short.ok);
    assert!(short.stderr.contains("not a watch id"), "{}", short.stderr);
    Ok(())
}
