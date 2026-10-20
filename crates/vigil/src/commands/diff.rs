//! `vigil diff <id>`.

use super::{open_store, resolve};
use crate::config::Config;
use crate::format::{colorize, timestamp, wants_color};
use vigil_core::LineDiff;

/// Lines of unchanged context around each change.
const CONTEXT: usize = 3;

/// Print the difference between a watch's two newest snapshots.
pub async fn run(config: &Config, id: &str) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let id = resolve(&store, id).await?;
    let watch = store.get_watch(id).await?;
    let snapshots = store.list_snapshots(id, 2).await?;
    let [newer, older] = snapshots.as_slice() else {
        println!(
            "{}: nothing to compare yet (needs two snapshots, has {})",
            watch.url,
            snapshots.len()
        );
        return Ok(());
    };

    let old_text = store.snapshot_content(older.id).await?;
    let new_text = store.snapshot_content(newer.id).await?;
    let diff = LineDiff::between(&old_text, &new_text);
    if diff.is_empty() {
        println!(
            "{}: the two newest snapshots have the same lines",
            watch.url
        );
        return Ok(());
    }

    let mut out = format!(
        "--- {} ({})\n+++ {} ({})\n{}",
        watch.url,
        timestamp(older.taken_at),
        watch.url,
        timestamp(newer.taken_at),
        diff.unified(CONTEXT),
    );
    if wants_color() {
        out = colorize(&out);
    }
    print!("{out}");
    Ok(())
}
