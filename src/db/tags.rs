/// Tag operations and article-tag associations.
use sqlx::SqlitePool;

use super::DbError;
use super::models::{Tag, TagWithConfidence};

/// Get or create a tag by name (case-insensitive, stored lowercase).
pub async fn get_or_create_tag(pool: &SqlitePool, name: &str) -> Result<Tag, DbError> {
    let lower = name.to_lowercase();

    sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
        .bind(&lower)
        .execute(pool)
        .await?;

    let tag = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE name = ?")
        .bind(&lower)
        .fetch_one(pool)
        .await?;

    Ok(tag)
}

/// Add tags with confidence scores to an article (in a transaction).
pub async fn add_tags_to_article(
    pool: &SqlitePool,
    article_id: i64,
    tags: &[TagWithConfidence],
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;

    for twc in tags {
        let lower = twc.name.to_lowercase();

        sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
            .bind(&lower)
            .execute(&mut *tx)
            .await?;

        let tag: Tag = sqlx::query_as("SELECT * FROM tags WHERE name = ?")
            .bind(&lower)
            .fetch_one(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT OR IGNORE INTO article_tags (article_id, tag_id, confidence) VALUES (?, ?, ?)",
        )
        .bind(article_id)
        .bind(tag.id)
        .bind(twc.confidence)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Get all tags for an article (with confidence).
pub async fn get_tags_for_article(
    pool: &SqlitePool,
    article_id: i64,
) -> Result<Vec<(Tag, f64)>, DbError> {
    let rows: Vec<(i64, String, i64, String, f64)> = sqlx::query_as(
        "SELECT t.id, t.name, t.article_count, t.created_at, at.confidence
         FROM tags t
         JOIN article_tags at ON at.tag_id = t.id
         WHERE at.article_id = ?
         ORDER BY at.confidence DESC",
    )
    .bind(article_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, name, article_count, created_at, confidence)| {
            (
                Tag {
                    id,
                    name,
                    article_count,
                    created_at,
                },
                confidence,
            )
        })
        .collect())
}

/// Get all article IDs with a given tag name.
pub async fn get_articles_by_tag(pool: &SqlitePool, tag_name: &str) -> Result<Vec<i64>, DbError> {
    let lower = tag_name.to_lowercase();
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT at.article_id FROM article_tags at
         JOIN tags t ON t.id = at.tag_id
         WHERE t.name = ?",
    )
    .bind(&lower)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// List all tags ordered by article count descending.
pub async fn list_all_tags(pool: &SqlitePool) -> Result<Vec<Tag>, DbError> {
    let tags = sqlx::query_as::<_, Tag>("SELECT * FROM tags ORDER BY article_count DESC, name ASC")
        .fetch_all(pool)
        .await?;
    Ok(tags)
}

/// Get the top N tags by article count.
pub async fn get_top_tags(pool: &SqlitePool, limit: i64) -> Result<Vec<Tag>, DbError> {
    let tags = sqlx::query_as::<_, Tag>(
        "SELECT * FROM tags ORDER BY article_count DESC, name ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{articles, feeds};
    use crate::test_utils::db::test_pool;

    async fn setup_article(pool: &SqlitePool) -> i64 {
        let feed = feeds::add_feed(pool, "https://example.com/rss", None)
            .await
            .unwrap();
        articles::insert_article(
            pool,
            &super::super::models::NewArticle {
                feed_id: feed.id,
                guid: Some("guid-1".to_string()),
                url: "https://example.com/article-1".to_string(),
                title: "Test".to_string(),
                author: None,
                published_at: None,
                summary: None,
                content: None,
                content_hash: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn get_or_create_idempotent() {
        let pool = test_pool().await;
        let t1 = get_or_create_tag(&pool, "Rust").await.unwrap();
        let t2 = get_or_create_tag(&pool, "rust").await.unwrap();
        assert_eq!(t1.id, t2.id);
        assert_eq!(t1.name, "rust");
    }

    #[tokio::test]
    async fn add_tags_and_article_count_trigger() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        add_tags_to_article(
            &pool,
            article_id,
            &[
                TagWithConfidence {
                    name: "rust".to_string(),
                    confidence: 0.95,
                },
                TagWithConfidence {
                    name: "programming".to_string(),
                    confidence: 0.8,
                },
            ],
        )
        .await
        .unwrap();

        let tags = get_tags_for_article(&pool, article_id).await.unwrap();
        assert_eq!(tags.len(), 2);
        // Highest confidence first
        assert_eq!(tags[0].0.name, "rust");
        assert!((tags[0].1 - 0.95).abs() < f64::EPSILON);

        // Trigger should have updated article_count
        let rust_tag = get_or_create_tag(&pool, "rust").await.unwrap();
        assert_eq!(rust_tag.article_count, 1);
    }

    #[tokio::test]
    async fn get_articles_by_tag_works() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        add_tags_to_article(
            &pool,
            article_id,
            &[TagWithConfidence {
                name: "rust".to_string(),
                confidence: 1.0,
            }],
        )
        .await
        .unwrap();

        let ids = get_articles_by_tag(&pool, "Rust").await.unwrap();
        assert_eq!(ids, vec![article_id]);
    }

    #[tokio::test]
    async fn top_tags() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        add_tags_to_article(
            &pool,
            article_id,
            &[
                TagWithConfidence {
                    name: "a".to_string(),
                    confidence: 1.0,
                },
                TagWithConfidence {
                    name: "b".to_string(),
                    confidence: 1.0,
                },
            ],
        )
        .await
        .unwrap();

        let top = get_top_tags(&pool, 1).await.unwrap();
        assert_eq!(top.len(), 1);
    }

    #[tokio::test]
    async fn cascade_delete_updates_tag_count() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        add_tags_to_article(
            &pool,
            article_id,
            &[TagWithConfidence {
                name: "rust".to_string(),
                confidence: 1.0,
            }],
        )
        .await
        .unwrap();

        // Verify count is 1
        let tag = get_or_create_tag(&pool, "rust").await.unwrap();
        assert_eq!(tag.article_count, 1);

        // Delete the feed (cascades to articles → article_tags)
        feeds::remove_feed(&pool, 1).await.unwrap();

        // Tag should still exist but count should be 0
        let tag = get_or_create_tag(&pool, "rust").await.unwrap();
        assert_eq!(tag.article_count, 0);
    }

    #[tokio::test]
    async fn add_tags_empty_slice() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;
        // Should succeed as a no-op
        add_tags_to_article(&pool, article_id, &[]).await.unwrap();
        let tags = get_tags_for_article(&pool, article_id).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn add_duplicate_tag_to_article_is_idempotent() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        let tag = TagWithConfidence {
            name: "rust".to_string(),
            confidence: 0.9,
        };

        add_tags_to_article(&pool, article_id, &[tag.clone()])
            .await
            .unwrap();
        // Adding same tag again should be silently ignored (INSERT OR IGNORE)
        add_tags_to_article(&pool, article_id, &[tag])
            .await
            .unwrap();

        let tags = get_tags_for_article(&pool, article_id).await.unwrap();
        assert_eq!(tags.len(), 1);

        // article_count should still be 1, not 2
        let t = get_or_create_tag(&pool, "rust").await.unwrap();
        assert_eq!(t.article_count, 1);
    }

    #[tokio::test]
    async fn add_tags_with_duplicate_names_in_same_call() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;

        add_tags_to_article(
            &pool,
            article_id,
            &[
                TagWithConfidence {
                    name: "rust".to_string(),
                    confidence: 0.9,
                },
                TagWithConfidence {
                    name: "Rust".to_string(),
                    confidence: 0.5,
                },
            ],
        )
        .await
        .unwrap();

        // Should only have one tag (case-insensitive dedup via INSERT OR IGNORE)
        let tags = get_tags_for_article(&pool, article_id).await.unwrap();
        assert_eq!(tags.len(), 1);
    }

    #[tokio::test]
    async fn get_tags_for_article_empty() {
        let pool = test_pool().await;
        let article_id = setup_article(&pool).await;
        let tags = get_tags_for_article(&pool, article_id).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn get_tags_for_nonexistent_article() {
        let pool = test_pool().await;
        let tags = get_tags_for_article(&pool, 999).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn get_articles_by_nonexistent_tag() {
        let pool = test_pool().await;
        let ids = get_articles_by_tag(&pool, "nonexistent").await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn list_all_tags_empty() {
        let pool = test_pool().await;
        let tags = list_all_tags(&pool).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn list_all_tags_ordering() {
        let pool = test_pool().await;

        // Create two articles so we can give different tags different counts
        let feed = feeds::add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        let a1 = articles::insert_article(
            &pool,
            &super::super::models::NewArticle {
                feed_id: feed.id,
                guid: Some("g1".to_string()),
                url: "https://example.com/a1".to_string(),
                title: "A1".to_string(),
                author: None,
                published_at: None,
                summary: None,
                content: None,
                content_hash: None,
            },
        )
        .await
        .unwrap();
        let a2 = articles::insert_article(
            &pool,
            &super::super::models::NewArticle {
                feed_id: feed.id,
                guid: Some("g2".to_string()),
                url: "https://example.com/a2".to_string(),
                title: "A2".to_string(),
                author: None,
                published_at: None,
                summary: None,
                content: None,
                content_hash: None,
            },
        )
        .await
        .unwrap();

        // "popular" tag on both articles, "niche" on only one
        add_tags_to_article(
            &pool,
            a1.id,
            &[
                TagWithConfidence {
                    name: "popular".to_string(),
                    confidence: 1.0,
                },
                TagWithConfidence {
                    name: "niche".to_string(),
                    confidence: 1.0,
                },
            ],
        )
        .await
        .unwrap();
        add_tags_to_article(
            &pool,
            a2.id,
            &[TagWithConfidence {
                name: "popular".to_string(),
                confidence: 1.0,
            }],
        )
        .await
        .unwrap();

        let all = list_all_tags(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
        // "popular" (count=2) should come before "niche" (count=1)
        assert_eq!(all[0].name, "popular");
        assert_eq!(all[0].article_count, 2);
        assert_eq!(all[1].name, "niche");
        assert_eq!(all[1].article_count, 1);
    }

    #[tokio::test]
    async fn tag_article_count_multiple_articles() {
        let pool = test_pool().await;
        let feed = feeds::add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();

        for i in 0..3 {
            let a = articles::insert_article(
                &pool,
                &super::super::models::NewArticle {
                    feed_id: feed.id,
                    guid: Some(format!("g-{i}")),
                    url: format!("https://example.com/a-{i}"),
                    title: format!("A{i}"),
                    author: None,
                    published_at: None,
                    summary: None,
                    content: None,
                    content_hash: None,
                },
            )
            .await
            .unwrap();
            add_tags_to_article(
                &pool,
                a.id,
                &[TagWithConfidence {
                    name: "shared".to_string(),
                    confidence: 1.0,
                }],
            )
            .await
            .unwrap();
        }

        let tag = get_or_create_tag(&pool, "shared").await.unwrap();
        assert_eq!(tag.article_count, 3);
    }

    #[tokio::test]
    async fn get_top_tags_limit_exceeds_total() {
        let pool = test_pool().await;
        get_or_create_tag(&pool, "only-one").await.unwrap();
        let top = get_top_tags(&pool, 100).await.unwrap();
        assert_eq!(top.len(), 1);
    }

    #[tokio::test]
    async fn unicode_tag_names() {
        let pool = test_pool().await;
        let tag = get_or_create_tag(&pool, "机器学习").await.unwrap();
        assert_eq!(tag.name, "机器学习");

        let tag2 = get_or_create_tag(&pool, "机器学习").await.unwrap();
        assert_eq!(tag.id, tag2.id);
    }
}
