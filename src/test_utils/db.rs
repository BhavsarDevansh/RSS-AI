use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

/// Creates an isolated in-memory SQLite pool with migrations applied.
///
/// Each call produces a completely independent database, safe for parallel tests.
pub async fn test_pool() -> SqlitePool {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")
        .expect("valid connection string")
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("failed to create in-memory pool");

    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("failed to run migrations");

    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_works() {
        let pool = test_pool().await;
        let row: (i64,) = sqlx::query_as("SELECT 1")
            .fetch_one(&pool)
            .await
            .expect("SELECT 1 failed");
        assert_eq!(row.0, 1);
    }

    #[tokio::test]
    async fn migration_applied() {
        let pool = test_pool().await;
        // Verify the baseline migration created the _schema_version table
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _schema_version")
            .fetch_one(&pool)
            .await
            .expect("_schema_version table should exist");
        assert_eq!(row.0, 1);
    }
}
