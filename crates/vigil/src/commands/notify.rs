//! `vigil notify add|list|remove|test`.

use std::process::ExitCode;

use anyhow::{Context, ensure};
use vigil_notify::{Channel, RetryPolicy, Sender, SenderConfig, validate_template};
use vigil_scheduler::Notifier;
use vigil_store::{NewTarget, NotificationTarget, Store};

use super::open_store;
use crate::config::Config;
use crate::format::render_table;

/// A notifier that sends with the configured settings and records in `store`.
pub fn notifier(config: &Config, store: &Store) -> anyhow::Result<Notifier> {
    let sender =
        Sender::new(SenderConfig::default()).context("building the notification client")?;
    Ok(Notifier::new(store.clone(), sender).with_base_url(config.base_url.clone()))
}

/// Text given on the command line, or `@path` to a file that holds it.
pub fn read_text(flag: &str, value: &str) -> anyhow::Result<String> {
    match value.strip_prefix('@') {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {path} for --{flag}"))
        }
        None => Ok(value.to_owned()),
    }
}

/// Find each named target, failing on the first that does not exist.
pub async fn resolve_all(
    store: &Store,
    names: &[String],
) -> anyhow::Result<Vec<NotificationTarget>> {
    let mut targets = Vec::new();
    for name in names {
        targets.push(store.resolve_target(name).await?);
    }
    Ok(targets)
}

/// Check and store a new target.
pub async fn add(
    config: &Config,
    name: &str,
    url: &str,
    template: Option<&str>,
) -> anyhow::Result<()> {
    let name = name.trim();
    ensure!(!name.is_empty(), "the target needs a name");
    let channel = Channel::parse(url).context("invalid notification URL")?;
    let template = template
        .map(|text| read_text("template", text))
        .transpose()?;
    if let Some(template) = &template {
        validate_template(template).context("invalid --template")?;
    }

    let store = open_store(config).await?;
    let target = store
        .create_target(&NewTarget {
            name: name.to_owned(),
            url: url.trim().to_owned(),
            default_template: template,
        })
        .await?;
    println!(
        "added {} {} ({})",
        target.id,
        target.name,
        channel.redacted()
    );
    if channel.sends_secrets_in_cleartext() {
        eprintln!(
            "warning: this URL sends a secret (a password, a token, a topic or a webhook path) \
             over plain HTTP; use the https form (ntfys://, jsons://, forms://, gotifys://) \
             if the server supports it"
        );
    }
    Ok(())
}

/// Print the targets with their credentials hidden.
pub async fn list(config: &Config) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let targets = store.list_targets().await?;
    if targets.is_empty() {
        println!("no notification targets yet; add one with `vigil notify add <name> <url>`");
        return Ok(());
    }
    let linked = store.linked_watch_counts().await?;
    let mut rows = vec![
        ["ID", "NAME", "KIND", "URL", "STATE", "WATCHES"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for target in &targets {
        let (kind, url) = match Channel::parse(&target.url) {
            Ok(channel) => (channel.kind().to_owned(), channel.redacted()),
            Err(_) => (
                "invalid".to_owned(),
                "(not a valid notification URL)".to_owned(),
            ),
        };
        rows.push(vec![
            target.id.to_string(),
            target.name.clone(),
            kind,
            url,
            if target.enabled {
                "enabled"
            } else {
                "disabled"
            }
            .to_owned(),
            linked.get(&target.id).copied().unwrap_or(0).to_string(),
        ]);
    }
    print!("{}", render_table(&rows));
    Ok(())
}

/// Delete a target.
pub async fn remove(config: &Config, target: &str) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let target = store.resolve_target(target).await?;
    let linked = store.linked_watch_counts().await?;
    store.delete_target(target.id).await?;
    println!("removed {} {}", target.id, target.name);
    if let Some(count) = linked.get(&target.id) {
        eprintln!("{count} watch(es) no longer notify it");
    }
    Ok(())
}

/// Send a test notification and say whether it arrived. There are no retries: someone is
/// waiting at the terminal to be told what is wrong.
pub async fn test(config: &Config, target: &str) -> anyhow::Result<ExitCode> {
    let store = open_store(config).await?;
    let target = store.resolve_target(target).await?;
    let retry = RetryPolicy {
        delays: Vec::new(),
        ..RetryPolicy::default()
    };
    let report = notifier(config, &store)?
        .with_retry(retry)
        .test(&target)
        .await;
    if let Some(note) = &report.report.note {
        eprintln!("note: {note}");
    }
    match report.report.result {
        Ok(()) => {
            println!("sent a test notification to {}", target.name);
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("could not notify {}: {error}", target.name);
            Ok(ExitCode::FAILURE)
        }
    }
}
