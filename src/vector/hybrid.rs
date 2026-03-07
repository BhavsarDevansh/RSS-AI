/// Hybrid search combining keyword (Tantivy) and semantic (HNSW) results.
use std::collections::HashMap;

use super::error::VectorError;
use super::index::VectorIndex;
use super::types::HybridSearchResult;
use crate::embeddings::EmbeddingClient;
use crate::search::{SearchIndex, SearchOptions};

/// Standard RRF constant (controls how much lower-ranked results contribute).
const RRF_K: f32 = 60.0;

/// Maximum over-fetch multiplier to prevent unreasonable memory use.
const MAX_OVERFETCH_LIMIT: usize = 1_000;

/// Perform hybrid search combining keyword and semantic results using
/// Reciprocal Rank Fusion (RRF).
///
/// 1. Runs keyword search via the Tantivy index
/// 2. Embeds the query and runs vector search via the HNSW index
/// 3. Merges results using RRF: `score = Σ 1/(k + rank)` per system
pub async fn hybrid_search(
    query: &str,
    search_index: &SearchIndex,
    vector_index: &VectorIndex,
    embedding_client: &EmbeddingClient,
    limit: usize,
) -> Result<Vec<HybridSearchResult>, VectorError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let overfetch = (limit.saturating_mul(2)).min(MAX_OVERFETCH_LIMIT);

    // Keyword search (over-fetch for better RRF coverage)
    let search_options = SearchOptions {
        limit: overfetch,
        ..Default::default()
    };
    let keyword_results = search_index.search(query, &search_options)?;

    // Semantic search
    let query_embedding = embedding_client.embed_text(query).await?;
    let vector_results = vector_index.search(&query_embedding, overfetch)?;

    // RRF merge (1-based ranks)
    let capacity = keyword_results.len() + vector_results.len();
    let mut combined: HashMap<i64, RrfEntry> = HashMap::with_capacity(capacity);

    for (rank, result) in keyword_results.into_iter().enumerate() {
        let rrf_score = 1.0 / (RRF_K + (rank + 1) as f32);
        let entry = combined
            .entry(result.article_id)
            .or_insert_with(|| RrfEntry {
                keyword_score: None,
                semantic_score: None,
                combined_score: 0.0,
                title: String::new(),
                snippet: String::new(),
            });
        entry.keyword_score = Some(result.score);
        entry.combined_score += rrf_score;
        // Take title/snippet from keyword results (they have rich data)
        entry.title = result.title;
        entry.snippet = result.snippet;
    }

    for (rank, result) in vector_results.iter().enumerate() {
        let rrf_score = 1.0 / (RRF_K + (rank + 1) as f32);
        let entry = combined
            .entry(result.article_id)
            .or_insert_with(|| RrfEntry {
                keyword_score: None,
                semantic_score: None,
                combined_score: 0.0,
                title: String::new(),
                snippet: String::new(),
            });
        entry.semantic_score = Some(result.similarity);
        entry.combined_score += rrf_score;
    }

    let mut results: Vec<HybridSearchResult> = combined
        .into_iter()
        .map(|(article_id, e)| HybridSearchResult {
            article_id,
            combined_score: e.combined_score,
            keyword_score: e.keyword_score,
            semantic_score: e.semantic_score,
            title: e.title,
            snippet: e.snippet,
        })
        .collect();

    results.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
}

struct RrfEntry {
    keyword_score: Option<f32>,
    semantic_score: Option<f32>,
    combined_score: f32,
    title: String,
    snippet: String,
}
