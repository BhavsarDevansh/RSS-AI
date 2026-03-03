/// Application configuration loading and validation.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Error type ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("Validation error: {0}")]
    Validation(String),
}

// ── Config structs ──────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub service: ServiceConfig,
    pub polling: PollingConfig,
    pub llm: LlmConfig,
    pub extraction: ExtractionConfig,
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    pub data_dir: String,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PollingConfig {
    pub default_interval_minutes: u32,
    pub max_concurrent_fetches: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub api_base_url: String,
    pub model: String,
    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub max_tokens: u32,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtractionConfig {
    pub user_agent: String,
    pub request_timeout_seconds: u32,
    pub max_article_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub stdio_enabled: bool,
    pub sse_enabled: bool,
    pub sse_host: String,
    pub sse_port: u16,
}

// ── Defaults ────────────────────────────────────────────────────────

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.local/share/rss-ai".to_string(),
            log_level: "info".to_string(),
        }
    }
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            default_interval_minutes: 30,
            max_concurrent_fetches: 4,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_base_url: "http://localhost:11434".to_string(),
            model: "llama3.2".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            embedding_dimensions: 768,
            max_tokens: 2048,
            temperature: 0.7,
        }
    }
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            user_agent: "rss-ai/0.2.0".to_string(),
            request_timeout_seconds: 30,
            max_article_size_bytes: 5_242_880, // 5 MiB
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            stdio_enabled: true,
            sse_enabled: false,
            sse_host: "127.0.0.1".to_string(),
            sse_port: 8080,
        }
    }
}

// ── Helper ──────────────────────────────────────────────────────────

/// Expand a leading `~/` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

// ── Config impl ─────────────────────────────────────────────────────

impl Config {
    /// Platform-appropriate default config path: `<config_dir>/rss-ai/config.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("rss-ai")
            .join("config.toml")
    }

    /// Load config from `path` (or the default path).
    ///
    /// When using the default path and the file doesn't exist, creates the
    /// parent directory and writes the commented default template.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let (config_path, is_default) = match path {
            Some(p) => (p.to_path_buf(), false),
            None => (Self::default_path(), true),
        };

        let contents = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && is_default => {
                if let Some(parent) = config_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let template = Self::default_toml_with_comments();
                std::fs::write(&config_path, template)?;
                template.to_string()
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };

        let mut config: Config = toml::from_str(&contents)?;
        config.apply_env_overrides(|key| std::env::var(key).ok());
        config.validate()?;
        Ok(config)
    }

    /// Validate the loaded configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // data_dir must be creatable: at least one ancestor must exist
        let data_path = expand_tilde(&self.service.data_dir);
        if data_path.is_absolute() && !data_path.ancestors().any(|a| a.exists()) {
            return Err(ConfigError::Validation(format!(
                "no ancestor of data_dir exists: {}",
                data_path.display()
            )));
        }

        // log_level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.service.log_level.to_lowercase().as_str()) {
            return Err(ConfigError::Validation(format!(
                "invalid log_level '{}', expected one of: {valid_levels:?}",
                self.service.log_level
            )));
        }

        // polling minimums
        if self.polling.default_interval_minutes < 1 {
            return Err(ConfigError::Validation(
                "default_interval_minutes must be >= 1".to_string(),
            ));
        }
        if self.polling.max_concurrent_fetches < 1 {
            return Err(ConfigError::Validation(
                "max_concurrent_fetches must be >= 1".to_string(),
            ));
        }

        // LLM URL
        if url::Url::parse(&self.llm.api_base_url).is_err() {
            return Err(ConfigError::Validation(format!(
                "invalid api_base_url: '{}'",
                self.llm.api_base_url
            )));
        }

        // temperature
        if !(0.0..=2.0).contains(&self.llm.temperature) {
            return Err(ConfigError::Validation(format!(
                "temperature must be between 0.0 and 2.0, got {}",
                self.llm.temperature
            )));
        }

        // SSE host must not be empty
        if self.mcp.sse_host.is_empty() {
            return Err(ConfigError::Validation(
                "sse_host must not be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Apply environment variable overrides.
    ///
    /// Pattern: `RSS_AI_<SECTION>_<KEY>` (e.g. `RSS_AI_SERVICE_LOG_LEVEL`).
    /// Bad numeric conversions are silently ignored.
    pub fn apply_env_overrides(&mut self, env_fn: impl Fn(&str) -> Option<String>) {
        // service
        if let Some(v) = env_fn("RSS_AI_SERVICE_DATA_DIR") {
            self.service.data_dir = v;
        }
        if let Some(v) = env_fn("RSS_AI_SERVICE_LOG_LEVEL") {
            self.service.log_level = v;
        }

        // polling
        if let Some(v) = env_fn("RSS_AI_POLLING_DEFAULT_INTERVAL_MINUTES")
            && let Ok(n) = v.parse()
        {
            self.polling.default_interval_minutes = n;
        }
        if let Some(v) = env_fn("RSS_AI_POLLING_MAX_CONCURRENT_FETCHES")
            && let Ok(n) = v.parse()
        {
            self.polling.max_concurrent_fetches = n;
        }

        // llm
        if let Some(v) = env_fn("RSS_AI_LLM_API_BASE_URL") {
            self.llm.api_base_url = v;
        }
        if let Some(v) = env_fn("RSS_AI_LLM_MODEL") {
            self.llm.model = v;
        }
        if let Some(v) = env_fn("RSS_AI_LLM_EMBEDDING_MODEL") {
            self.llm.embedding_model = v;
        }
        if let Some(v) = env_fn("RSS_AI_LLM_EMBEDDING_DIMENSIONS")
            && let Ok(n) = v.parse()
        {
            self.llm.embedding_dimensions = n;
        }
        if let Some(v) = env_fn("RSS_AI_LLM_MAX_TOKENS")
            && let Ok(n) = v.parse()
        {
            self.llm.max_tokens = n;
        }
        if let Some(v) = env_fn("RSS_AI_LLM_TEMPERATURE")
            && let Ok(n) = v.parse()
        {
            self.llm.temperature = n;
        }

        // extraction
        if let Some(v) = env_fn("RSS_AI_EXTRACTION_USER_AGENT") {
            self.extraction.user_agent = v;
        }
        if let Some(v) = env_fn("RSS_AI_EXTRACTION_REQUEST_TIMEOUT_SECONDS")
            && let Ok(n) = v.parse()
        {
            self.extraction.request_timeout_seconds = n;
        }
        if let Some(v) = env_fn("RSS_AI_EXTRACTION_MAX_ARTICLE_SIZE_BYTES")
            && let Ok(n) = v.parse()
        {
            self.extraction.max_article_size_bytes = n;
        }

        // mcp
        if let Some(v) = env_fn("RSS_AI_MCP_STDIO_ENABLED")
            && let Ok(b) = v.parse()
        {
            self.mcp.stdio_enabled = b;
        }
        if let Some(v) = env_fn("RSS_AI_MCP_SSE_ENABLED")
            && let Ok(b) = v.parse()
        {
            self.mcp.sse_enabled = b;
        }
        if let Some(v) = env_fn("RSS_AI_MCP_SSE_HOST") {
            self.mcp.sse_host = v;
        }
        if let Some(v) = env_fn("RSS_AI_MCP_SSE_PORT")
            && let Ok(n) = v.parse()
        {
            self.mcp.sse_port = n;
        }
    }

    /// Hand-crafted commented TOML template for `config --generate`.
    pub fn default_toml_with_comments() -> &'static str {
        r#"# RSS-AI Configuration
# See https://github.com/BhavsarDevansh/rss-ai for documentation.

[service]
# Directory for SQLite database, search index, and vector store.
# data_dir = "~/.local/share/rss-ai"

# Logging verbosity: trace, debug, info, warn, error.
# log_level = "info"

[polling]
# How often (in minutes) to check feeds for new articles.
# default_interval_minutes = 30

# Maximum number of feeds to fetch concurrently.
# max_concurrent_fetches = 4

[llm]
# Base URL of the OpenAI-compatible API (e.g. Ollama).
# api_base_url = "http://localhost:11434"

# Model name for chat / summarisation.
# model = "llama3.2"

# Model name for generating embeddings.
# embedding_model = "nomic-embed-text"

# Dimensionality of the embedding vectors.
# embedding_dimensions = 768

# Maximum tokens to generate per request.
# max_tokens = 2048

# Sampling temperature (0.0 – 2.0).
# temperature = 0.7

[extraction]
# User-Agent header sent when fetching articles.
# user_agent = "rss-ai/0.2.0"

# HTTP request timeout in seconds.
# request_timeout_seconds = 30

# Maximum article body size in bytes (5 MiB).
# max_article_size_bytes = 5242880

[mcp]
# Enable the MCP stdio transport.
# stdio_enabled = true

# Enable the MCP SSE transport.
# sse_enabled = false

# Host to bind the SSE server to.
# sse_host = "127.0.0.1"

# Port for the SSE server.
# sse_port = 8080
"#
    }

    /// Return the tilde-expanded data directory path.
    pub fn data_dir(&self) -> PathBuf {
        expand_tilde(&self.service.data_dir)
    }
}

// ── Unit tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Helper: build an env lookup fn from a HashMap.
    fn env_from_map<'a>(
        map: &'a HashMap<&'a str, &'a str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| map.get(key).map(|v| (*v).to_string())
    }

    /// Return a config with data_dir set to /tmp so tests pass on any host.
    fn test_config() -> Config {
        let mut cfg = Config::default();
        cfg.service.data_dir = "/tmp/rss-ai-test".to_string();
        cfg
    }

    #[test]
    fn default_config_validates() {
        test_config().validate().unwrap();
    }

    #[test]
    fn tilde_expansion_works() {
        let expanded = expand_tilde("~/data");
        assert!(
            !expanded.to_string_lossy().starts_with('~'),
            "tilde should be expanded: {expanded:?}"
        );

        // Non-tilde path left unchanged.
        let plain = expand_tilde("/tmp/data");
        assert_eq!(plain, PathBuf::from("/tmp/data"));
    }

    #[test]
    fn bad_url_fails_validation() {
        let mut cfg = test_config();
        cfg.llm.api_base_url = "not a url".to_string();
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("api_base_url")),
            "expected Validation error about api_base_url, got: {err}"
        );
    }

    #[test]
    fn bad_interval_fails_validation() {
        let mut cfg = test_config();
        cfg.polling.default_interval_minutes = 0;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(ref msg) if msg.contains("interval")));
    }

    #[test]
    fn bad_log_level_fails_validation() {
        let mut cfg = test_config();
        cfg.service.log_level = "verbose".to_string();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(ref msg) if msg.contains("log_level")));
    }

    #[test]
    fn bad_temperature_fails_validation() {
        let mut cfg = test_config();
        cfg.llm.temperature = 3.0;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(ref msg) if msg.contains("temperature")));
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let partial = r#"
[service]
log_level = "debug"
"#;
        let cfg: Config = toml::from_str(partial).unwrap();
        assert_eq!(cfg.service.log_level, "debug");
        // Everything else should be default.
        assert_eq!(cfg.polling.default_interval_minutes, 30);
        assert_eq!(cfg.llm.model, "llama3.2");
    }

    #[test]
    fn env_overrides_apply() {
        let mut cfg = Config::default();
        let mut env = HashMap::new();
        env.insert("RSS_AI_SERVICE_LOG_LEVEL", "debug");
        env.insert("RSS_AI_POLLING_DEFAULT_INTERVAL_MINUTES", "60");
        env.insert("RSS_AI_LLM_TEMPERATURE", "0.3");
        env.insert("RSS_AI_MCP_SSE_PORT", "9090");

        cfg.apply_env_overrides(env_from_map(&env));

        assert_eq!(cfg.service.log_level, "debug");
        assert_eq!(cfg.polling.default_interval_minutes, 60);
        assert!((cfg.llm.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(cfg.mcp.sse_port, 9090);
    }

    #[test]
    fn bad_numeric_env_override_ignored() {
        let mut cfg = Config::default();
        let mut env = HashMap::new();
        env.insert("RSS_AI_POLLING_DEFAULT_INTERVAL_MINUTES", "not_a_number");

        cfg.apply_env_overrides(env_from_map(&env));

        // Should keep the default.
        assert_eq!(cfg.polling.default_interval_minutes, 30);
    }

    #[test]
    fn default_template_roundtrips() {
        let template = Config::default_toml_with_comments();
        let mut cfg: Config = toml::from_str(template).unwrap();
        cfg.service.data_dir = "/tmp/rss-ai-test".to_string();
        cfg.validate().unwrap();
    }
}
