//! The Phase 5 acceptance test: a backup of another change monitor, written for these tests,
//! with 22 varied watches, is imported with `vigil import`. The watches that come out are
//! equivalent to what went in, the report says what was left out or changed, and a few of
//! them are run against a mock server to show that their filters give the expected text.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use common::*;
use serde_json::Value;
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP: &str = "<html><body><div id=box><div class=ad>BUY NOW</div>\
    <p id=price>Price: 42</p><p id=stock>In stock</p></div><footer>(c) shop</footer></body></html>";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../vigil-import/tests/fixtures/backup")
}

/// Every file of the fixture tree, as (path inside the archive, text).
fn files(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            files(&entry.path(), &format!("{name}/"), out)?;
        } else {
            out.push((name, std::fs::read_to_string(entry.path())?));
        }
    }
    Ok(())
}

/// The fixture as a ZIP, with the address of the mock server filled in.
fn zip_of_fixture(server: &str, to: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut all = Vec::new();
    files(&fixture_dir(), "", &mut all)?;
    let mut writer = zip::ZipWriter::new(std::fs::File::create(to)?);
    for (name, text) in all {
        writer.start_file(name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(text.replace("{{SERVER}}", server).as_bytes())?;
    }
    writer.finish()?;
    Ok(())
}

async fn mock() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/shop"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SHOP, "text/html; charset=utf-8"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"items": [{"name": "a"}, {"name": "b"}]}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    // The headers and the body are the watch's own, and the global ones: the settings of the
    // whole backup (a language, a User-Agent) reach the request.
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("content-type", "application/json"))
        .and(header("x-token", "FIXTURE-TOKEN"))
        .and(header("accept-language", "uk"))
        .and(header("user-agent", "FixtureBrowser/1.0"))
        .and(body_string(r#"{"q": "widgets"}"#))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"hits": [{"title": "Widget", "price": 9}, {"title": "Gadget", "price": 12}]}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;
    server
}

/// The settings of every imported watch, without ids and with the mock server's address
/// replaced, ordered by name.
async fn watches_as_imported(
    dir: &Path,
    server: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let list = vigil(dir, &["watch", "list"]).await?;
    assert!(list.ok, "{}", list.stderr);
    let mut out = Vec::new();
    for line in list.stdout.lines().skip(1) {
        let id = line
            .split_whitespace()
            .next()
            .ok_or("a row without an id")?;
        let shown = vigil(dir, &["watch", "show", id]).await?;
        assert!(shown.ok, "{}", shown.stderr);
        let mut json: Value = serde_json::from_str(&shown.stdout.replace(server, "{{SERVER}}"))?;
        json["id"] = Value::from("[id]");
        out.push(json);
    }
    out.sort_by_key(|w| w["name"].as_str().unwrap_or_default().to_owned());
    Ok(out)
}

async fn text_of(dir: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let list = vigil(dir, &["watch", "list"]).await?;
    let id = list
        .stdout
        .lines()
        .find(|line| line.contains(name))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| format!("no watch named {name}"))?
        .to_owned();
    let run = vigil(dir, &["preview", &id]).await?;
    assert!(
        run.ok,
        "preview of {name} failed: {}{}",
        run.stdout, run.stderr
    );
    let out = run
        .stdout
        .split("--- output ---\n")
        .nth(1)
        .ok_or("no output section")?;
    Ok(out.split("\n---").next().unwrap_or_default().to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_of_22_varied_watches_becomes_equivalent_watches_and_a_report() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = mock().await;
    let archive = dir.path().join("backup.zip");
    zip_of_fixture(&server.uri(), &archive)?;
    let archive_arg = archive.to_string_lossy().into_owned();

    // The preview says what would happen and changes nothing.
    let dry = vigil(dir.path(), &["import", &archive_arg, "--dry-run"]).await?;
    assert!(dry.ok, "{}", dry.stderr);
    assert!(dry.stdout.contains("dry run: nothing was changed"));
    let listed = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(
        listed.stdout.contains("no watches yet"),
        "{}",
        listed.stdout
    );
    for secret in [
        "FIXTURE-TOKEN",
        "FIXTUREBOT",
        "fixture-topic",
        "FixtureBrowser",
        "hooks.example",
    ] {
        assert!(!dry.stdout.contains(secret), "the report repeats {secret}");
    }
    let report = dry.stdout.replace(&server.uri(), "{{SERVER}}");
    insta::assert_snapshot!("report", report);

    // The import itself.
    let made = vigil(dir.path(), &["import", &archive_arg]).await?;
    assert!(made.ok, "{}", made.stderr);
    assert!(made.stdout.contains("made 21 watch(es)"), "{}", made.stdout);

    let watches = watches_as_imported(dir.path(), &server.uri()).await?;
    assert_eq!(watches.len(), 21);
    let names: Vec<&str> = watches.iter().filter_map(|w| w["name"].as_str()).collect();
    assert!(
        !names.iter().any(|n| n.starts_with("Stale")),
        "a stale copy was read: {names:?}"
    );
    assert!(!names.contains(&"Image comparison"));
    insta::assert_json_snapshot!("watches", watches);

    // Importing the same file again makes nothing: every address is watched already.
    let again = vigil(dir.path(), &["import", &archive_arg]).await?;
    assert!(
        again.ok && again.stdout.contains("made 0 watch(es)"),
        "{}",
        again.stdout
    );
    assert_eq!(
        watches_as_imported(dir.path(), &server.uri()).await?.len(),
        21
    );

    // The filters do what the source's did, run against a server.
    assert_eq!(
        text_of(dir.path(), "Price (css)").await?.trim(),
        "Price: 42"
    );
    assert_eq!(
        text_of(dir.path(), "Price without ads").await?.trim(),
        "Price: 42\n\nIn stock"
    );
    assert_eq!(text_of(dir.path(), "Extract price").await?.trim(), "42");
    let listing = text_of(dir.path(), "Listing (jq)").await?;
    assert!(
        listing.contains("\"a\"") && listing.contains("\"b\""),
        "{listing}"
    );
    assert_eq!(
        text_of(dir.path(), "Search (post, jqraw)").await?.trim(),
        "Widget | 9\nGadget | 12"
    );
    Ok(())
}
