use tempfile::TempDir;

use crate::config::Config;

/// Build a [`Config`] suitable for tests.
///
/// - `data_dir` points inside a fresh [`TempDir`] (caller keeps it alive).
/// - Short timeouts and minimal polling interval.
/// - `log_level` set to `"debug"`.
/// - LLM `api_base_url` set to the provided mock server URL.
pub fn test_config(mock_llm_url: &str) -> (Config, TempDir) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    let mut cfg = Config::default();
    cfg.service.data_dir = tmp.path().join("data").to_string_lossy().into_owned();
    cfg.service.log_level = "debug".to_string();
    cfg.polling.default_interval_minutes = 1;
    cfg.polling.max_concurrent_fetches = 1;
    cfg.extraction.request_timeout_seconds = 5;
    cfg.llm.api_base_url = mock_llm_url.to_string();

    cfg.validate().expect("test_config should be valid");

    (cfg, tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validates() {
        let (cfg, _tmp) = test_config("http://localhost:1234");
        assert_eq!(cfg.service.log_level, "debug");
        assert_eq!(cfg.polling.default_interval_minutes, 1);
        assert_eq!(cfg.extraction.request_timeout_seconds, 5);
        assert_eq!(cfg.llm.api_base_url, "http://localhost:1234");
        assert!(cfg.data_dir().exists() || cfg.data_dir().parent().unwrap().exists());
    }

    #[test]
    fn test_config_temp_dir_is_unique() {
        let (_cfg1, tmp1) = test_config("http://localhost:1111");
        let (_cfg2, tmp2) = test_config("http://localhost:2222");
        assert_ne!(tmp1.path(), tmp2.path());
    }
}
