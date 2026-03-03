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
}
