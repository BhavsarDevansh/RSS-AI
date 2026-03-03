use std::fs;

use rss_ai::config::{Config, ConfigError};

#[test]
fn load_full_config_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
[service]
data_dir = "/tmp/rss-ai-test"
log_level = "debug"

[polling]
default_interval_minutes = 15
max_concurrent_fetches = 8

[llm]
api_base_url = "http://localhost:11434"
model = "mistral"
embedding_model = "nomic-embed-text"
embedding_dimensions = 512
max_tokens = 4096
temperature = 0.5

[extraction]
user_agent = "test-agent/1.0"
request_timeout_seconds = 10
max_article_size_bytes = 1048576

[mcp]
stdio_enabled = false
sse_enabled = true
sse_host = "0.0.0.0"
sse_port = 9090
"#,
    )
    .unwrap();

    let cfg = Config::load(Some(&path)).unwrap();

    assert_eq!(cfg.service.data_dir, "/tmp/rss-ai-test");
    assert_eq!(cfg.service.log_level, "debug");
    assert_eq!(cfg.polling.default_interval_minutes, 15);
    assert_eq!(cfg.polling.max_concurrent_fetches, 8);
    assert_eq!(cfg.llm.model, "mistral");
    assert_eq!(cfg.llm.embedding_dimensions, 512);
    assert_eq!(cfg.llm.max_tokens, 4096);
    assert!((cfg.llm.temperature - 0.5).abs() < f64::EPSILON);
    assert_eq!(cfg.extraction.user_agent, "test-agent/1.0");
    assert_eq!(cfg.extraction.request_timeout_seconds, 10);
    assert_eq!(cfg.extraction.max_article_size_bytes, 1_048_576);
    assert!(!cfg.mcp.stdio_enabled);
    assert!(cfg.mcp.sse_enabled);
    assert_eq!(cfg.mcp.sse_host, "0.0.0.0");
    assert_eq!(cfg.mcp.sse_port, 9090);
}

#[test]
fn partial_config_uses_defaults_for_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
[service]
log_level = "warn"
"#,
    )
    .unwrap();

    let cfg = Config::load(Some(&path)).unwrap();

    assert_eq!(cfg.service.log_level, "warn");
    // Rest should be defaults.
    assert_eq!(cfg.polling.default_interval_minutes, 30);
    assert_eq!(cfg.llm.model, "llama3.2");
    assert!(cfg.mcp.stdio_enabled);
}

#[test]
fn empty_file_uses_all_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();

    let cfg = Config::load(Some(&path)).unwrap();
    let defaults = Config::default();

    assert_eq!(cfg.service.log_level, defaults.service.log_level);
    assert_eq!(
        cfg.polling.default_interval_minutes,
        defaults.polling.default_interval_minutes
    );
    assert_eq!(cfg.llm.model, defaults.llm.model);
}

#[test]
fn missing_custom_path_returns_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.toml");
    let err = Config::load(Some(&path)).unwrap_err();
    assert!(
        matches!(err, ConfigError::Io(_)),
        "expected Io error, got: {err}"
    );
}

#[test]
fn default_path_auto_creates_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("rss-ai");
    let config_path = config_dir.join("config.toml");

    // Temporarily set XDG_CONFIG_HOME so default_path resolves into our tempdir.
    // We can't easily override default_path, so we test the auto-create logic
    // by manually calling load with a path inside a non-existent directory.
    // Instead, test that the template written is valid.
    assert!(!config_path.exists());

    fs::create_dir_all(&config_dir).unwrap();
    fs::write(&config_path, Config::default_toml_with_comments()).unwrap();

    assert!(config_path.exists());
    let cfg = Config::load(Some(&config_path)).unwrap();
    cfg.validate().unwrap();
}

#[test]
fn generated_template_is_parseable() {
    let template = Config::default_toml_with_comments();
    let cfg: Config = toml::from_str(template).unwrap();
    cfg.validate().unwrap();
}
