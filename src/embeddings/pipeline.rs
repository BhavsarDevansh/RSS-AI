/// Batch processing pipeline for embedding generation.
use sqlx::SqlitePool;

use super::client::EmbeddingClient;
use super::error::EmbeddingError;
use super::text::prepare_article_text;
use super::types::EmbeddingBatchResult;
use crate::db;

const DEFAULT_BATCH_SIZE: usize = 10;

/// Process articles where `content_extracted = 1` and `embedding_generated = 0`.
/// Generates embeddings, marks articles in the DB, and returns the embeddings
/// for the caller to store in the vector index.
pub async fn process_pending_articles(
    client: &EmbeddingClient,
    pool: &SqlitePool,
    batch_size: Option<usize>,
) -> Result<EmbeddingBatchResult, EmbeddingError> {
    let batch_size = batch_size.unwrap_or(DEFAULT_BATCH_SIZE);

    let pending: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, content FROM articles
         WHERE content_extracted = 1 AND embedding_generated = 0
         ORDER BY id
         LIMIT 1000",
    )
    .fetch_all(pool)
    .await
    .map_err(db::DbError::from)?;

    let mut result = EmbeddingBatchResult {
        processed: 0,
        failed: 0,
        embeddings: Vec::with_capacity(pending.len()),
    };

    for chunk in pending.chunks(batch_size) {
        let texts: Vec<String> = chunk
            .iter()
            .map(|(_, title, content)| prepare_article_text(title, content.as_deref()))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();

        match client.embed_batch(&text_refs).await {
            Ok(embeddings) => {
                for (i, embedding) in embeddings.into_iter().enumerate() {
                    let article_id = chunk[i].0;
                    if let Err(e) = db::articles::mark_embedding_generated(pool, article_id).await {
                        tracing::warn!(article_id, error = %e, "failed to mark embedding_generated");
                        result.failed += 1;
                        continue;
                    }
                    result.embeddings.push((article_id, embedding));
                    result.processed += 1;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "batch embedding failed, falling back to individual");
                for (i, text) in text_refs.iter().enumerate() {
                    let article_id = chunk[i].0;
                    match client.embed_text(text).await {
                        Ok(embedding) => {
                            if let Err(e) =
                                db::articles::mark_embedding_generated(pool, article_id).await
                            {
                                tracing::warn!(article_id, error = %e, "failed to mark embedding_generated");
                                result.failed += 1;
                                continue;
                            }
                            result.embeddings.push((article_id, embedding));
                            result.processed += 1;
                        }
                        Err(e) => {
                            tracing::warn!(article_id, error = %e, "embedding generation failed, skipping");
                            result.failed += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}
