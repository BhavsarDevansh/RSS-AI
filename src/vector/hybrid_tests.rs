use super::hybrid::hybrid_search;
use super::index::VectorIndex;
use crate::embeddings::EmbeddingClient;
use crate::search::{ArticleIndexData, SearchIndex, index_article};
use crate::test_utils::mock_llm::MockLlmServer;

fn make_embedding_response(vector: &[f32]) -> String {
    serde_json::json!({
        "object": "list",
        "data": [{
            "object": "embedding",
            "embedding": vector,
            "index": 0
        }],
        "model": "test",
        "usage": { "prompt_tokens": 1, "total_tokens": 1 }
    })
    .to_string()
}

#[tokio::test]
async fn hybrid_combines_keyword_and_semantic() {
    let server = MockLlmServer::start().await;
    let query_vec = vec![1.0, 0.0, 0.0];
    server
        .mount_embeddings(&make_embedding_response(&query_vec))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    // Keyword index
    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();
    let mut writer = si.writer().unwrap();
    let tags = vec![];
    let data = ArticleIndexData {
        article_id: 1,
        title: "Rust Programming",
        content: Some("Rust is a systems programming language"),
        summary: None,
        author: None,
        tags: &tags,
        feed_title: Some("Blog"),
        published_at: Some("2025-06-15T12:00:00+00:00"),
    };
    index_article(&mut writer, &si.fields(), &data).unwrap();
    si.reader().reload().unwrap();

    // Vector index
    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();
    vi.add(1, &[1.0, 0.0, 0.0]).unwrap();

    let results = hybrid_search("rust", &si, &vi, &client, 10).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].article_id, 1);
    assert!(results[0].keyword_score.is_some());
    assert!(results[0].semantic_score.is_some());
    // RRF from both systems should give higher score than either alone
    assert!(results[0].combined_score > 0.0);
}

#[tokio::test]
async fn keyword_only_result_has_no_semantic_score() {
    let server = MockLlmServer::start().await;
    // Query embedding far from any vector in the index
    let query_vec = vec![0.0, 0.0, 1.0];
    server
        .mount_embeddings(&make_embedding_response(&query_vec))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();
    let mut writer = si.writer().unwrap();
    let tags = vec![];
    index_article(
        &mut writer,
        &si.fields(),
        &ArticleIndexData {
            article_id: 1,
            title: "Rust Programming",
            content: Some("Rust is great"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: None,
        },
    )
    .unwrap();
    si.reader().reload().unwrap();

    // Empty vector index
    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();

    let results = hybrid_search("rust", &si, &vi, &client, 10).await.unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].keyword_score.is_some());
    assert!(results[0].semantic_score.is_none());
}

#[tokio::test]
async fn semantic_only_result_has_no_keyword_score() {
    let server = MockLlmServer::start().await;
    let query_vec = vec![1.0, 0.0, 0.0];
    server
        .mount_embeddings(&make_embedding_response(&query_vec))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    // Empty keyword index
    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();

    // Vector index with one article
    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();
    vi.add(1, &[1.0, 0.0, 0.0]).unwrap();

    let results = hybrid_search("anything", &si, &vi, &client, 10)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].keyword_score.is_none());
    assert!(results[0].semantic_score.is_some());
}

#[tokio::test]
async fn rrf_ranks_dual_hits_higher() {
    let server = MockLlmServer::start().await;
    let query_vec = vec![1.0, 0.0, 0.0];
    server
        .mount_embeddings(&make_embedding_response(&query_vec))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();
    let mut writer = si.writer().unwrap();
    let tags = vec![];

    // Article 1: appears in both keyword and vector
    index_article(
        &mut writer,
        &si.fields(),
        &ArticleIndexData {
            article_id: 1,
            title: "Rust Programming",
            content: Some("Rust programming language"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: None,
        },
    )
    .unwrap();

    // Article 2: appears only in keyword
    index_article(
        &mut writer,
        &si.fields(),
        &ArticleIndexData {
            article_id: 2,
            title: "Rust Programming Guide",
            content: Some("Another Rust programming article"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: None,
        },
    )
    .unwrap();
    si.reader().reload().unwrap();

    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();
    // Only article 1 in vector index
    vi.add(1, &[1.0, 0.0, 0.0]).unwrap();

    let results = hybrid_search("rust programming", &si, &vi, &client, 10)
        .await
        .unwrap();

    assert!(results.len() >= 2);
    // Article 1 (in both) should rank higher than article 2 (keyword only)
    assert_eq!(results[0].article_id, 1);
    assert!(results[0].combined_score > results[1].combined_score);
}

#[tokio::test]
async fn empty_results() {
    let server = MockLlmServer::start().await;
    server
        .mount_embeddings(&make_embedding_response(&[0.0, 0.0, 1.0]))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();

    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();

    let results = hybrid_search("nonexistent", &si, &vi, &client, 10)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn limit_zero_returns_empty() {
    let server = MockLlmServer::start().await;
    server
        .mount_embeddings(&make_embedding_response(&[1.0, 0.0, 0.0]))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();

    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();
    vi.add(1, &[1.0, 0.0, 0.0]).unwrap();

    let results = hybrid_search("test", &si, &vi, &client, 0).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn hybrid_result_has_title_from_keyword() {
    let server = MockLlmServer::start().await;
    server
        .mount_embeddings(&make_embedding_response(&[1.0, 0.0, 0.0]))
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3).unwrap();

    let search_dir = tempfile::tempdir().unwrap();
    let si = SearchIndex::open_at(search_dir.path()).unwrap();
    let mut writer = si.writer().unwrap();
    let tags = vec![];
    index_article(
        &mut writer,
        &si.fields(),
        &ArticleIndexData {
            article_id: 1,
            title: "Important Title",
            content: Some("test content here"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: None,
        },
    )
    .unwrap();
    si.reader().reload().unwrap();

    let vec_dir = tempfile::tempdir().unwrap();
    let vi = VectorIndex::open_at(vec_dir.path(), 3).unwrap();
    vi.add(1, &[1.0, 0.0, 0.0]).unwrap();

    let results = hybrid_search("test", &si, &vi, &client, 10).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Important Title");
}
