use super::client::EmbeddingClient;
use super::pipeline::process_pending_articles;
use crate::db;
use crate::test_utils::mock_llm::MockLlmServer;

#[tokio::test]
async fn embedding_generated_flag_updated() {
    let pool = crate::test_utils::db::test_pool().await;
    let server = MockLlmServer::start().await;
    server.expect_embedding(vec![0.1, 0.2, 0.3]).await;

    let feed = db::feeds::add_feed(&pool, "https://example.com/rss", None)
        .await
        .unwrap();
    let article = db::articles::insert_article(
        &pool,
        &db::NewArticle {
            feed_id: feed.id,
            guid: Some("g1".to_string()),
            url: "https://example.com/post".to_string(),
            title: "Test Article".to_string(),
            author: None,
            published_at: None,
            summary: None,
            content: None,
            content_hash: None,
        },
    )
    .await
    .unwrap();

    db::articles::update_article_content(&pool, article.id, "Some content here", "hash1")
        .await
        .unwrap();

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();
    let result = process_pending_articles(&client, &pool, Some(10))
        .await
        .unwrap();

    assert_eq!(result.processed, 1);
    assert_eq!(result.failed, 0);
    assert_eq!(result.embeddings.len(), 1);
    assert_eq!(result.embeddings[0].0, article.id);
    assert_eq!(result.embeddings[0].1.len(), 3);

    let updated = db::articles::get_article(&pool, article.id).await.unwrap();
    assert_eq!(updated.embedding_generated, 1);

    // Running again should find nothing pending
    let result2 = process_pending_articles(&client, &pool, Some(10))
        .await
        .unwrap();
    assert_eq!(result2.processed, 0);
}
