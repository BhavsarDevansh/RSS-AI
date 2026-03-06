/// Data types for the OpenAI-compatible embeddings API.
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct EmbeddingRequest<'a> {
    pub model: &'a str,
    pub input: EmbeddingInput<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum EmbeddingInput<'a> {
    Single(&'a str),
    Batch(Vec<&'a str>),
}

#[derive(Deserialize)]
pub(crate) struct EmbeddingResponse {
    pub data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
pub(crate) struct EmbeddingData {
    pub embedding: Vec<f32>,
}

/// Result of processing pending articles for embedding generation.
pub struct EmbeddingBatchResult {
    pub processed: usize,
    pub failed: usize,
    pub embeddings: Vec<(i64, Vec<f32>)>,
}
