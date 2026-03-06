use super::client::EmbeddingClient;
use super::error::EmbeddingError;
use crate::test_utils::mock_llm::MockLlmServer;

fn make_embedding_response(vectors: &[Vec<f32>]) -> String {
    let data: Vec<serde_json::Value> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            serde_json::json!({
                "object": "embedding",
                "embedding": v,
                "index": i
            })
        })
        .collect();

    serde_json::json!({
        "object": "list",
        "data": data,
        "model": "test",
        "usage": { "prompt_tokens": 1, "total_tokens": 1 }
    })
    .to_string()
}

#[tokio::test]
async fn embed_text_with_mock_server() {
    let server = MockLlmServer::start().await;
    let dims = 4;
    let expected_vec = vec![0.1, 0.2, 0.3, 0.4];
    server.expect_embedding(expected_vec.clone()).await;

    let client = EmbeddingClient::with_url(&server.url(), "test-model", dims);
    let result = client.embed_text("hello world").await.unwrap();

    assert_eq!(result.len(), dims);
    assert_eq!(result, expected_vec);

    let body = server.last_request_body("/v1/embeddings").await;
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["input"], "hello world");
}

#[tokio::test]
async fn text_truncation() {
    let server = MockLlmServer::start().await;
    server.expect_embedding(vec![0.1, 0.2, 0.3]).await;

    let mut client = EmbeddingClient::with_url(&server.url(), "test", 3);
    client.max_input_chars = 20;

    let long_text = "a".repeat(100);
    let result = client.embed_text(&long_text).await.unwrap();
    assert_eq!(result.len(), 3);

    let body = server.last_request_body("/v1/embeddings").await;
    let sent_input = body["input"].as_str().unwrap();
    assert_eq!(sent_input.len(), 20);
}

#[tokio::test]
async fn batch_embedding() {
    let server = MockLlmServer::start().await;
    let vecs = vec![vec![0.1, 0.2], vec![0.3, 0.4], vec![0.5, 0.6]];
    let response = make_embedding_response(&vecs);
    server.mount_embeddings(&response).await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 2);
    let texts = vec!["first", "second", "third"];
    let results = client.embed_batch(&texts).await.unwrap();

    assert_eq!(results.len(), 3);
    assert_eq!(results[0], vec![0.1, 0.2]);
    assert_eq!(results[2], vec![0.5, 0.6]);
}

#[tokio::test]
async fn wrong_dimensions_rejected() {
    let server = MockLlmServer::start().await;
    server.expect_embedding(vec![0.1, 0.2, 0.3]).await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 5);
    let err = client.embed_text("hello").await.unwrap_err();

    assert!(
        matches!(
            err,
            EmbeddingError::DimensionMismatch {
                expected: 5,
                actual: 3
            }
        ),
        "expected DimensionMismatch, got: {err}"
    );
}

#[tokio::test]
async fn server_error_returns_error() {
    let server = MockLlmServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/embeddings"))
        .respond_with(
            wiremock::ResponseTemplate::new(400).set_body_string(r#"{"error":"bad request"}"#),
        )
        .mount(server.server())
        .await;

    let client = EmbeddingClient::with_url(&server.url(), "test", 3);
    let err = client.embed_text("hello").await.unwrap_err();

    assert!(
        matches!(err, EmbeddingError::ApiError { status: 400, .. }),
        "expected ApiError 400, got: {err}"
    );
}
