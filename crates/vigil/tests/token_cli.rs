//! `vigil token`: tokens for the API, made and revoked from the command line.

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_printed_once_listed_without_its_value_and_revoked() -> TestResult {
    let dir = tempfile::tempdir()?;

    let empty = vigil(dir.path(), &["token", "list"]).await?;
    assert!(empty.ok, "{}", empty.stderr);
    assert!(
        empty.stdout.contains("no API tokens yet"),
        "{}",
        empty.stdout
    );

    let made = vigil(dir.path(), &["token", "add", "backup script"]).await?;
    assert!(made.ok, "{}", made.stderr);
    // The token is all of stdout, so a script can capture it.
    let token = made.stdout.trim();
    assert!(
        token.starts_with("vigil_") && token.len() == 70 && !token.contains(char::is_whitespace),
        "{token:?}"
    );
    assert!(made.stderr.contains("not shown again"), "{}", made.stderr);
    assert!(!made.stderr.contains(token));

    let listed = vigil(dir.path(), &["token", "list"]).await?;
    assert!(listed.ok, "{}", listed.stderr);
    assert!(listed.stdout.contains("backup script"), "{}", listed.stdout);
    assert!(listed.stdout.contains(&format!("{}…", &token[..10])));
    assert!(
        !listed.stdout.contains(token),
        "the list must not print the token"
    );
    assert!(listed.stdout.contains("never"), "{}", listed.stdout);

    let again = vigil(dir.path(), &["token", "add", "backup script"]).await?;
    assert!(!again.ok);
    assert!(again.stderr.contains("already exists"), "{}", again.stderr);

    let removed = vigil(dir.path(), &["token", "remove", "backup script"]).await?;
    assert!(removed.ok, "{}", removed.stderr);
    assert!(removed.stdout.contains("revoked"), "{}", removed.stdout);
    let after = vigil(dir.path(), &["token", "list"]).await?;
    assert!(after.stdout.contains("no API tokens yet"));
    let missing = vigil(dir.path(), &["token", "remove", "backup script"]).await?;
    assert!(!missing.ok);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_needs_a_name_that_is_not_empty_or_long() -> TestResult {
    let dir = tempfile::tempdir()?;
    let empty = vigil(dir.path(), &["token", "add", "  "]).await?;
    assert!(!empty.ok);
    assert!(empty.stderr.contains("needs a name"), "{}", empty.stderr);
    let long = "x".repeat(51);
    let too_long = vigil(dir.path(), &["token", "add", &long]).await?;
    assert!(!too_long.ok);
    assert!(
        too_long.stderr.contains("at most 50"),
        "{}",
        too_long.stderr
    );
    Ok(())
}
