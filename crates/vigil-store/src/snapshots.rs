//! Snapshot reads and zstd content handling.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;
use vigil_core::{ContentHash, WatchId};

use crate::{Store, StoreError};

/// Identifier of a stored snapshot (a UUID v7).
pub type SnapshotId = Uuid;

/// zstd level for snapshot text; the library default trades speed and size sensibly.
const ZSTD_LEVEL: i32 = 3;

/// A snapshot without its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Snapshot id.
    pub id: SnapshotId,
    /// The watch it belongs to.
    pub watch_id: WatchId,
    /// When it was taken, unix milliseconds.
    pub taken_at: i64,
    /// Hash of the processed text.
    pub hash: ContentHash,
    /// Size of the fetched body in bytes, before processing.
    pub raw_size: u64,
    /// HTTP status of the response.
    pub http_status: Option<u16>,
    /// `Content-Type` of the response.
    pub content_type: Option<String>,
}

impl Store {
    /// A watch's snapshots, newest first, at most `limit`.
    pub async fn list_snapshots(
        &self,
        watch: WatchId,
        limit: u32,
    ) -> Result<Vec<SnapshotMeta>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, taken_at, hash, raw_size, http_status, content_type \
             FROM snapshots WHERE watch_id = ? ORDER BY taken_at DESC, id DESC LIMIT ?",
        )
        .bind(watch.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(meta_from_row).collect()
    }

    /// The processed text of a snapshot.
    pub async fn snapshot_content(&self, id: SnapshotId) -> Result<String, StoreError> {
        let row = sqlx::query("SELECT content FROM snapshots WHERE id = ?")
            .bind(id.hyphenated().to_string())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => decompress(&row.try_get::<Vec<u8>, _>("content")?),
            None => Err(StoreError::NotFound(format!("snapshot {id}"))),
        }
    }
}

pub(crate) fn compress(text: &str) -> Result<Vec<u8>, StoreError> {
    zstd::encode_all(text.as_bytes(), ZSTD_LEVEL).map_err(StoreError::Compression)
}

fn decompress(bytes: &[u8]) -> Result<String, StoreError> {
    let raw = zstd::decode_all(bytes).map_err(StoreError::Compression)?;
    String::from_utf8(raw).map_err(|_| StoreError::Corrupt("snapshot is not valid utf-8".into()))
}

fn meta_from_row(row: &SqliteRow) -> Result<SnapshotMeta, StoreError> {
    let id: String = row.try_get("id")?;
    let watch_id: String = row.try_get("watch_id")?;
    let hash: Vec<u8> = row.try_get("hash")?;
    let raw_size: i64 = row.try_get("raw_size")?;
    let http_status: Option<i64> = row.try_get("http_status")?;
    Ok(SnapshotMeta {
        id: Uuid::parse_str(&id)
            .map_err(|_| StoreError::Corrupt(format!("invalid snapshot id {id:?}")))?,
        watch_id: watch_id
            .parse()
            .map_err(|_| StoreError::Corrupt(format!("invalid watch id {watch_id:?}")))?,
        taken_at: row.try_get("taken_at")?,
        hash: ContentHash::from_slice(&hash)
            .ok_or_else(|| StoreError::Corrupt("snapshot hash is not 32 bytes".into()))?,
        raw_size: u64::try_from(raw_size)
            .map_err(|_| StoreError::Corrupt(format!("negative raw_size {raw_size}")))?,
        http_status: http_status
            .map(|s| {
                u16::try_from(s).map_err(|_| StoreError::Corrupt(format!("bad http_status {s}")))
            })
            .transpose()?,
        content_type: row.try_get("content_type")?,
    })
}
