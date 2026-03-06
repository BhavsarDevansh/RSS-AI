/// Article CRUD operations.
use sqlx::SqlitePool;

use super::DbError;
use super::models::{Article, NewArticle};

/// Extracted article content payload for persistence.
pub struct ExtractedArticleUpdate<'a> {
    pub content: &'a str,
    pub content_hash: &'a str,
    pub word_count: i64,
    pub title: Option<&'a str>,
    pub author: Option<&'a str>,
    pub published_at: Option<&'a str>,
}

/// Insert a single article. Returns `DuplicateEntry` on UNIQUE constraint violation.
pub async fn insert_article(pool: &SqlitePool, article: &NewArticle) -> Result<Article, DbError> {
    let result = sqlx::query_as::<_, Article>(
        "INSERT INTO articles (feed_id, guid, url, title, author, published_at, summary, content, content_hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING *",
    )
    .bind(article.feed_id)
    .bind(&article.guid)
    .bind(&article.url)
    .bind(&article.title)
    .bind(&article.author)
    .bind(&article.published_at)
    .bind(&article.summary)
    .bind(&article.content)
    .bind(&article.content_hash)
    .fetch_one(pool)
    .await;

    match result {
        Ok(a) => Ok(a),
        Err(sqlx::Error::Database(ref e)) if e.message().contains("UNIQUE") => {
            Err(DbError::DuplicateEntry(format!(
                "article already exists: url={} guid={:?}",
                article.url, article.guid
            )))
        }
        Err(e) => Err(DbError::Sqlx(e)),
    }
}

/// Insert multiple articles in a single transaction.
/// Returns the count of successfully inserted articles (skips duplicates).
pub async fn insert_articles_batch(
    pool: &SqlitePool,
    articles: &[NewArticle],
) -> Result<u64, DbError> {
    let mut tx = pool.begin().await?;
    let mut count = 0u64;

    for article in articles {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO articles (feed_id, guid, url, title, author, published_at, summary, content, content_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(article.feed_id)
        .bind(&article.guid)
        .bind(&article.url)
        .bind(&article.title)
        .bind(&article.author)
        .bind(&article.published_at)
        .bind(&article.summary)
        .bind(&article.content)
        .bind(&article.content_hash)
        .execute(&mut *tx)
        .await?;

        count += result.rows_affected();
    }

    tx.commit().await?;
    Ok(count)
}

/// Get a single article by ID.
pub async fn get_article(pool: &SqlitePool, article_id: i64) -> Result<Article, DbError> {
    sqlx::query_as::<_, Article>("SELECT * FROM articles WHERE id = ?")
        .bind(article_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("article id={article_id}")))
}

/// Get all articles for a feed, ordered by published_at descending.
pub async fn get_articles_by_feed(
    pool: &SqlitePool,
    feed_id: i64,
) -> Result<Vec<Article>, DbError> {
    let articles = sqlx::query_as::<_, Article>(
        "SELECT * FROM articles WHERE feed_id = ? ORDER BY published_at DESC",
    )
    .bind(feed_id)
    .fetch_all(pool)
    .await?;
    Ok(articles)
}

/// Check whether an article with the given URL already exists.
pub async fn article_exists(pool: &SqlitePool, url: &str) -> Result<bool, DbError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM articles WHERE url = ?")
        .bind(url)
        .fetch_one(pool)
        .await?;
    Ok(row.0 > 0)
}

/// Update article content and mark content_extracted = 1.
pub async fn update_article_content(
    pool: &SqlitePool,
    article_id: i64,
    content: &str,
    content_hash: &str,
) -> Result<(), DbError> {
    let word_count = content.split_whitespace().count() as i64;
    let update = ExtractedArticleUpdate {
        content,
        content_hash,
        word_count,
        title: None,
        author: None,
        published_at: None,
    };
    update_article_content_with_metadata(pool, article_id, &update).await
}

/// Update extracted content fields and optional metadata from the article page.
pub async fn update_article_content_with_metadata(
    pool: &SqlitePool,
    article_id: i64,
    update: &ExtractedArticleUpdate<'_>,
) -> Result<(), DbError> {
    let result = sqlx::query(
        "UPDATE articles SET
            content = ?,
            content_hash = ?,
            word_count = ?,
            title = COALESCE(?, title),
            author = COALESCE(?, author),
            published_at = COALESCE(?, published_at),
            content_extracted = 1,
            updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(update.content)
    .bind(update.content_hash)
    .bind(update.word_count)
    .bind(update.title)
    .bind(update.author)
    .bind(update.published_at)
    .bind(article_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("article id={article_id}")));
    }
    Ok(())
}

/// Mark that embeddings have been generated for an article.
pub async fn mark_embedding_generated(pool: &SqlitePool, article_id: i64) -> Result<(), DbError> {
    let result = sqlx::query(
        "UPDATE articles SET embedding_generated = 1, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(article_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("article id={article_id}")));
    }
    Ok(())
}

/// Mark that tags have been generated for an article.
pub async fn mark_tags_generated(pool: &SqlitePool, article_id: i64) -> Result<(), DbError> {
    let result = sqlx::query(
        "UPDATE articles SET tags_generated = 1, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(article_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("article id={article_id}")));
    }
    Ok(())
}

/// Search articles within a date range (inclusive).
pub async fn search_articles_by_date_range(
    pool: &SqlitePool,
    from: &str,
    to: &str,
) -> Result<Vec<Article>, DbError> {
    let articles = sqlx::query_as::<_, Article>(
        "SELECT * FROM articles WHERE published_at >= ? AND published_at <= ? ORDER BY published_at DESC",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(articles)
}

/// Get the most recent articles across all feeds.
pub async fn get_recent_articles(pool: &SqlitePool, limit: i64) -> Result<Vec<Article>, DbError> {
    let articles =
        sqlx::query_as::<_, Article>("SELECT * FROM articles ORDER BY created_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(pool)
            .await?;
    Ok(articles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::feeds;
    use crate::test_utils::db::test_pool;

    fn sample_article(feed_id: i64) -> NewArticle {
        NewArticle {
            feed_id,
            guid: Some("guid-1".to_string()),
            url: "https://example.com/article-1".to_string(),
            title: "Test Article".to_string(),
            author: Some("Author".to_string()),
            published_at: Some("2024-01-01 00:00:00".to_string()),
            summary: Some("Summary".to_string()),
            content: None,
            content_hash: None,
        }
    }

    async fn setup_feed(pool: &SqlitePool) -> i64 {
        feeds::add_feed(pool, "https://example.com/rss", None)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn insert_and_get_article() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();
        assert_eq!(article.title, "Test Article");
        assert_eq!(article.content_extracted, 0);

        let fetched = get_article(&pool, article.id).await.unwrap();
        assert_eq!(fetched.id, article.id);
    }

    #[tokio::test]
    async fn duplicate_article_url() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();
        let err = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DuplicateEntry(_)));
    }

    #[tokio::test]
    async fn batch_insert_skips_duplicates() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;

        let a1 = sample_article(feed_id);
        insert_article(&pool, &a1).await.unwrap();

        let a2 = NewArticle {
            guid: Some("guid-2".to_string()),
            url: "https://example.com/article-2".to_string(),
            title: "Second".to_string(),
            ..sample_article(feed_id)
        };

        let count = insert_articles_batch(&pool, &[a1, a2]).await.unwrap();
        assert_eq!(count, 1); // first was duplicate, second inserted
    }

    #[tokio::test]
    async fn get_articles_by_feed_works() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        let articles = get_articles_by_feed(&pool, feed_id).await.unwrap();
        assert_eq!(articles.len(), 1);
    }

    #[tokio::test]
    async fn article_exists_check() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        assert!(
            !article_exists(&pool, "https://example.com/article-1")
                .await
                .unwrap()
        );
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();
        assert!(
            article_exists(&pool, "https://example.com/article-1")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn update_content_sets_flag() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        update_article_content(&pool, article.id, "Full content", "hash123")
            .await
            .unwrap();
        let updated = get_article(&pool, article.id).await.unwrap();
        assert_eq!(updated.content.as_deref(), Some("Full content"));
        assert_eq!(updated.content_hash.as_deref(), Some("hash123"));
        assert_eq!(updated.word_count, Some(2));
        assert_eq!(updated.content_extracted, 1);
    }

    #[tokio::test]
    async fn mark_embedding_and_tags() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        mark_embedding_generated(&pool, article.id).await.unwrap();
        let a = get_article(&pool, article.id).await.unwrap();
        assert_eq!(a.embedding_generated, 1);

        mark_tags_generated(&pool, article.id).await.unwrap();
        let a = get_article(&pool, article.id).await.unwrap();
        assert_eq!(a.tags_generated, 1);
    }

    #[tokio::test]
    async fn date_range_search() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        let results = search_articles_by_date_range(&pool, "2023-01-01", "2025-01-01")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let empty = search_articles_by_date_range(&pool, "2025-01-01", "2025-12-31")
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn recent_articles() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        let recent = get_recent_articles(&pool, 10).await.unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[tokio::test]
    async fn cascade_delete_feed_removes_articles() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        feeds::remove_feed(&pool, feed_id).await.unwrap();
        let err = get_article(&pool, article.id).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn get_article_not_found() {
        let pool = test_pool().await;
        let err = get_article(&pool, 999).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn insert_article_nonexistent_feed() {
        let pool = test_pool().await;
        let mut article = sample_article(999);
        article.url = "https://example.com/orphan".to_string();
        let err = insert_article(&pool, &article).await.unwrap_err();
        assert!(matches!(err, DbError::Sqlx(_)));
    }

    #[tokio::test]
    async fn batch_insert_empty_slice() {
        let pool = test_pool().await;
        let count = insert_articles_batch(&pool, &[]).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn batch_insert_all_duplicates() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let a = sample_article(feed_id);
        insert_article(&pool, &a).await.unwrap();

        let count = insert_articles_batch(&pool, &[a]).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn batch_insert_all_new() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;

        let articles: Vec<NewArticle> = (0..3)
            .map(|i| NewArticle {
                feed_id,
                guid: Some(format!("batch-guid-{i}")),
                url: format!("https://example.com/batch-{i}"),
                title: format!("Batch {i}"),
                author: None,
                published_at: None,
                summary: None,
                content: None,
                content_hash: None,
            })
            .collect();

        let count = insert_articles_batch(&pool, &articles).await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn duplicate_guid_same_feed() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        // Same guid, different URL — should still fail on (feed_id, guid) unique
        let a2 = NewArticle {
            url: "https://example.com/different-url".to_string(),
            ..sample_article(feed_id)
        };
        let err = insert_article(&pool, &a2).await.unwrap_err();
        assert!(matches!(err, DbError::DuplicateEntry(_)));
    }

    #[tokio::test]
    async fn get_articles_by_feed_empty() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let articles = get_articles_by_feed(&pool, feed_id).await.unwrap();
        assert!(articles.is_empty());
    }

    #[tokio::test]
    async fn get_articles_by_feed_ordering() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;

        let a1 = NewArticle {
            guid: Some("early".to_string()),
            url: "https://example.com/early".to_string(),
            title: "Early".to_string(),
            published_at: Some("2024-01-01 00:00:00".to_string()),
            ..sample_article(feed_id)
        };
        let a2 = NewArticle {
            guid: Some("late".to_string()),
            url: "https://example.com/late".to_string(),
            title: "Late".to_string(),
            published_at: Some("2024-06-01 00:00:00".to_string()),
            ..sample_article(feed_id)
        };
        insert_article(&pool, &a1).await.unwrap();
        insert_article(&pool, &a2).await.unwrap();

        let articles = get_articles_by_feed(&pool, feed_id).await.unwrap();
        assert_eq!(articles.len(), 2);
        // Should be DESC by published_at: late first
        assert_eq!(articles[0].title, "Late");
        assert_eq!(articles[1].title, "Early");
    }

    #[tokio::test]
    async fn get_articles_by_nonexistent_feed() {
        let pool = test_pool().await;
        let articles = get_articles_by_feed(&pool, 999).await.unwrap();
        assert!(articles.is_empty());
    }

    #[tokio::test]
    async fn update_content_not_found() {
        let pool = test_pool().await;
        let err = update_article_content(&pool, 999, "content", "hash")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn mark_embedding_not_found() {
        let pool = test_pool().await;
        let err = mark_embedding_generated(&pool, 999).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn mark_tags_not_found() {
        let pool = test_pool().await;
        let err = mark_tags_generated(&pool, 999).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn mark_embedding_idempotent() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        mark_embedding_generated(&pool, article.id).await.unwrap();
        mark_embedding_generated(&pool, article.id).await.unwrap();
        let a = get_article(&pool, article.id).await.unwrap();
        assert_eq!(a.embedding_generated, 1);
    }

    #[tokio::test]
    async fn date_range_exact_boundary() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();
        // published_at is "2024-01-01 00:00:00"

        // Exact match on from=to should include the article (inclusive)
        let results =
            search_articles_by_date_range(&pool, "2024-01-01 00:00:00", "2024-01-01 00:00:00")
                .await
                .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn recent_articles_respects_limit() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;

        for i in 0..5 {
            let a = NewArticle {
                guid: Some(format!("guid-{i}")),
                url: format!("https://example.com/article-{i}"),
                title: format!("Article {i}"),
                ..sample_article(feed_id)
            };
            insert_article(&pool, &a).await.unwrap();
        }

        let recent = get_recent_articles(&pool, 3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn recent_articles_limit_zero() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();

        let recent = get_recent_articles(&pool, 0).await.unwrap();
        assert!(recent.is_empty());
    }

    #[tokio::test]
    async fn article_with_null_guid() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let a = NewArticle {
            guid: None,
            ..sample_article(feed_id)
        };
        let article = insert_article(&pool, &a).await.unwrap();
        assert!(article.guid.is_none());
    }

    #[tokio::test]
    async fn article_default_flags() {
        let pool = test_pool().await;
        let feed_id = setup_feed(&pool).await;
        let article = insert_article(&pool, &sample_article(feed_id))
            .await
            .unwrap();
        assert_eq!(article.word_count, None);
        assert_eq!(article.content_extracted, 0);
        assert_eq!(article.embedding_generated, 0);
        assert_eq!(article.tags_generated, 0);
        assert!(!article.created_at.is_empty());
        assert!(!article.updated_at.is_empty());
    }
}
