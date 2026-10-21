//! Applying an import: tags, targets and watches made in one transaction.
//!
//! The batch names tags by name and targets by address, not by id, and they are looked up again
//! inside the transaction. A plan made a quarter of an hour ago (the web page keeps one that
//! long between the preview and the button) may meet tags and targets that have since been
//! made or deleted.

use std::collections::{HashMap, HashSet};

use sqlx::{Row, SqliteConnection};
use uuid::Uuid;
use vigil_core::WatchId;

use crate::tags::{NewTag, insert_tag};
use crate::targets::{NewTarget, insert_target};
use crate::watches::{NewWatch, insert_watch};
use crate::{Store, StoreError, now_ms};

/// Colour of a tag an import makes.
const NEW_TAG_COLOR: &str = "gray";

/// A tag an import uses: an existing one of that name, or a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTag {
    /// The tag's name.
    pub name: String,
    /// Default interval of a new tag, in seconds; an existing tag keeps its own.
    pub default_interval_secs: Option<u64>,
    /// Addresses of targets a new tag notifies by default; an existing tag keeps its own.
    pub target_urls: Vec<String>,
}

/// A notification target an import uses: the existing one with this address, or a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// The notification address.
    pub url: String,
    /// The name a new target is given; a taken name gets a number added.
    pub name: String,
}

/// A watch to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWatch {
    /// The settings of the watch.
    pub settings: NewWatch,
    /// Whether it starts paused.
    pub paused: bool,
    /// Names of its tags; each is one of the batch's [`ImportTag`]s.
    pub tags: Vec<String>,
    /// Addresses of the targets it notifies; each is one of the batch's [`ImportTarget`]s.
    pub target_urls: Vec<String>,
    /// Its message template.
    pub template: Option<String>,
}

/// Everything an import makes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportBatch {
    /// The tags the watches use.
    pub tags: Vec<ImportTag>,
    /// The targets the watches and tags use.
    pub targets: Vec<ImportTarget>,
    /// The watches to make.
    pub watches: Vec<ImportWatch>,
    /// Make a watch even when one with its address exists. By default it is skipped, so that
    /// running an import twice does not double the watches.
    pub allow_duplicates: bool,
}

/// What an import made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    /// For each watch of the batch, its id, or `None` if it was skipped because a watch with
    /// its address exists.
    pub watches: Vec<Option<WatchId>>,
    /// Tags that were made; the rest already existed.
    pub tags_created: usize,
    /// Targets that were made; the rest already existed.
    pub targets_created: usize,
}

/// The existing name that `wanted` stands for: an exact match, else the only name that
/// differs by case. Two names that differ only by case are different tags, so a name that
/// matches both ways is not guessed at.
pub fn match_name<'a>(
    existing: impl IntoIterator<Item = &'a str>,
    wanted: &str,
) -> Option<&'a str> {
    let lower = wanted.to_lowercase();
    let mut similar = None;
    let mut count = 0;
    for name in existing {
        if name == wanted {
            return Some(name);
        }
        if name.to_lowercase() == lower {
            similar = Some(name);
            count += 1;
        }
    }
    if count == 1 { similar } else { None }
}

/// Find `base`, or `base 2`, `base 3`, ... among `taken`, whichever is free.
pub fn free_name(taken: &HashSet<String>, base: &str) -> String {
    if !taken.contains(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| base.to_owned())
}

fn id_of(text: &str, what: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(text).map_err(|_| StoreError::Corrupt(format!("invalid {what} id {text:?}")))
}

async fn names(
    conn: &mut SqliteConnection,
    sql: &'static str,
) -> Result<Vec<(Uuid, String)>, StoreError> {
    let rows = sqlx::query(sql).fetch_all(conn).await?;
    rows.iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            Ok((id_of(&id, "row")?, row.try_get("key")?))
        })
        .collect()
}

impl Store {
    /// Make everything in `batch`, all or nothing.
    ///
    /// Tags are matched by name ([`match_name`]), targets by address, and new ones are made.
    /// A watch whose address is already watched is skipped unless the batch allows duplicates.
    /// A watch made here has no snapshots: its first check is its baseline and never notifies.
    pub async fn apply_import(&self, batch: &ImportBatch) -> Result<ImportOutcome, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut outcome = ImportOutcome::default();

        let mut targets: HashMap<String, Uuid> =
            names(&mut tx, "SELECT id, url AS key FROM notification_targets")
                .await?
                .into_iter()
                .map(|(id, url)| (url, id))
                .collect();
        let mut target_names: HashSet<String> =
            names(&mut tx, "SELECT id, name AS key FROM notification_targets")
                .await?
                .into_iter()
                .map(|(_, name)| name)
                .collect();
        for target in &batch.targets {
            if targets.contains_key(&target.url) {
                continue;
            }
            let id = Uuid::now_v7();
            let name = free_name(&target_names, &target.name);
            let new = NewTarget {
                name: name.clone(),
                url: target.url.clone(),
                default_template: None,
            };
            insert_target(&mut tx, id, &new).await?;
            target_names.insert(name);
            targets.insert(target.url.clone(), id);
            outcome.targets_created += 1;
        }
        let target_ids = |urls: &[String]| -> Result<Vec<Uuid>, StoreError> {
            let mut ids = Vec::new();
            for url in urls {
                let id = targets.get(url).copied().ok_or_else(|| {
                    StoreError::Corrupt("an import names a target it does not list".to_owned())
                })?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Ok(ids)
        };

        let existing_tags = names(&mut tx, "SELECT id, name AS key FROM tags").await?;
        let mut tags: HashMap<String, Uuid> = HashMap::new();
        for tag in &batch.tags {
            let found = match_name(existing_tags.iter().map(|(_, n)| n.as_str()), &tag.name)
                .and_then(|name| existing_tags.iter().find(|(_, n)| n == name));
            if let Some((id, _)) = found {
                tags.insert(tag.name.clone(), *id);
                continue;
            }
            let id = Uuid::now_v7();
            let new = NewTag {
                name: tag.name.clone(),
                color: NEW_TAG_COLOR.to_owned(),
                default_interval_secs: tag.default_interval_secs,
            };
            insert_tag(&mut tx, id, &new).await?;
            for target in target_ids(&tag.target_urls)? {
                sqlx::query(
                    "INSERT OR IGNORE INTO tag_notifications (tag_id, target_id) VALUES (?, ?)",
                )
                .bind(id.to_string())
                .bind(target.to_string())
                .execute(&mut *tx)
                .await?;
            }
            tags.insert(tag.name.clone(), id);
            outcome.tags_created += 1;
        }

        let watched: HashSet<String> = names(&mut tx, "SELECT id, url AS key FROM watches")
            .await?
            .into_iter()
            .map(|(_, url)| url)
            .collect();
        for watch in &batch.watches {
            if !batch.allow_duplicates && watched.contains(&watch.settings.url) {
                outcome.watches.push(None);
                continue;
            }
            let id = WatchId::from_uuid(Uuid::now_v7());
            insert_watch(&mut tx, id, &watch.settings, now_ms()).await?;
            if watch.paused {
                sqlx::query("UPDATE watches SET paused = 1 WHERE id = ?")
                    .bind(id.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
            if let Some(template) = &watch.template {
                sqlx::query("UPDATE watches SET notification_template_override = ? WHERE id = ?")
                    .bind(template)
                    .bind(id.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
            for name in &watch.tags {
                let tag = tags.get(name).ok_or_else(|| {
                    StoreError::Corrupt("an import names a tag it does not list".to_owned())
                })?;
                sqlx::query("INSERT OR IGNORE INTO watch_tags (watch_id, tag_id) VALUES (?, ?)")
                    .bind(id.to_string())
                    .bind(tag.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
            for target in target_ids(&watch.target_urls)? {
                sqlx::query(
                    "INSERT OR IGNORE INTO watch_notifications (watch_id, target_id) VALUES (?, ?)",
                )
                .bind(id.to_string())
                .bind(target.to_string())
                .execute(&mut *tx)
                .await?;
            }
            outcome.watches.push(Some(id));
        }
        tx.commit().await?;
        Ok(outcome)
    }
}
