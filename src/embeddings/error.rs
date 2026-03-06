/// Error types for the embedding module.
use crate::db;

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("API error (status {status}): {body}")]
    ApiError { status: u16, body: String },

    #[error("malformed API response: {0}")]
    MalformedResponse(String),

    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    #[error("database error: {0}")]
    Db(#[from] db::DbError),
}
