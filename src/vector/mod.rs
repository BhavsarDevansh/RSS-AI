/// Vector similarity search via HNSW (hnsw_rs).
mod error;
pub mod hybrid;
pub mod index;
mod types;

pub use error::VectorError;
pub use hybrid::hybrid_search;
pub use index::VectorIndex;
pub use types::{HybridSearchResult, VectorSearchResult};

#[cfg(test)]
mod hybrid_tests;
#[cfg(test)]
mod index_tests;
