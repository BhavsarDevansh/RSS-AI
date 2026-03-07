use super::error::VectorError;
use super::index::VectorIndex;

fn test_index(dims: usize) -> (VectorIndex, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let index = VectorIndex::open_at(dir.path(), dims).unwrap();
    (index, dir)
}

#[test]
fn add_and_search_basic() {
    let (index, _dir) = test_index(4);
    let emb = vec![1.0, 0.0, 0.0, 0.0];
    index.add(1, &emb).unwrap();

    let results = index.search(&emb, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].article_id, 1);
    assert!(
        results[0].similarity > 0.99,
        "similarity: {}",
        results[0].similarity
    );
}

#[test]
fn dimension_mismatch_rejected() {
    let (index, _dir) = test_index(4);
    let err = index.add(1, &[1.0, 2.0]).unwrap_err();
    assert!(
        matches!(
            err,
            VectorError::DimensionMismatch {
                expected: 4,
                actual: 2
            }
        ),
        "got: {err}"
    );
}

#[test]
fn dimension_mismatch_on_search() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    let err = index.search(&[1.0, 0.0], 10).unwrap_err();
    assert!(matches!(err, VectorError::DimensionMismatch { .. }));
}

#[test]
fn duplicate_add_rejected() {
    let (index, _dir) = test_index(4);
    let emb = vec![1.0, 0.0, 0.0, 0.0];
    index.add(1, &emb).unwrap();
    let err = index.add(1, &emb).unwrap_err();
    assert!(matches!(err, VectorError::DuplicateEntry(1)), "got: {err}");
}

#[test]
fn remove_filters_from_search() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();

    index.remove(1).unwrap();

    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].article_id, 2);
}

#[test]
fn remove_nonexistent_returns_error() {
    let (index, _dir) = test_index(4);
    let err = index.remove(999).unwrap_err();
    assert!(matches!(err, VectorError::NotFound(999)), "got: {err}");
}

#[test]
fn remove_already_tombstoned_returns_error() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.remove(1).unwrap();
    let err = index.remove(1).unwrap_err();
    assert!(matches!(err, VectorError::NotFound(1)), "got: {err}");
}

#[test]
fn contains_works() {
    let (index, _dir) = test_index(4);
    assert!(!index.contains(1).unwrap());

    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    assert!(index.contains(1).unwrap());

    index.remove(1).unwrap();
    assert!(!index.contains(1).unwrap());
}

#[test]
fn count_tracks_live_entries() {
    let (index, _dir) = test_index(4);
    assert_eq!(index.count().unwrap(), 0);

    for i in 1..=5 {
        let mut emb = vec![0.0; 4];
        emb[(i as usize) % 4] = 1.0;
        index.add(i, &emb).unwrap();
    }
    assert_eq!(index.count().unwrap(), 5);

    index.remove(2).unwrap();
    index.remove(4).unwrap();
    assert_eq!(index.count().unwrap(), 3);
}

#[test]
fn persistence_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let dims = 4;

    // Create and populate
    {
        let index = VectorIndex::open_at(dir.path(), dims).unwrap();
        index.add(10, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(20, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.add(30, &[0.0, 0.0, 1.0, 0.0]).unwrap();
        index.save().unwrap();
    }

    // Reload and verify
    {
        let index = VectorIndex::open_at(dir.path(), dims).unwrap();
        assert_eq!(index.count().unwrap(), 3);
        assert!(index.contains(10).unwrap());
        assert!(index.contains(20).unwrap());
        assert!(index.contains(30).unwrap());

        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].article_id, 10);
        assert!(results[0].similarity > 0.99);
    }
}

#[test]
fn persistence_excludes_tombstoned() {
    let dir = tempfile::tempdir().unwrap();
    let dims = 4;

    {
        let index = VectorIndex::open_at(dir.path(), dims).unwrap();
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.remove(1).unwrap();
        index.save().unwrap();
    }

    {
        let index = VectorIndex::open_at(dir.path(), dims).unwrap();
        assert_eq!(index.count().unwrap(), 1);
        assert!(!index.contains(1).unwrap());
        assert!(index.contains(2).unwrap());
    }
}

#[test]
fn auto_save_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let index = VectorIndex::open_at(dir.path(), 4)
        .unwrap()
        .with_auto_save_interval(3);

    // Files should not exist yet
    assert!(!dir.path().join("index_state.json").exists());

    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(!dir.path().join("index_state.json").exists());

    // Third insert triggers auto-save
    index.add(3, &[0.0, 0.0, 1.0, 0.0]).unwrap();
    assert!(dir.path().join("index_state.json").exists());
    assert!(dir.path().join("embeddings.bin").exists());
}

#[test]
fn search_empty_index() {
    let (index, _dir) = test_index(4);
    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn batch_add_inserts_all() {
    let (index, _dir) = test_index(4);
    let items: Vec<(i64, Vec<f32>)> = (0..10)
        .map(|i| {
            let mut emb = vec![0.0; 4];
            emb[i % 4] = 1.0;
            emb[0] += i as f32 * 0.01; // slight variation
            (i as i64 + 1, emb)
        })
        .collect();

    index.add_batch(&items).unwrap();
    assert_eq!(index.count().unwrap(), 10);

    for (id, _) in &items {
        assert!(index.contains(*id).unwrap());
    }
}

#[test]
fn similarity_ordering() {
    let (index, _dir) = test_index(4);

    // Article 1: exactly matches query direction
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    // Article 2: partially matches
    index.add(2, &[0.7, 0.7, 0.0, 0.0]).unwrap();
    // Article 3: orthogonal
    index.add(3, &[0.0, 0.0, 1.0, 0.0]).unwrap();

    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 3).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].article_id, 1);
    assert!(results[0].similarity > results[1].similarity);
    assert!(results[1].similarity > results[2].similarity);
}

#[test]
fn re_add_after_remove() {
    let (index, _dir) = test_index(4);

    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.remove(1).unwrap();
    assert!(!index.contains(1).unwrap());

    // Re-add with different embedding
    index.add(1, &[0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(index.contains(1).unwrap());
    assert_eq!(index.count().unwrap(), 1);

    // Should find the new embedding
    let results = index.search(&[0.0, 1.0, 0.0, 0.0], 1).unwrap();
    assert_eq!(results[0].article_id, 1);
    assert!(results[0].similarity > 0.99);
}

// ── Additional edge case tests ──────────────────────────────────────

#[test]
fn zero_dimensions_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err = VectorIndex::open_at(dir.path(), 0).unwrap_err();
    assert!(
        matches!(
            err,
            VectorError::DimensionMismatch {
                expected: 1,
                actual: 0
            }
        ),
        "got: {err}"
    );
}

#[test]
fn search_limit_zero_returns_empty() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 0).unwrap();
    assert!(results.is_empty());
}

#[test]
fn batch_add_dimension_mismatch() {
    let (index, _dir) = test_index(4);
    let items = vec![
        (1, vec![1.0, 0.0, 0.0, 0.0]),
        (2, vec![0.0, 1.0]), // wrong dimensions
    ];
    let err = index.add_batch(&items).unwrap_err();
    assert!(matches!(
        err,
        VectorError::DimensionMismatch {
            expected: 4,
            actual: 2
        }
    ));
    // None should have been inserted
    assert_eq!(index.count().unwrap(), 0);
}

#[test]
fn batch_add_duplicate_in_existing() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();

    let items = vec![
        (2, vec![0.0, 1.0, 0.0, 0.0]),
        (1, vec![0.0, 0.0, 1.0, 0.0]), // duplicate with existing
    ];
    let err = index.add_batch(&items).unwrap_err();
    assert!(matches!(err, VectorError::DuplicateEntry(1)));
    // None of the batch should have been inserted
    assert_eq!(index.count().unwrap(), 1);
}

#[test]
fn batch_add_empty_is_noop() {
    let (index, _dir) = test_index(4);
    index.add_batch(&[]).unwrap();
    assert_eq!(index.count().unwrap(), 0);
}

#[test]
fn persistence_dimension_mismatch_on_reload() {
    let dir = tempfile::tempdir().unwrap();

    // Save with dims=4
    {
        let index = VectorIndex::open_at(dir.path(), 4).unwrap();
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.save().unwrap();
    }

    // Try to reload with dims=3
    let err = VectorIndex::open_at(dir.path(), 3).unwrap_err();
    assert!(
        matches!(
            err,
            VectorError::DimensionMismatch {
                expected: 3,
                actual: 4
            }
        ),
        "got: {err}"
    );
}

#[test]
fn corrupted_state_file_returns_persistence_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();

    // Write invalid JSON
    std::fs::write(dir.path().join("index_state.json"), "not valid json").unwrap();
    std::fs::write(dir.path().join("embeddings.bin"), &[]).unwrap();

    let err = VectorIndex::open_at(dir.path(), 4).unwrap_err();
    assert!(
        matches!(err, VectorError::Persistence(_)),
        "expected Persistence error, got: {err}"
    );
}

#[test]
fn truncated_embeddings_file_returns_io_error() {
    let dir = tempfile::tempdir().unwrap();

    // Create valid state but truncated embeddings
    {
        let index = VectorIndex::open_at(dir.path(), 4).unwrap();
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.save().unwrap();
    }

    // Truncate the embeddings file (leave only the u64 ID, no float data)
    let emb_path = dir.path().join("embeddings.bin");
    std::fs::write(&emb_path, &[0u8; 8]).unwrap();

    let err = VectorIndex::open_at(dir.path(), 4).unwrap_err();
    assert!(
        matches!(err, VectorError::Io(_)),
        "expected Io error, got: {err}"
    );
}

#[test]
fn similarity_scores_in_valid_range() {
    let (index, _dir) = test_index(4);
    index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
    index.add(3, &[-1.0, 0.0, 0.0, 0.0]).unwrap();

    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
    for r in &results {
        assert!(
            (0.0..=1.0).contains(&r.similarity),
            "similarity {} out of range for article {}",
            r.similarity,
            r.article_id
        );
    }
}

#[test]
fn concurrent_reads_are_safe() {
    let (index, _dir) = test_index(4);
    for i in 1..=20 {
        let mut emb = vec![0.0; 4];
        emb[(i as usize) % 4] = 1.0;
        emb[0] += i as f32 * 0.001;
        index.add(i, &emb).unwrap();
    }

    let index = std::sync::Arc::new(index);
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let idx = std::sync::Arc::clone(&index);
            std::thread::spawn(move || {
                let query = vec![1.0, 0.0, 0.0, 0.0];
                for _ in 0..50 {
                    let results = idx.search(&query, 5).unwrap();
                    assert!(!results.is_empty());
                    assert_eq!(idx.count().unwrap(), 20);
                    assert!(idx.contains(((t % 20) + 1) as i64).unwrap());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn search_returns_at_most_limit() {
    let (index, _dir) = test_index(4);
    for i in 1..=20 {
        let mut emb = vec![0.0; 4];
        emb[(i as usize) % 4] = 1.0;
        emb[0] += i as f32 * 0.001;
        index.add(i, &emb).unwrap();
    }

    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 3).unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn negative_article_ids_work() {
    let (index, _dir) = test_index(4);
    index.add(-1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    index.add(-100, &[0.0, 1.0, 0.0, 0.0]).unwrap();

    assert!(index.contains(-1).unwrap());
    assert!(index.contains(-100).unwrap());
    assert_eq!(index.count().unwrap(), 2);

    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
    assert_eq!(results[0].article_id, -1);
}
