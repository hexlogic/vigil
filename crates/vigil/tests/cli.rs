//! End-to-end tests of the `vigil` binary against a mock web server.

mod common;

use common::*;
use vigil_store::Store;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[tokio::test(flavor = "multi_thread")]
async fn proxy_environment_variables_are_ignored() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, V1).await;
    let id = add_watch(dir.path(), &server.uri()).await?;

    // Nothing listens on port 9, so a fetch routed through this proxy would fail.
    let dead_proxy = "http://127.0.0.1:9";
    let envs = [
        ("HTTP_PROXY", dead_proxy),
        ("http_proxy", dead_proxy),
        ("ALL_PROXY", dead_proxy),
        ("all_proxy", dead_proxy),
    ];
    let run = vigil_with_env(dir.path(), &["check", &id], &envs).await?;
    assert!(run.ok, "check should bypass the proxy: {}", run.stderr);
    assert!(run.stdout.contains("baseline stored"), "{}", run.stdout);
    Ok(())
}

/// A page with a price and a banner that changes on every visit.
fn shop(banner: &str, price: &str) -> String {
    format!(
        "<html><body><div class=banner>{banner}</div><p>Widget</p>\
         <span id=price>{price}</span></body></html>"
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_add_takes_a_pipeline_and_conditions_and_shows_them_back() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, &shop("Welcome", "$10")).await;

    let rules = dir.path().join("rules.json");
    std::fs::write(
        &rules,
        r#"{"trigger_text": ["$12"], "notify_on": {"removed": false}}"#,
    )?;
    let rules_arg = format!("@{}", rules.display());
    let run = vigil(
        dir.path(),
        &[
            "watch",
            "add",
            &server.uri(),
            "--pipeline",
            r##"[{"type": "css", "selector": "#price"}, {"type": "trim"}]"##,
            "--conditions",
            &rules_arg,
        ],
    )
    .await?;
    assert!(run.ok, "{}", run.stderr);
    let id = run
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id")?
        .to_owned();

    let shown = vigil(dir.path(), &["watch", "show", &id]).await?;
    assert!(shown.ok, "{}", shown.stderr);
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(json["id"], id.as_str());
    assert_eq!(json["processor"], "text");
    assert_eq!(
        json["pipeline"],
        serde_json::json!([
            {"type": "css", "selector": "#price", "mode": "include"},
            {"type": "trim"}
        ])
    );
    assert_eq!(
        json["conditions"]["trigger_text"],
        serde_json::json!(["$12"])
    );
    assert_eq!(json["conditions"]["notify_on"]["removed"], false);

    // The pipeline is in effect: only the price is compared.
    assert!(vigil(dir.path(), &["check", &id]).await?.ok);
    server.reset().await;
    serve_page(&server, &shop("Autumn sale!", "$10")).await;
    let run = vigil(dir.path(), &["check", &id]).await?;
    assert!(run.stdout.contains("unchanged"), "{}", run.stdout);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_pipeline_or_condition_is_refused_before_anything_is_saved() -> TestResult {
    let dir = tempfile::tempdir()?;
    let url = "https://example.com";
    let cases: [(&[&str], &str); 5] = [
        (&["--pipeline", "not json"], "invalid --pipeline"),
        (
            &["--pipeline", r#"[{"type": "nope"}]"#],
            "unknown variant `nope`",
        ),
        (
            &["--pipeline", r#"[{"type": "css", "selector": "div["}]"#],
            "step 1 (css): invalid selector",
        ),
        (
            &["--conditions", r#"{"block_text": ["/(/"]}"#],
            "block_text[0]",
        ),
        (
            &[
                "--processor",
                "json",
                "--pipeline",
                r#"[{"type": "css", "selector": "p"}]"#,
            ],
            "html steps need the text processor",
        ),
    ];
    for (options, expected) in cases {
        let mut args = vec!["watch", "add", url];
        args.extend_from_slice(options);
        let run = vigil(dir.path(), &args).await?;
        assert!(!run.ok, "{options:?} was accepted");
        assert!(run.stderr.contains(expected), "{options:?}: {}", run.stderr);
    }
    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(list.stdout.contains("no watches yet"), "{}", list.stdout);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_edit_changes_the_pipeline_and_keeps_the_rest() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, &shop("Welcome", "$10")).await;
    let id = add_watch(dir.path(), &server.uri()).await?;
    assert!(vigil(dir.path(), &["check", &id]).await?.ok);

    let run = vigil(
        dir.path(),
        &[
            "watch",
            "edit",
            &id,
            "--pipeline",
            r##"[{"type": "css", "selector": "#price"}]"##,
        ],
    )
    .await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.starts_with(&format!("updated {id}")),
        "{}",
        run.stdout
    );

    let shown = vigil(dir.path(), &["watch", "show", &id]).await?;
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(json["pipeline"][0]["selector"], "#price");
    assert_eq!(json["schedule"], serde_json::json!({"interval": 300}));

    // The next check compares the new output with the old snapshot, so it reports a change
    // even though the page is the same.
    let run = vigil(dir.path(), &["check", &id]).await?;
    assert!(run.stdout.contains("changed"), "{}", run.stdout);

    let run = vigil(
        dir.path(),
        &["watch", "edit", &id, "--every", "2h", "--name", "Shop"],
    )
    .await?;
    assert!(run.ok, "{}", run.stderr);
    let list = vigil(dir.path(), &["watch", "list"]).await?;
    assert!(
        list.stdout.contains("2h") && list.stdout.contains("Shop"),
        "{}",
        list.stdout
    );
    let shown = vigil(dir.path(), &["watch", "show", &id]).await?;
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(json["pipeline"][0]["selector"], "#price");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_edit_refuses_nothing_to_change_and_invalid_results() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, V1).await;
    let id = add_watch(dir.path(), &server.uri()).await?;
    assert!(
        vigil(
            dir.path(),
            &[
                "watch",
                "edit",
                &id,
                "--pipeline",
                r#"[{"type": "css", "selector": "p"}]"#
            ]
        )
        .await?
        .ok
    );

    let run = vigil(dir.path(), &["watch", "edit", &id]).await?;
    assert!(!run.ok);
    assert!(run.stderr.contains("nothing to change"), "{}", run.stderr);

    // Changing the processor alone would leave steps that do not fit it.
    let run = vigil(dir.path(), &["watch", "edit", &id, "--processor", "json"]).await?;
    assert!(!run.ok);
    assert!(
        run.stderr.contains("html steps need the text processor"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("--pipeline ('[]' removes them)"),
        "{}",
        run.stderr
    );
    // Nothing was changed by the refused edit.
    let shown = vigil(dir.path(), &["watch", "show", &id]).await?;
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(json["processor"], "text");

    let run = vigil(
        dir.path(),
        &[
            "watch",
            "edit",
            &id,
            "--processor",
            "json",
            "--pipeline",
            "[]",
        ],
    )
    .await?;
    assert!(run.ok, "{}", run.stderr);

    let run = vigil(dir.path(), &["watch", "edit", "zzzzzz", "--name", "x"]).await?;
    assert!(!run.ok);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_shows_the_output_the_diff_and_the_verdict_and_saves_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, &shop("Welcome", "$10")).await;
    let id = add_watch(dir.path(), &server.uri()).await?;

    // Nothing to compare with yet.
    let run = vigil(dir.path(), &["preview", &id]).await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("--- output ---\nWelcome"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("--- no snapshot to compare with yet ---"),
        "{}",
        run.stdout
    );

    assert!(vigil(dir.path(), &["check", &id]).await?.ok);
    server.reset().await;
    serve_page(&server, &shop("Welcome", "$12")).await;

    let run = vigil(dir.path(), &["preview", &id]).await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(run.stdout.contains("-$10\n+$12\n"), "{}", run.stdout);
    assert!(
        run.stdout.ends_with("would notify (matched)\n"),
        "{}",
        run.stdout
    );

    // Overrides apply to this run only.
    let run = vigil(
        dir.path(),
        &[
            "preview",
            &id,
            "--pipeline",
            r##"[{"type": "css", "selector": "#price"}]"##,
            "--conditions",
            r#"{"block_text": ["$12"]}"#,
        ],
    )
    .await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("--- output ---\n$12\n"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .ends_with("would not notify (an added line matches block_text \"$12\")\n"),
        "{}",
        run.stdout
    );
    let shown = vigil(dir.path(), &["watch", "show", &id]).await?;
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(json["pipeline"], serde_json::json!([]));

    // Still one snapshot, and the stored state is untouched.
    let store = Store::open(&dir.path().join("data").join("vigil.db")).await?;
    let watch = store.get_watch(id.parse()?).await?;
    assert_eq!(store.list_snapshots(watch.id, 10).await?.len(), 1);
    assert_eq!(watch.state.consecutive_errors, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_fails_with_a_message_when_the_settings_or_the_page_do_not_work() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, V1).await;
    let id = add_watch(dir.path(), &server.uri()).await?;

    let run = vigil(
        dir.path(),
        &[
            "preview",
            &id,
            "--pipeline",
            r#"[{"type": "css", "selector": ".none", "required": true}]"#,
        ],
    )
    .await?;
    assert!(!run.ok);
    assert!(run.stderr.contains("preview failed"), "{}", run.stderr);
    assert!(
        run.stderr.contains("step 1 (css): matched nothing"),
        "{}",
        run.stderr
    );

    // Refused before fetching: the override itself is invalid.
    let run = vigil(dir.path(), &["preview", &id, "--pipeline", "nope"]).await?;
    assert!(!run.ok);
    assert!(run.stderr.contains("invalid --pipeline"), "{}", run.stderr);

    let run = vigil(dir.path(), &["preview", "zzzzzz"]).await?;
    assert!(!run.ok);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn check_says_when_conditions_hold_a_change_back() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = MockServer::start().await;
    serve_page(&server, V1).await;
    let run = vigil(
        dir.path(),
        &[
            "watch",
            "add",
            &server.uri(),
            "--conditions",
            r#"{"trigger_text": ["gadget"]}"#,
        ],
    )
    .await?;
    let id = run
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id")?
        .to_owned();
    assert!(vigil(dir.path(), &["check", &id]).await?.ok);

    server.reset().await;
    serve_page(&server, V2).await;
    let run = vigil(dir.path(), &["check", &id]).await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("changed, +1 -1 lines")
            && run
                .stdout
                .contains("no notification: no added line matches trigger_text"),
        "{}",
        run.stdout
    );
    Ok(())
}
