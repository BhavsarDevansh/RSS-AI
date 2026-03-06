# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # Run all tests
cargo test <test_name>         # Run a single test
cargo fmt --check              # Check formatting (CI enforces this)
cargo clippy -- -D warnings    # Lint with warnings as errors (CI enforces this)
cargo run -- serve             # Start the RSS-AI service
cargo run -- config            # Print default TOML config template
```

## Testing

The `src/test_utils/` module provides shared test infrastructure:
- `db::test_pool()` — in-memory SQLite pool with migrations applied
- `fixtures::read_fixture(path)` — loads files from `tests/fixtures/`
- `mock_http::MockFeedServer` — wiremock-based mock RSS/HTTP server (exposes `.server()` for custom wiremock mounts)
- `mock_llm::MockLlmServer` — wiremock-based mock OpenAI-compatible API

Fixture files live in `tests/fixtures/{rss,html,llm,config}/`.

## Architecture

RSS-AI is an AI-powered RSS reader combining feed aggregation with semantic search and content analysis. It exposes functionality via a CLI (`clap` derive API) with `serve` and `config` subcommands.

**Data pipeline:** feeds are fetched (`feed` using `feed-rs` + `reqwest`) → full article content extracted (`extractor` using `scraper` + `html2text` + `sha2`, with robots.txt compliance and per-domain rate limiting) → stored in SQLite (`db` via `sqlx`) → indexed for full-text search (`search` via `tantivy`) and vector similarity search (`vector` via `hnsw_rs`) → auto-tagged (`tagger`) and cross-linked (`linker`) → queried via natural language (`query`) or MCP protocol (`mcp`). The `scheduler` handles periodic feed polling.

### Implemented modules

- **`config`** — TOML config with env overrides, validation, tilde expansion
- **`db/`** — SQLite persistence (module directory, not single file):
  - `mod.rs` — `DbError`, `init_pool(data_dir)` (WAL, FK, busy_timeout, migrations)
  - `models.rs` — `Feed`, `Article` (includes `word_count`), `Tag`, `ArticleTag`, `ArticleLink`, `NewArticle`, etc.
  - `feeds.rs` — CRUD + poll status + HTTP cache headers + active feed listing
  - `articles.rs` — insert/batch/get/exists/update content/mark flags/search/recent; `ExtractedArticleUpdate` struct + `update_article_content_with_metadata` for enriched extraction results
  - `tags.rs` — get_or_create/add to article/query by tag/list/top
  - `links.rs` — add/get/bidirectional/BFS graph traversal
- **`feed`** — RSS/Atom fetching with conditional requests (ETag/If-Modified-Since), deduplication, concurrent fetching via semaphore, error isolation per feed
- **`extractor`** — Full article content extraction pipeline: fetches article URLs, extracts readable text/title/author/date from HTML (using `scraper` + `html2text`), respects `robots.txt` (cached), per-domain rate limiting (1 req/s), SHA-256 content hashing (`sha2`), word counting, boilerplate filtering. `ExtractedContent` struct. `process_pending_articles` batch processes unextracted articles. `ExtractorError` error type.
- **`search`** — Tantivy full-text search index at `{data_dir}/tantivy_index/`. `SearchIndex` struct caches schema fields, reader, and default search fields for zero-overhead repeated queries. Schema: `article_id` (u64), `title`/`content`/`summary` (en_stem tokenizer), `author`/`feed_title` (default tokenizer, stored), `tags` (en_stem, not stored), `published_at` (date). Title boosted 2×, summary 1.5×. Date filtering and recency boost operate on raw timestamps (no string round-tripping). Snippet generation caps lowercase scan to avoid processing megabytes. `rebuild_index` batch-fetches all tags in one query. 50 MB default writer memory budget, `ReloadPolicy::OnCommitWithDelay`, 3× overfetch when post-filters active. `SearchError` error type.
- **`embeddings/`** — Embedding generation via OpenAI-compatible `/v1/embeddings` API (module directory):
  - `mod.rs` — re-exports only
  - `error.rs` — `EmbeddingError` error type
  - `types.rs` — API request/response data types + `EmbeddingBatchResult`
  - `client.rs` — `EmbeddingClient` wrapping `reqwest::Client`; `embed_text`/`embed_batch` with retry (3× exponential backoff on 5xx/429/connection errors), dimension validation
  - `text.rs` — `prepare_article_text` (title+content concat with null-byte/control-char sanitisation), `prepare_input` (zero-copy `Cow<str>` truncation to ~8192 tokens)
  - `pipeline.rs` — `process_pending_articles` batch pipeline (default batch size 10), marks `embedding_generated` flag, returns `(article_id, Vec<f32>)` pairs for vector index storage
  - `{client,text,pipeline}_tests.rs` — tests for each concern, compiled only under `#[cfg(test)]`

### Module organisation
Each file should have exactly one distinct concern. When a module has multiple concerns (e.g. error type, data types, client logic, pipeline), split it into a directory module (`mod.rs` + subfiles). Follow the pattern used by `embeddings/`:
- `mod.rs` — re-exports only (no logic)
- `error.rs` — error type
- `types.rs` — data types / request-response structs
- One file per logical concern (e.g. `client.rs`, `pipeline.rs`, `text.rs`)
- Tests in separate `*_tests.rs` files, declared as `#[cfg(test)] mod *_tests;` in `mod.rs`

Files may be small — that is fine. Clarity and maintainability are more important than file size.

### Key design decisions
- **Runtime queries** (`sqlx::query` / `sqlx::query_as` without `!`) — avoids compile-time DB dependency
- Pure Rust vector search (`hnsw_rs`) instead of `usearch` to avoid C++ build dependency
- `scraper` + `html2text` for content extraction with custom boilerplate filtering instead of immature readability crates
- `sqlx` with embedded migrations (`sqlx::migrate!()`)
- Structured logging via `tracing` with env-filter
- `thiserror` for library errors, `anyhow` for application errors
- Each module has its own error type (e.g. `DbError`, `FeedError`)

## Database

SQLite with WAL mode, foreign keys, 5s busy timeout. Migrations in `migrations/` dir (4 files). Tables: `feeds`, `articles`, `tags`, `article_tags`, `article_links`. The `articles` table includes a `word_count` column populated during content extraction. Triggers maintain `tags.article_count`. Cascade deletes on feed removal.

## CI

GitHub Actions on push/PR to `main`: fmt check → clippy → test → release build. Uses `dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2`.

## Branch Convention

Feature branches follow `issue-N/description` pattern (e.g., `issue-3/database-layer`).

## Versioning

Uses Semantic Versioning. Every update should change the application version number in Cargo.toml and the user-agent string in `src/config.rs`.
