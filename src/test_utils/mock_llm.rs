use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A mock LLM API server (OpenAI-compatible format).
pub struct MockLlmServer {
    server: MockServer,
}

impl MockLlmServer {
    /// Start a new mock LLM server.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// Base URL of the mock server.
    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// Mount a chat completion response at `/v1/chat/completions`.
    pub async fn mount_chat_completion(&self, response_json: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(response_json)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&self.server)
            .await;
    }

    /// Mount an embeddings response at `/v1/embeddings`.
    pub async fn mount_embeddings(&self, response_json: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(response_json)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&self.server)
            .await;
    }

    /// Mount a 429 rate limit error at the given path.
    pub async fn mount_rate_limit(&self, api_path: &str, error_json: &str) {
        Mock::given(method("POST"))
            .and(path(api_path))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_string(error_json)
                    .insert_header("content-type", "application/json")
                    .insert_header("retry-after", "60"),
            )
            .mount(&self.server)
            .await;
    }

    // ── Convenience helpers ─────────────────────────────────────────

    /// Mount a chat completion that returns `text` as the assistant message.
    pub async fn expect_chat_response(&self, text: &str) {
        let json = serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop"
            }]
        });
        self.mount_chat_completion(&json.to_string()).await;
    }

    /// Mount a chat completion whose content is a stringified JSON value.
    pub async fn expect_chat_json(&self, value: Value) {
        self.expect_chat_response(&value.to_string()).await;
    }

    /// Mount an embeddings response returning the given vector.
    pub async fn expect_embedding(&self, vector: Vec<f32>) {
        let json = serde_json::json!({
            "object": "list",
            "data": [{
                "object": "embedding",
                "embedding": vector,
                "index": 0
            }],
            "model": "test",
            "usage": { "prompt_tokens": 1, "total_tokens": 1 }
        });
        self.mount_embeddings(&json.to_string()).await;
    }

    /// Mount an embeddings response with a deterministic vector of the given size.
    ///
    /// Each component is `(i as f32 + 1.0) / dimensions`, producing a
    /// reproducible unit-ish vector useful for snapshot-style assertions.
    pub async fn expect_embedding_dimensions(&self, dimensions: usize) {
        let vector: Vec<f32> = (0..dimensions)
            .map(|i| (i as f32 + 1.0) / dimensions as f32)
            .collect();
        self.expect_embedding(vector).await;
    }

    /// Assert that `endpoint` (e.g. `"/v1/chat/completions"`) was called
    /// exactly `expected` times.
    pub async fn assert_called(&self, endpoint: &str, expected: usize) {
        let requests = self.server.received_requests().await.unwrap_or_default();
        let count = requests.iter().filter(|r| r.url.path() == endpoint).count();
        assert_eq!(
            count, expected,
            "expected {expected} calls to {endpoint}, got {count}"
        );
    }

    /// Return the body of the last request sent to `endpoint`.
    ///
    /// # Panics
    /// Panics if no request was received for the endpoint.
    pub async fn last_request_body(&self, endpoint: &str) -> Value {
        let requests = self.server.received_requests().await.unwrap_or_default();
        let req = requests
            .iter()
            .rev()
            .find(|r| r.url.path() == endpoint)
            .unwrap_or_else(|| panic!("no requests received for {endpoint}"));
        serde_json::from_slice(&req.body)
            .unwrap_or_else(|e| panic!("failed to parse request body as JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_chat_completion() {
        let server = MockLlmServer::start().await;
        let json = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        server.mount_chat_completion(json).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/chat/completions", server.url()))
            .json(&serde_json::json!({"model": "test", "messages": []}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "hello");
    }

    #[tokio::test]
    async fn serves_embeddings() {
        let server = MockLlmServer::start().await;
        let json = r#"{"object":"list","data":[{"object":"embedding","embedding":[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8],"index":0}]}"#;
        server.mount_embeddings(json).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/embeddings", server.url()))
            .json(&serde_json::json!({"model": "test", "input": "hello"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["data"][0]["embedding"].as_array().unwrap().len(), 8);
    }

    #[tokio::test]
    async fn returns_rate_limit() {
        let server = MockLlmServer::start().await;
        let error_json = r#"{"error":{"message":"Rate limit exceeded","type":"rate_limit_error"}}"#;
        server
            .mount_rate_limit("/v1/chat/completions", error_json)
            .await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/chat/completions", server.url()))
            .json(&serde_json::json!({"model": "test", "messages": []}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 429);
    }

    #[tokio::test]
    async fn expect_chat_response_wraps_text() {
        let server = MockLlmServer::start().await;
        server.expect_chat_response("world").await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/chat/completions", server.url()))
            .json(&serde_json::json!({"model": "test", "messages": []}))
            .send()
            .await
            .unwrap();

        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "world");
        assert_eq!(body["choices"][0]["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn expect_chat_json_stringifies_value() {
        let server = MockLlmServer::start().await;
        let payload = serde_json::json!({"tags": ["rust", "ai"]});
        server.expect_chat_json(payload.clone()).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/chat/completions", server.url()))
            .json(&serde_json::json!({"model": "test", "messages": []}))
            .send()
            .await
            .unwrap();

        let body: Value = resp.json().await.unwrap();
        let content_str = body["choices"][0]["message"]["content"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(content_str).unwrap();
        assert_eq!(parsed, payload);
    }

    #[tokio::test]
    async fn expect_embedding_returns_vector() {
        let server = MockLlmServer::start().await;
        let vec = vec![0.1, 0.2, 0.3];
        server.expect_embedding(vec.clone()).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/embeddings", server.url()))
            .json(&serde_json::json!({"model": "test", "input": "hi"}))
            .send()
            .await
            .unwrap();

        let body: Value = resp.json().await.unwrap();
        let emb: Vec<f64> = body["data"][0]["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(emb.len(), 3);
    }

    #[tokio::test]
    async fn expect_embedding_dimensions_generates_vector() {
        let server = MockLlmServer::start().await;
        server.expect_embedding_dimensions(128).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/v1/embeddings", server.url()))
            .json(&serde_json::json!({"model": "test", "input": "hi"}))
            .send()
            .await
            .unwrap();

        let body: Value = resp.json().await.unwrap();
        let emb = body["data"][0]["embedding"].as_array().unwrap();
        assert_eq!(emb.len(), 128);
        // First element should be 1.0/128.0
        let first = emb[0].as_f64().unwrap();
        assert!((first - 1.0 / 128.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn assert_called_tracks_counts() {
        let server = MockLlmServer::start().await;
        server.expect_chat_response("ok").await;

        let client = reqwest::Client::new();
        for _ in 0..3 {
            client
                .post(format!("{}/v1/chat/completions", server.url()))
                .json(&serde_json::json!({"model": "test", "messages": []}))
                .send()
                .await
                .unwrap();
        }

        server.assert_called("/v1/chat/completions", 3).await;
        server.assert_called("/v1/embeddings", 0).await;
    }

    #[tokio::test]
    async fn last_request_body_returns_payload() {
        let server = MockLlmServer::start().await;
        server.expect_chat_response("ok").await;

        let client = reqwest::Client::new();
        client
            .post(format!("{}/v1/chat/completions", server.url()))
            .json(&serde_json::json!({"model": "gpt-4", "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();

        let body = server.last_request_body("/v1/chat/completions").await;
        assert_eq!(body["model"], "gpt-4");
        assert_eq!(body["messages"][0]["content"], "hi");
    }
}
