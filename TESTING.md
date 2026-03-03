# Testing Guide

## Running Tests

```bash
cargo test                     # Run all tests
cargo test <test_name>         # Run a single test
cargo test test_utils          # Run all test_utils tests
```

## Test Infrastructure (`src/test_utils/`)

The `test_utils` module (compiled only under `#[cfg(test)]`) provides shared helpers for all tests in the crate.

### Modules

| Module | Purpose |
|--------|---------|
| `db` | `test_pool()` — isolated in-memory SQLite with migrations applied |
| `fixtures` | `fixtures_dir()`, `read_fixture(path)` — load static fixture files |
| `mock_http` | `MockFeedServer` — wiremock-based mock for RSS feeds and HTML articles |
| `mock_llm` | `MockLlmServer` — wiremock-based mock for OpenAI-compatible API |

### Planned Modules (blocked on domain types)

- `context` — `AppContext` test builder (needs #3)
- `assertions` — domain-specific assertion helpers (needs #2)
- `search_index` — Tantivy test index (needs #5)
- `vector_index` — HNSW test index (needs #7)

## Fixture Files (`tests/fixtures/`)

Static test data organized by type:

```
tests/fixtures/
├── rss/                        # RSS/Atom feed XML files
│   ├── rss_valid.xml           # RSS 2.0, 5 geopolitics entries
│   ├── atom_valid.xml          # Atom feed, 5 tech policy entries
│   ├── rss_malformed.xml       # Bad dates, missing fields, unescaped HTML
│   └── rss_empty.xml           # Valid RSS with zero items
├── html/                       # Article HTML for content extraction
│   ├── news_article.html       # Full page: nav, ads, article, comments
│   ├── blog_post.html          # Blog layout: sidebar, related posts
│   ├── technical_article.html  # Code blocks, tables
│   ├── paywall_article.html    # Truncated paywall content
│   └── minimal.html            # Bare <p> tag only
├── llm/                        # Mock LLM API responses
│   ├── tag_response.json       # Chat completion for tagging
│   ├── embedding_response.json # 8-dimensional embedding vector
│   └── error_response.json     # 429 rate limit error
└── config/
    └── test_config.toml        # Full config for test environments
```

Load fixtures in tests:

```rust
use crate::test_utils::fixtures::read_fixture;

let xml = read_fixture("rss/rss_valid.xml");
```

## Mock Server Patterns

### Mock Feed Server

```rust
use crate::test_utils::mock_http::MockFeedServer;
use crate::test_utils::fixtures::read_fixture;

let server = MockFeedServer::start().await;
let xml = read_fixture("rss/rss_valid.xml");
server.mount_feed("/feed.xml", &xml).await;

// Use server.url() as the base URL for feed fetching
let feed_url = format!("{}/feed.xml", server.url());
```

### Mock LLM Server

```rust
use crate::test_utils::mock_llm::MockLlmServer;
use crate::test_utils::fixtures::read_fixture;

let server = MockLlmServer::start().await;
let json = read_fixture("llm/tag_response.json");
server.mount_chat_completion(&json).await;

// Use server.url() as the API base URL
let api_base = server.url();
```

## SQLx Offline Mode

The project uses sqlx's offline mode for CI builds. The `.sqlx/` directory contains pre-computed query metadata and is committed to the repository.

### Updating `.sqlx/` after migration changes

```bash
./scripts/sqlx-prepare.sh
git add .sqlx/
```

### How it works

1. `scripts/sqlx-prepare.sh` creates a temporary SQLite database
2. Runs all migrations against it
3. Generates `.sqlx/` metadata via `cargo sqlx prepare`
4. CI sets `SQLX_OFFLINE=true` so `sqlx` uses the pre-computed metadata instead of a live database
5. The `sqlx-check` CI job verifies the metadata is up-to-date
