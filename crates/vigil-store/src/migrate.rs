//! Backup of the database file before pending migrations are applied.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;

use crate::{StoreError, now_ms};

/// Copy the database to `<file>.backup-v<latest>-<unix ms>` if `migrator` holds migrations the
/// database has not applied yet.
///
/// A database that has never been migrated (a fresh file) and one that is already up to date are
/// left alone. Returns the path of the backup when one was made. A failed backup is an error:
/// migrating without one would defeat the point.
pub(crate) async fn backup_if_pending(
    pool: &SqlitePool,
    path: &Path,
    migrator: &Migrator,
) -> Result<Option<PathBuf>, StoreError> {
    let table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?;
    if table.is_none() {
        return Ok(None);
    }
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1")
            .fetch_all(pool)
            .await?;
    let Some(latest) = applied.iter().copied().max() else {
        return Ok(None);
    };
    let pending = migrator
        .iter()
        .any(|m| m.migration_type.is_up_migration() && !applied.contains(&m.version));
    if !pending {
        return Ok(None);
    }

    let mut name = path.as_os_str().to_owned();
    name.push(format!(".backup-v{latest}-{}", now_ms()));
    let target = PathBuf::from(name);
    let target_str = target
        .to_str()
        .ok_or_else(|| StoreError::Backup("the database path is not valid UTF-8".into()))?;
    // VACUUM INTO writes a consistent copy even in WAL mode, which a plain file copy would not.
    sqlx::query("VACUUM INTO ?1")
        .bind(target_str)
        .execute(pool)
        .await
        .map_err(|e| StoreError::Backup(e.to_string()))?;
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use sqlx::SqlSafeStr;
    use sqlx::migrate::{Migration, MigrationType};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn migration(version: i64, sql: &'static str) -> Migration {
        Migration::new(
            version,
            format!("m{version}").into(),
            MigrationType::Simple,
            sql.into_sql_str(),
            false,
        )
    }

    fn migrator(count: i64) -> Migrator {
        let all = [
            migration(1, "CREATE TABLE one (id INTEGER)"),
            migration(2, "CREATE TABLE two (id INTEGER)"),
        ];
        Migrator::with_migrations(all.into_iter().take(count as usize).collect())
    }

    async fn pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        SqlitePoolOptions::new().connect_with(options).await
    }

    fn files(dir: &Path) -> Result<Vec<String>, std::io::Error> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    #[tokio::test]
    async fn a_fresh_database_is_not_backed_up() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v.db");
        let pool = pool(&path).await?;
        assert_eq!(backup_if_pending(&pool, &path, &migrator(2)).await?, None);
        assert_eq!(files(dir.path())?, ["v.db"]);
        Ok(())
    }

    #[tokio::test]
    async fn an_up_to_date_database_is_not_backed_up() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v.db");
        let pool = pool(&path).await?;
        migrator(2).run(&pool).await?;
        assert_eq!(backup_if_pending(&pool, &path, &migrator(2)).await?, None);
        assert_eq!(files(dir.path())?, ["v.db"]);
        Ok(())
    }

    #[tokio::test]
    async fn pending_migrations_get_a_backup_at_the_old_version() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v.db");
        let pool = pool(&path).await?;
        migrator(1).run(&pool).await?;
        sqlx::query("INSERT INTO one (id) VALUES (7)")
            .execute(&pool)
            .await?;

        let backup = backup_if_pending(&pool, &path, &migrator(2))
            .await?
            .ok_or("expected a backup")?;
        let name = backup.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with("v.db.backup-v1-"), "{name}");

        let copy = self::pool(&backup).await?;
        let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations")
            .fetch_all(&copy)
            .await?;
        assert_eq!(versions, [1]);
        let rows: Vec<i64> = sqlx::query_scalar("SELECT id FROM one")
            .fetch_all(&copy)
            .await?;
        assert_eq!(rows, [7]);
        Ok(())
    }
}
