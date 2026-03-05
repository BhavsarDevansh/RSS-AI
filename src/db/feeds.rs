/// Feed CRUD operations.
use sqlx::SqlitePool;

use super::DbError;
use super::models::Feed;

/// Add a new feed. Returns `DuplicateEntry` if the URL already exists.
pub async fn add_feed(pool: &SqlitePool, url: &str, title: Option<&str>) -> Result<Feed, DbError> {
    let result =
        sqlx::query_as::<_, Feed>("INSERT INTO feeds (url, title) VALUES (?, ?) RETURNING *")
            .bind(url)
            .bind(title)
            .fetch_one(pool)
            .await;

    match result {
        Ok(feed) => Ok(feed),
        Err(sqlx::Error::Database(ref e)) if e.message().contains("UNIQUE") => Err(
            DbError::DuplicateEntry(format!("feed URL already exists: {url}")),
        ),
        Err(e) => Err(DbError::Sqlx(e)),
    }
}

/// Remove a feed by ID. Returns `NotFound` if it doesn't exist.
pub async fn remove_feed(pool: &SqlitePool, feed_id: i64) -> Result<(), DbError> {
    let result = sqlx::query("DELETE FROM feeds WHERE id = ?")
        .bind(feed_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("feed id={feed_id}")));
    }
    Ok(())
}

/// List all feeds.
pub async fn list_feeds(pool: &SqlitePool) -> Result<Vec<Feed>, DbError> {
    let feeds = sqlx::query_as::<_, Feed>("SELECT * FROM feeds ORDER BY id")
        .fetch_all(pool)
        .await?;
    Ok(feeds)
}

/// Get a single feed by ID.
pub async fn get_feed(pool: &SqlitePool, feed_id: i64) -> Result<Feed, DbError> {
    sqlx::query_as::<_, Feed>("SELECT * FROM feeds WHERE id = ?")
        .bind(feed_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("feed id={feed_id}")))
}

/// Update feed metadata. Only non-None fields are changed (COALESCE pattern).
pub async fn update_feed(
    pool: &SqlitePool,
    feed_id: i64,
    title: Option<&str>,
    description: Option<&str>,
    site_url: Option<&str>,
    poll_interval_minutes: Option<i64>,
) -> Result<Feed, DbError> {
    let feed = sqlx::query_as::<_, Feed>(
        "UPDATE feeds SET
            title = COALESCE(?, title),
            description = COALESCE(?, description),
            site_url = COALESCE(?, site_url),
            poll_interval_minutes = COALESCE(?, poll_interval_minutes),
            updated_at = datetime('now')
         WHERE id = ?
         RETURNING *",
    )
    .bind(title)
    .bind(description)
    .bind(site_url)
    .bind(poll_interval_minutes)
    .bind(feed_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("feed id={feed_id}")))?;

    Ok(feed)
}

/// Update poll status after a fetch attempt.
///
/// On success: sets `last_polled_at` to now, clears error fields.
/// On failure: sets `last_error`, increments `error_count`.
pub async fn update_poll_status(
    pool: &SqlitePool,
    feed_id: i64,
    success: bool,
    error_message: Option<&str>,
) -> Result<(), DbError> {
    if success {
        sqlx::query(
            "UPDATE feeds SET last_polled_at = datetime('now'), last_error = NULL, error_count = 0, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(feed_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE feeds SET last_error = ?, error_count = error_count + 1, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(error_message)
        .bind(feed_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::db::test_pool;

    #[tokio::test]
    async fn add_and_get_feed() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", Some("Example"))
            .await
            .unwrap();
        assert_eq!(feed.url, "https://example.com/rss");
        assert_eq!(feed.title.as_deref(), Some("Example"));
        assert_eq!(feed.active, 1);

        let fetched = get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(fetched.id, feed.id);
    }

    #[tokio::test]
    async fn duplicate_feed_url() {
        let pool = test_pool().await;
        add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        let err = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DuplicateEntry(_)));
    }

    #[tokio::test]
    async fn list_feeds_returns_all() {
        let pool = test_pool().await;
        add_feed(&pool, "https://a.com/rss", None).await.unwrap();
        add_feed(&pool, "https://b.com/rss", None).await.unwrap();
        let feeds = list_feeds(&pool).await.unwrap();
        assert_eq!(feeds.len(), 2);
    }

    #[tokio::test]
    async fn update_feed_coalesce() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", Some("Old"))
            .await
            .unwrap();

        let updated = update_feed(&pool, feed.id, Some("New"), None, None, None)
            .await
            .unwrap();
        assert_eq!(updated.title.as_deref(), Some("New"));
        // Other fields unchanged
        assert_eq!(updated.poll_interval_minutes, 30);
    }

    #[tokio::test]
    async fn remove_feed_works() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        remove_feed(&pool, feed.id).await.unwrap();

        let err = get_feed(&pool, feed.id).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn remove_nonexistent_feed() {
        let pool = test_pool().await;
        let err = remove_feed(&pool, 999).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn poll_status_success() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();

        update_poll_status(&pool, feed.id, true, None)
            .await
            .unwrap();
        let fetched = get_feed(&pool, feed.id).await.unwrap();
        assert!(fetched.last_polled_at.is_some());
        assert!(fetched.last_error.is_none());
        assert_eq!(fetched.error_count, 0);
    }

    #[tokio::test]
    async fn poll_status_failure_increments() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();

        update_poll_status(&pool, feed.id, false, Some("timeout"))
            .await
            .unwrap();
        let f1 = get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(f1.error_count, 1);
        assert_eq!(f1.last_error.as_deref(), Some("timeout"));

        update_poll_status(&pool, feed.id, false, Some("timeout again"))
            .await
            .unwrap();
        let f2 = get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(f2.error_count, 2);
    }

    #[tokio::test]
    async fn add_feed_without_title() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        assert!(feed.title.is_none());
    }

    #[tokio::test]
    async fn list_feeds_empty() {
        let pool = test_pool().await;
        let feeds = list_feeds(&pool).await.unwrap();
        assert!(feeds.is_empty());
    }

    #[tokio::test]
    async fn get_feed_not_found() {
        let pool = test_pool().await;
        let err = get_feed(&pool, 999).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_feed_all_none_is_noop() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", Some("Title"))
            .await
            .unwrap();

        let updated = update_feed(&pool, feed.id, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(updated.title.as_deref(), Some("Title"));
        assert_eq!(updated.poll_interval_minutes, 30);
    }

    #[tokio::test]
    async fn update_feed_all_some() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();

        let updated = update_feed(
            &pool,
            feed.id,
            Some("New Title"),
            Some("Description"),
            Some("https://example.com"),
            Some(60),
        )
        .await
        .unwrap();
        assert_eq!(updated.title.as_deref(), Some("New Title"));
        assert_eq!(updated.description.as_deref(), Some("Description"));
        assert_eq!(updated.site_url.as_deref(), Some("https://example.com"));
        assert_eq!(updated.poll_interval_minutes, 60);
    }

    #[tokio::test]
    async fn update_feed_not_found() {
        let pool = test_pool().await;
        let err = update_feed(&pool, 999, Some("Title"), None, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn poll_status_failure_then_success_clears_error() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();

        // Fail twice
        update_poll_status(&pool, feed.id, false, Some("err1"))
            .await
            .unwrap();
        update_poll_status(&pool, feed.id, false, Some("err2"))
            .await
            .unwrap();
        let f = get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(f.error_count, 2);

        // Then succeed — should clear errors
        update_poll_status(&pool, feed.id, true, None)
            .await
            .unwrap();
        let f = get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(f.error_count, 0);
        assert!(f.last_error.is_none());
        assert!(f.last_polled_at.is_some());
    }

    #[tokio::test]
    async fn feed_default_values() {
        let pool = test_pool().await;
        let feed = add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        assert_eq!(feed.active, 1);
        assert_eq!(feed.poll_interval_minutes, 30);
        assert_eq!(feed.error_count, 0);
        assert!(feed.last_polled_at.is_none());
        assert!(feed.last_error.is_none());
        assert!(!feed.created_at.is_empty());
        assert!(!feed.updated_at.is_empty());
    }
}
