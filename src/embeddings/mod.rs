/// Text embedding generation for semantic search via OpenAI-compatible API.
pub mod client;
mod error;
pub mod pipeline;
pub mod text;
mod types;

pub use client::EmbeddingClient;
pub use error::EmbeddingError;
pub use pipeline::process_pending_articles;
pub use text::prepare_article_text;
pub use types::EmbeddingBatchResult;

#[cfg(test)]
mod client_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod text_tests;
