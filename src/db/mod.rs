/// SQLite database access, migrations, and CRUD operations.
pub mod articles;
pub mod feeds;
pub mod links;
pub mod models;
pub mod tags;

pub use models::*;

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("duplicate entry: {0}")]
    DuplicateEntry(String),
}

// ── Pool initialisation ────────────────────────────────────────────

/// Initialise a SQLite connection pool at `<data_dir>/rss_ai.db`.
///
/// Creates the directory if needed, enables WAL mode, foreign keys,
/// sets a 5-second busy timeout, and runs pending migrations.
pub async fn init_pool(data_dir: &Path) -> Result<SqlitePool, DbError> {
    std::fs::create_dir_all(data_dir).map_err(|e| {
        sqlx::Error::Io(std::io::Error::new(
            e.kind(),
            format!("failed to create data_dir {}: {e}", data_dir.display()),
        ))
    })?;

    let db_path = data_dir.join("rss_ai.db");

    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_millis(5000));

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;

    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use crate::test_utils::db::test_pool;

    #[tokio::test]
    async fn tables_exist() {
        let pool = test_pool().await;
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        let names: Vec<&str> = tables.iter().map(|r| r.0.as_str()).collect();
        assert!(names.contains(&"feeds"), "missing feeds table");
        assert!(names.contains(&"articles"), "missing articles table");
        assert!(names.contains(&"tags"), "missing tags table");
        assert!(
            names.contains(&"article_tags"),
            "missing article_tags table"
        );
        assert!(
            names.contains(&"article_links"),
            "missing article_links table"
        );
    }

    #[tokio::test]
    async fn init_pool_creates_db() {
        let dir = tempfile::tempdir().unwrap();
        let pool = super::init_pool(dir.path()).await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 1);
        assert!(dir.path().join("rss_ai.db").exists());
    }

    #[tokio::test]
    async fn init_pool_enables_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let pool = super::init_pool(dir.path()).await.unwrap();
        let row: (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 1, "foreign keys should be enabled");
    }

    #[tokio::test]
    async fn init_pool_enables_wal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let pool = super::init_pool(dir.path()).await.unwrap();
        let row: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn init_pool_invalid_path() {
        let result = super::init_pool(std::path::Path::new("/proc/nonexistent/db")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn indexes_exist() {
        let pool = test_pool().await;
        let indexes: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let names: Vec<&str> = indexes.iter().map(|r| r.0.as_str()).collect();
        assert!(names.contains(&"idx_articles_feed_id"));
        assert!(names.contains(&"idx_articles_url"));
        assert!(names.contains(&"idx_articles_published_at"));
        assert!(names.contains(&"idx_articles_content_hash"));
        assert!(names.contains(&"idx_article_tags_tag_id"));
        assert!(names.contains(&"idx_article_links_source"));
        assert!(names.contains(&"idx_article_links_target"));
    }

    #[tokio::test]
    async fn triggers_exist() {
        let pool = test_pool().await;
        let triggers: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<&str> = triggers.iter().map(|r| r.0.as_str()).collect();
        assert!(names.contains(&"trg_article_tags_insert"));
        assert!(names.contains(&"trg_article_tags_delete"));
    }
}
