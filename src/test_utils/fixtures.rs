use std::path::PathBuf;

/// Returns the path to the `tests/fixtures/` directory.
pub fn fixtures_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path
}

/// Reads a fixture file relative to `tests/fixtures/`.
///
/// # Panics
/// Panics if the file does not exist or cannot be read.
pub fn read_fixture(relative_path: &str) -> String {
    let path = fixtures_dir().join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_dir_exists() {
        let dir = fixtures_dir();
        assert!(
            dir.is_dir(),
            "fixtures dir does not exist: {}",
            dir.display()
        );
    }

    #[test]
    fn read_rss_fixture() {
        let content = read_fixture("rss/rss_valid.xml");
        assert!(content.contains("<rss"));
    }
}
