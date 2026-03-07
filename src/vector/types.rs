/// Data types for vector similarity search.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// A single vector search result.
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub article_id: i64,
    /// Cosine similarity score (0.0 to 1.0).
    pub similarity: f32,
}

/// A result from hybrid (keyword + semantic) search.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    pub article_id: i64,
    pub combined_score: f32,
    pub keyword_score: Option<f32>,
    pub semantic_score: Option<f32>,
    pub title: String,
    pub snippet: String,
}

/// Persisted metadata for the vector index.
#[derive(Serialize, Deserialize)]
pub(crate) struct IndexState {
    pub dimensions: usize,
    pub next_internal_id: usize,
    pub id_to_internal: HashMap<i64, usize>,
    pub internal_to_id: HashMap<usize, i64>,
    pub tombstones: HashSet<usize>,
}
