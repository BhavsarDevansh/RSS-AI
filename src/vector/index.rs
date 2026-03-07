/// Core vector index implementation using hnsw_rs.
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use hnsw_rs::prelude::*;

use super::error::VectorError;
use super::types::{IndexState, VectorSearchResult};
use crate::config::Config;

// ── HNSW parameters ─────────────────────────────────────────────────

/// Maximum number of bidirectional links per node per layer.
const HNSW_M: usize = 16;

/// Number of candidates to consider during index construction.
const HNSW_EF_CONSTRUCTION: usize = 200;

/// Maximum number of layers.
const HNSW_MAX_LAYER: usize = 16;

/// Default ef parameter for search (controls accuracy vs speed).
const DEFAULT_EF_SEARCH: usize = 32;

/// Initial allocation hint for the number of elements.
const INITIAL_MAX_ELEMENTS: usize = 10_000;

/// Default number of inserts between auto-saves.
const DEFAULT_AUTO_SAVE_INTERVAL: usize = 100;

/// Maximum number of embeddings that can be loaded from disk, preventing
/// a corrupted or malicious file from exhausting memory.
const MAX_LOADABLE_EMBEDDINGS: usize = 10_000_000;

// ── File names ──────────────────────────────────────────────────────

const STATE_FILE: &str = "index_state.json";
const EMBEDDINGS_FILE: &str = "embeddings.bin";

// ── Inner state (behind RwLock) ─────────────────────────────────────

struct InnerIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    id_to_internal: HashMap<i64, usize>,
    internal_to_id: HashMap<usize, i64>,
    tombstones: HashSet<usize>,
    embeddings: HashMap<usize, Vec<f32>>,
    next_internal_id: usize,
    inserts_since_save: usize,
    dimensions: usize,
}

/// An embedded HNSW-based vector similarity index for article embeddings.
///
/// Thread-safe: multiple readers can search concurrently while a single
/// writer adds or removes vectors.
pub struct VectorIndex {
    inner: RwLock<InnerIndex>,
    index_dir: PathBuf,
    auto_save_interval: usize,
}

impl std::fmt::Debug for VectorIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorIndex")
            .field("index_dir", &self.index_dir)
            .field("auto_save_interval", &self.auto_save_interval)
            .finish_non_exhaustive()
    }
}

impl VectorIndex {
    /// Create or load a vector index using paths from the application config.
    pub fn new(config: &Config) -> Result<Self, VectorError> {
        let index_dir = config.data_dir().join("vector_index");
        let dimensions = config.llm.embedding_dimensions as usize;
        Self::open_at(&index_dir, dimensions)
    }

    /// Create or load a vector index at a specific path.
    pub fn open_at(path: &Path, dimensions: usize) -> Result<Self, VectorError> {
        if dimensions == 0 {
            return Err(VectorError::DimensionMismatch {
                expected: 1,
                actual: 0,
            });
        }

        std::fs::create_dir_all(path)?;

        let state_path = path.join(STATE_FILE);
        let embeddings_path = path.join(EMBEDDINGS_FILE);

        let inner = if state_path.exists() && embeddings_path.exists() {
            Self::load_from_disk(path, dimensions)?
        } else {
            InnerIndex {
                hnsw: Hnsw::new(
                    HNSW_M,
                    INITIAL_MAX_ELEMENTS,
                    HNSW_MAX_LAYER,
                    HNSW_EF_CONSTRUCTION,
                    DistCosine {},
                ),
                id_to_internal: HashMap::new(),
                internal_to_id: HashMap::new(),
                tombstones: HashSet::new(),
                embeddings: HashMap::new(),
                next_internal_id: 0,
                inserts_since_save: 0,
                dimensions,
            }
        };

        Ok(Self {
            inner: RwLock::new(inner),
            index_dir: path.to_path_buf(),
            auto_save_interval: DEFAULT_AUTO_SAVE_INTERVAL,
        })
    }

    /// Set a custom auto-save interval (number of inserts between saves).
    pub fn with_auto_save_interval(mut self, interval: usize) -> Self {
        self.auto_save_interval = interval;
        self
    }

    /// Add a single embedding to the index, keyed by article ID.
    ///
    /// **Note on tombstoned re-adds:** When re-adding a previously removed article,
    /// the old HNSW node becomes orphaned because HNSW does not support true deletion.
    /// Over many add/remove cycles for the same article, orphaned nodes accumulate,
    /// consuming memory and potentially degrading search performance. Long-running
    /// instances should periodically call [`rebuild`](Self::rebuild) to compact the index.
    pub fn add(&self, article_id: i64, embedding: &[f32]) -> Result<(), VectorError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;

        if embedding.len() != inner.dimensions {
            return Err(VectorError::DimensionMismatch {
                expected: inner.dimensions,
                actual: embedding.len(),
            });
        }

        // Handle duplicate / tombstoned re-add.
        // NOTE: The old HNSW node for the previous internal_id is NOT removed
        // (HNSW doesn't support deletion). It becomes an orphan. See `rebuild()`.
        if let Some(&existing_iid) = inner.id_to_internal.get(&article_id) {
            if !inner.tombstones.contains(&existing_iid) {
                return Err(VectorError::DuplicateEntry(article_id));
            }
            inner.tombstones.remove(&existing_iid);
            inner.embeddings.remove(&existing_iid);
            inner.internal_to_id.remove(&existing_iid);
        }

        let internal_id = inner.next_internal_id;
        inner.next_internal_id += 1;

        inner.id_to_internal.insert(article_id, internal_id);
        inner.internal_to_id.insert(internal_id, article_id);
        inner.embeddings.insert(internal_id, embedding.to_vec());
        inner.hnsw.insert((embedding, internal_id));

        inner.inserts_since_save += 1;
        if self.auto_save_interval > 0 && inner.inserts_since_save >= self.auto_save_interval {
            self.save_inner(&inner)?;
            inner.inserts_since_save = 0;
        }

        Ok(())
    }

    /// Batch-add embeddings. More efficient than repeated `add()` calls for
    /// large batches because it uses parallel insertion internally.
    pub fn add_batch(&self, items: &[(i64, Vec<f32>)]) -> Result<(), VectorError> {
        if items.is_empty() {
            return Ok(());
        }

        let mut inner = self
            .inner
            .write()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;

        // Validate all dimensions and check for duplicates upfront
        for (article_id, embedding) in items {
            if embedding.len() != inner.dimensions {
                return Err(VectorError::DimensionMismatch {
                    expected: inner.dimensions,
                    actual: embedding.len(),
                });
            }
            if let Some(&existing_iid) = inner.id_to_internal.get(article_id)
                && !inner.tombstones.contains(&existing_iid)
            {
                return Err(VectorError::DuplicateEntry(*article_id));
            }
        }

        // Assign IDs and store embeddings
        for (article_id, embedding) in items {
            if let Some(&existing_iid) = inner.id_to_internal.get(article_id) {
                inner.tombstones.remove(&existing_iid);
                inner.embeddings.remove(&existing_iid);
                inner.internal_to_id.remove(&existing_iid);
            }

            let internal_id = inner.next_internal_id;
            inner.next_internal_id += 1;

            inner.id_to_internal.insert(*article_id, internal_id);
            inner.internal_to_id.insert(internal_id, *article_id);
            inner.embeddings.insert(internal_id, embedding.clone());
        }

        // Collect references for parallel insertion
        let batch_data: Vec<(&Vec<f32>, usize)> = items
            .iter()
            .map(|(article_id, _)| {
                let iid = inner.id_to_internal[article_id];
                (inner.embeddings.get(&iid).unwrap(), iid)
            })
            .collect();

        inner.hnsw.parallel_insert(&batch_data);

        inner.inserts_since_save += items.len();
        if self.auto_save_interval > 0 && inner.inserts_since_save >= self.auto_save_interval {
            self.save_inner(&inner)?;
            inner.inserts_since_save = 0;
        }

        Ok(())
    }

    /// Remove a vector from the index by article ID (tombstone-based).
    pub fn remove(&self, article_id: i64) -> Result<(), VectorError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;

        let &internal_id = inner
            .id_to_internal
            .get(&article_id)
            .ok_or(VectorError::NotFound(article_id))?;

        if inner.tombstones.contains(&internal_id) {
            return Err(VectorError::NotFound(article_id));
        }

        inner.tombstones.insert(internal_id);
        Ok(())
    }

    /// Find the `limit` most similar vectors to `query_embedding`.
    pub fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>, VectorError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;

        if query_embedding.len() != inner.dimensions {
            return Err(VectorError::DimensionMismatch {
                expected: inner.dimensions,
                actual: query_embedding.len(),
            });
        }

        let live_count = inner.id_to_internal.len() - inner.tombstones.len();
        if live_count == 0 || limit == 0 {
            return Ok(Vec::new());
        }

        // Over-fetch to compensate for tombstoned results being filtered out
        let total_points = inner.id_to_internal.len();
        let overfetch = limit
            .saturating_add(inner.tombstones.len())
            .min(total_points);
        let ef = DEFAULT_EF_SEARCH.max(overfetch);
        let neighbours = inner.hnsw.search(query_embedding, overfetch, ef);

        let results: Vec<VectorSearchResult> = neighbours
            .into_iter()
            .filter(|n| !inner.tombstones.contains(&n.d_id))
            .filter_map(|n| {
                inner.internal_to_id.get(&n.d_id).map(|&article_id| {
                    VectorSearchResult {
                        article_id,
                        // DistCosine returns 1.0 - cosine_similarity
                        similarity: (1.0 - n.distance).clamp(0.0, 1.0),
                    }
                })
            })
            .take(limit)
            .collect();

        Ok(results)
    }

    /// Check if an article is currently in the index (not tombstoned).
    pub fn contains(&self, article_id: i64) -> Result<bool, VectorError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;
        let result = inner
            .id_to_internal
            .get(&article_id)
            .map(|iid| !inner.tombstones.contains(iid))
            .unwrap_or(false);
        Ok(result)
    }

    /// Return the number of live (non-tombstoned) vectors in the index.
    pub fn count(&self) -> Result<usize, VectorError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;
        Ok(inner.id_to_internal.len() - inner.tombstones.len())
    }

    /// Rebuild the HNSW index from scratch, eliminating orphaned nodes left by
    /// tombstoned deletions and re-adds. Call this periodically on long-running
    /// instances to reclaim memory and restore search performance.
    pub fn rebuild(&self) -> Result<(), VectorError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;

        // Remove tombstoned entries first so we can borrow embeddings cleanly
        let tombstoned: Vec<usize> = inner.tombstones.drain().collect();
        let removed = tombstoned.len();
        for iid in &tombstoned {
            inner.embeddings.remove(iid);
            inner.internal_to_id.remove(iid);
        }

        // Build new HNSW from remaining live entries
        let max_elements = inner.embeddings.len().max(INITIAL_MAX_ELEMENTS);
        let new_hnsw = Hnsw::new(
            HNSW_M,
            max_elements,
            HNSW_MAX_LAYER,
            HNSW_EF_CONSTRUCTION,
            DistCosine {},
        );

        let batch: Vec<(&Vec<f32>, usize)> = inner
            .embeddings
            .iter()
            .map(|(&iid, emb)| (emb, iid))
            .collect();

        let live = batch.len();
        if !batch.is_empty() {
            new_hnsw.parallel_insert(&batch);
        }

        inner.hnsw = new_hnsw;

        tracing::info!(live, removed, "vector index rebuilt");

        Ok(())
    }

    /// Persist the current index state to disk.
    pub fn save(&self) -> Result<(), VectorError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| VectorError::LockPoisoned(e.to_string()))?;
        self.save_inner(&inner)
    }

    // ── Private helpers ─────────────────────────────────────────────

    fn save_inner(&self, inner: &InnerIndex) -> Result<(), VectorError> {
        // Serialize directly from references — no cloning of maps.
        // IndexState borrows from InnerIndex via serde's Serialize impl.
        let state = IndexStateBorrowed {
            dimensions: inner.dimensions,
            next_internal_id: inner.next_internal_id,
            id_to_internal: &inner.id_to_internal,
            internal_to_id: &inner.internal_to_id,
            tombstones: &inner.tombstones,
        };

        // Write metadata
        let state_path = self.index_dir.join(STATE_FILE);
        let file = std::fs::File::create(&state_path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer(writer, &state)
            .map_err(|e| VectorError::Persistence(e.to_string()))?;

        // Write embeddings (skip tombstoned entries).
        // Write each embedding as a contiguous byte chunk rather than per-float.
        let embeddings_path = self.index_dir.join(EMBEDDINGS_FILE);
        let file = std::fs::File::create(&embeddings_path)?;
        let mut writer = BufWriter::new(file);

        for (&internal_id, embedding) in &inner.embeddings {
            if inner.tombstones.contains(&internal_id) {
                continue;
            }
            writer.write_all(&(internal_id as u64).to_le_bytes())?;
            for &val in embedding {
                writer.write_all(&val.to_le_bytes())?;
            }
        }
        writer.flush()?;

        tracing::debug!(
            count = inner.id_to_internal.len() - inner.tombstones.len(),
            "vector index saved"
        );

        Ok(())
    }

    fn load_from_disk(index_dir: &Path, expected_dims: usize) -> Result<InnerIndex, VectorError> {
        // Read metadata
        let state_path = index_dir.join(STATE_FILE);
        let file = std::fs::File::open(&state_path)?;
        let reader = BufReader::new(file);
        let state: IndexState =
            serde_json::from_reader(reader).map_err(|e| VectorError::Persistence(e.to_string()))?;

        if state.dimensions != expected_dims {
            return Err(VectorError::DimensionMismatch {
                expected: expected_dims,
                actual: state.dimensions,
            });
        }

        // Validate consistency of persisted state
        if state.id_to_internal.len() != state.internal_to_id.len() {
            return Err(VectorError::Persistence(
                "id_to_internal and internal_to_id have different lengths".to_string(),
            ));
        }

        // Read embeddings in bulk (one read per embedding row instead of per-float)
        let embeddings_path = index_dir.join(EMBEDDINGS_FILE);
        let file = std::fs::File::open(&embeddings_path)?;
        let mut reader = BufReader::new(file);
        let mut embeddings: HashMap<usize, Vec<f32>> = HashMap::new();
        let mut id_buf = [0u8; 8];
        let row_byte_len = expected_dims * std::mem::size_of::<f32>();
        let mut row_buf = vec![0u8; row_byte_len];

        loop {
            match reader.read_exact(&mut id_buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(VectorError::Io(e)),
            }

            if embeddings.len() >= MAX_LOADABLE_EMBEDDINGS {
                return Err(VectorError::Persistence(format!(
                    "embeddings file exceeds maximum of {MAX_LOADABLE_EMBEDDINGS} entries"
                )));
            }

            let internal_id = u64::from_le_bytes(id_buf) as usize;
            reader.read_exact(&mut row_buf)?;

            // Convert byte buffer to f32 slice
            let embedding: Vec<f32> = row_buf
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            embeddings.insert(internal_id, embedding);
        }

        // Create fresh HNSW and re-insert all non-tombstoned embeddings
        let max_elements = embeddings.len().max(INITIAL_MAX_ELEMENTS);
        let hnsw = Hnsw::new(
            HNSW_M,
            max_elements,
            HNSW_MAX_LAYER,
            HNSW_EF_CONSTRUCTION,
            DistCosine {},
        );

        let batch: Vec<(&Vec<f32>, usize)> = embeddings
            .iter()
            .filter(|(iid, _)| !state.tombstones.contains(iid))
            .map(|(iid, emb)| (emb, *iid))
            .collect();

        if !batch.is_empty() {
            hnsw.parallel_insert(&batch);
        }

        tracing::info!(
            count = batch.len(),
            tombstoned = state.tombstones.len(),
            "vector index loaded from disk"
        );

        Ok(InnerIndex {
            hnsw,
            id_to_internal: state.id_to_internal,
            internal_to_id: state.internal_to_id,
            tombstones: state.tombstones,
            embeddings,
            next_internal_id: state.next_internal_id,
            inserts_since_save: 0,
            dimensions: state.dimensions,
        })
    }
}

/// Borrowed version of [`IndexState`] for zero-copy serialization.
#[derive(serde::Serialize)]
struct IndexStateBorrowed<'a> {
    dimensions: usize,
    next_internal_id: usize,
    id_to_internal: &'a HashMap<i64, usize>,
    internal_to_id: &'a HashMap<usize, i64>,
    tombstones: &'a HashSet<usize>,
}
