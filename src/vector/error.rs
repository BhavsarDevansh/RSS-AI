/// Error types for the vector similarity search module.
#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    #[error("article {0} already exists in the index")]
    DuplicateEntry(i64),

    #[error("article {0} not found in the index")]
    NotFound(i64),

    #[error("index persistence error: {0}")]
    Persistence(String),

    #[error("lock poisoned: {0}")]
    LockPoisoned(String),

    #[error("embedding error: {0}")]
    Embedding(#[from] crate::embeddings::EmbeddingError),

    #[error("full-text search error: {0}")]
    SearchIndex(#[from] crate::search::SearchError),
}
