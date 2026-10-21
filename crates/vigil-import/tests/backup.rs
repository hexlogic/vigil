//! Reading backups: the two layouts, and archives that are not what they claim to be.

use std::io::Write;

use vigil_import::{Existing, Format, ImportError, Options, plan};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ID: &str = "11111111-2222-4333-8444-555555555555";

fn zip_of(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        writer.start_file(name.as_str(), zip::write::SimpleFileOptions::default())?;
        writer.write_all(bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn read(bytes: &[u8]) -> Result<vigil_import::ImportPlan, ImportError> {
    plan(bytes, None, &Options::default(), &Existing::default())
}

fn watch(url: &str) -> Vec<u8> {
    format!(r#"{{"uuid": "{ID}", "url": "{url}", "title": "Shop"}}"#).into_bytes()
}

#[test]
fn the_older_single_file_holds_every_watch_and_the_tags() -> TestResult {
    let old = include_bytes!("fixtures/backup/old-datastore.json");
    let plan = read(old)?;
    assert_eq!(plan.report.format, Format::Backup);
    assert_eq!(plan.report.totals.found, 2);
    assert!(
        plan.report
            .notes
            .iter()
            .any(|n| n.message.contains("older single-file datastore"))
    );

    let single = serde_json::json!({
        "watching": {ID: {"url": "https://a.example/", "title": "A", "tags": ["t1"]}},
        "settings": {"application": {"tags": {"t1": {"title": "Shops"}}}}
    });
    let plan = read(single.to_string().as_bytes())?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].tags, ["Shops"]);
    assert_eq!(plan.batch.tags.len(), 1);
    Ok(())
}

#[test]
fn only_a_folder_named_like_an_id_holds_a_watch() -> TestResult {
    let archive = zip_of(&[
        (format!("{ID}/watch.json"), watch("https://a.example/")),
        // None of these is read, whatever they hold.
        (
            "../escape/watch.json".to_owned(),
            watch("https://evil.example/"),
        ),
        (
            "not-an-id/watch.json".to_owned(),
            watch("https://evil.example/"),
        ),
        (
            format!("{ID}/nested/watch.json"),
            watch("https://evil.example/"),
        ),
        (format!("{ID}/history.txt"), b"1,abc.txt".to_vec()),
    ])?;
    let plan = read(&archive)?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].settings.url, "https://a.example/");
    Ok(())
}

#[test]
fn an_archive_with_nothing_to_import_is_an_error() -> TestResult {
    for archive in [
        zip_of(&[("readme.txt".to_owned(), b"hello".to_vec())])?,
        b"PK\x03\x04this is not really a zip".to_vec(),
    ] {
        assert!(
            matches!(
                read(&archive),
                Err(ImportError::Invalid {
                    format: Format::Backup,
                    ..
                })
            ),
            "{:?}",
            read(&archive).err()
        );
    }
    // An archive with no entries at all does not even start like one.
    assert!(matches!(
        read(&zip_of(&[])?),
        Err(ImportError::UnknownFormat)
    ));
    Ok(())
}

#[test]
fn a_file_that_is_too_large_is_refused_whatever_it_claims() -> TestResult {
    // 11 MiB of text that compresses to almost nothing: a small archive, a large file.
    let mut big = br#"{"url": "https://a.example/", "pad": ""#.to_vec();
    big.extend(std::iter::repeat_n(b'x', 11 * 1024 * 1024));
    big.extend_from_slice(br#""}"#);
    let archive = zip_of(&[(format!("{ID}/watch.json"), big)])?;
    assert!(
        archive.len() < 100_000,
        "the archive is {} bytes",
        archive.len()
    );
    let error = read(&archive)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("too large"), "{error}");
    Ok(())
}

#[test]
fn a_watch_file_that_is_not_an_object_is_reported_and_the_rest_imported() -> TestResult {
    let other = "22222222-2222-4333-8444-555555555555";
    let archive = zip_of(&[
        (format!("{ID}/watch.json"), b"[1, 2]".to_vec()),
        (format!("{other}/watch.json"), watch("https://b.example/")),
    ])?;
    let plan = read(&archive)?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.report.totals.skipped, 1);
    assert!(plan.report.items.iter().any(|i| {
        i.notes
            .iter()
            .any(|n| n.message.contains("not a JSON object"))
    }));
    Ok(())
}
