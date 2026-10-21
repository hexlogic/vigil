//! `vigil token add|list|remove`: API tokens, for setups without a browser.

use anyhow::{Context, bail};
use vigil_store::now_ms;

use super::open_store;
use crate::config::Config;
use crate::format::{ago, render_table};

/// Longest name a token can have, as on the settings page.
const NAME_LEN: usize = 50;

/// Make a token. The token goes to stdout alone, so `TOKEN=$(vigil token add ci)` works; the
/// rest goes to stderr.
pub async fn add(config: &Config, name: &str) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("a token needs a name");
    }
    if name.chars().count() > NAME_LEN {
        bail!("the name can have at most {NAME_LEN} characters");
    }
    let made = vigil_web::generate_token().context("making a random token")?;
    let store = open_store(config).await?;
    let saved = store
        .create_api_token(name, &made.hash, &made.prefix)
        .await?;
    println!("{}", made.token);
    eprintln!(
        "made API token {} {}; copy it now, it is not shown again",
        saved.id, saved.name
    );
    Ok(())
}

/// Print the tokens: never the tokens themselves, only how to tell them apart.
pub async fn list(config: &Config) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let tokens = store.list_api_tokens().await?;
    if tokens.is_empty() {
        println!("no API tokens yet; make one with `vigil token add <name>`");
        return Ok(());
    }
    let now = now_ms();
    let mut rows = vec![
        ["ID", "NAME", "TOKEN", "CREATED", "LAST USED"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for token in &tokens {
        rows.push(vec![
            token.id.to_string(),
            token.name.clone(),
            format!("{}…", token.prefix),
            ago(Some(token.created_at), now),
            ago(token.last_used_at, now),
        ]);
    }
    print!("{}", render_table(&rows));
    Ok(())
}

/// Revoke a token.
pub async fn remove(config: &Config, token: &str) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let token = store.resolve_api_token(token).await?;
    store.delete_api_token(token.id).await?;
    println!("revoked {} {}", token.id, token.name);
    Ok(())
}
