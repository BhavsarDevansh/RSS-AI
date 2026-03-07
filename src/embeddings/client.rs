/// HTTP client for the OpenAI-compatible embeddings API.
use std::time::Duration;

use tokio::time::sleep;

use super::error::EmbeddingError;
use super::text::{DEFAULT_MAX_INPUT_CHARS, prepare_input};
use super::types::{EmbeddingInput, EmbeddingRequest, EmbeddingResponse};
use crate::config::Config;

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 500;

pub struct EmbeddingClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    expected_dimensions: usize,
    pub(crate) max_input_chars: usize,
}

impl EmbeddingClient {
    pub fn new(config: &Config) -> Result<Self, EmbeddingError> {
        let base = config.llm.api_base_url.trim_end_matches('/');
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            client,
            endpoint: format!("{base}/v1/embeddings"),
            model: config.llm.embedding_model.clone(),
            expected_dimensions: config.llm.embedding_dimensions as usize,
            max_input_chars: DEFAULT_MAX_INPUT_CHARS,
        })
    }

    /// Create a client pointing at a specific URL (useful for tests).
    pub fn with_url(
        base_url: &str,
        model: &str,
        dimensions: usize,
    ) -> Result<Self, EmbeddingError> {
        let base = base_url.trim_end_matches('/');
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            client,
            endpoint: format!("{base}/v1/embeddings"),
            model: model.to_string(),
            expected_dimensions: dimensions,
            max_input_chars: DEFAULT_MAX_INPUT_CHARS,
        })
    }

    /// Embed a single text string. Retries transient failures up to 3 times.
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let truncated = prepare_input(text, self.max_input_chars);
        let body = EmbeddingRequest {
            model: &self.model,
            input: EmbeddingInput::Single(&truncated),
        };

        let response = self.send_with_retry(&body).await?;

        let first = response
            .data
            .into_iter()
            .next()
            .ok_or_else(|| EmbeddingError::MalformedResponse("empty data array".to_string()))?;

        self.validate_dimensions(&first.embedding)?;
        Ok(first.embedding)
    }

    /// Embed multiple texts in one API call. Falls back to sequential calls
    /// if the batch response count doesn't match.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if texts.len() == 1 {
            return Ok(vec![self.embed_text(texts[0]).await?]);
        }

        let truncated: Vec<_> = texts
            .iter()
            .map(|t| prepare_input(t, self.max_input_chars))
            .collect();
        let refs: Vec<&str> = truncated.iter().map(|c| c.as_ref()).collect();

        let body = EmbeddingRequest {
            model: &self.model,
            input: EmbeddingInput::Batch(refs),
        };

        match self.send_with_retry(&body).await {
            Ok(response) if response.data.len() == texts.len() => {
                let mut results = Vec::with_capacity(texts.len());
                for item in response.data {
                    self.validate_dimensions(&item.embedding)?;
                    results.push(item.embedding);
                }
                Ok(results)
            }
            // Batch not supported or count mismatch — fall back to sequential
            Ok(_) | Err(_) => {
                let mut results = Vec::with_capacity(texts.len());
                for text in texts {
                    results.push(self.embed_text(text).await?);
                }
                Ok(results)
            }
        }
    }

    fn validate_dimensions(&self, embedding: &[f32]) -> Result<(), EmbeddingError> {
        if embedding.len() != self.expected_dimensions {
            return Err(EmbeddingError::DimensionMismatch {
                expected: self.expected_dimensions,
                actual: embedding.len(),
            });
        }
        Ok(())
    }

    async fn send_with_retry(
        &self,
        body: &EmbeddingRequest<'_>,
    ) -> Result<EmbeddingResponse, EmbeddingError> {
        let mut last_err = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let backoff = Duration::from_millis(INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1));
                sleep(backoff).await;
            }

            let result = self.client.post(&self.endpoint).json(body).send().await;

            let response = match result {
                Ok(r) => r,
                Err(e) => {
                    if e.is_connect() || e.is_timeout() {
                        last_err = Some(EmbeddingError::Request(e));
                        continue;
                    }
                    return Err(EmbeddingError::Request(e));
                }
            };

            let status = response.status().as_u16();

            if response.status().is_success() {
                let parsed: EmbeddingResponse = response
                    .json()
                    .await
                    .map_err(|e| EmbeddingError::MalformedResponse(e.to_string()))?;
                return Ok(parsed);
            }

            // Retry on 5xx and 429
            if status >= 500 || status == 429 {
                let body_text = response.text().await.unwrap_or_default();
                last_err = Some(EmbeddingError::ApiError {
                    status,
                    body: body_text,
                });
                continue;
            }

            // Non-retryable client error
            let body_text = response.text().await.unwrap_or_default();
            return Err(EmbeddingError::ApiError {
                status,
                body: body_text,
            });
        }

        Err(last_err.unwrap_or_else(|| {
            EmbeddingError::MalformedResponse("max retries exceeded".to_string())
        }))
    }
}
