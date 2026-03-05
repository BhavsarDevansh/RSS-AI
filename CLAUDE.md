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
- `mock_http::MockFeedServer` — wiremock-based mock RSS/HTTP server
- `mock_llm::MockLlmServer` — wiremock-based mock OpenAI-compatible API

Fixture files live in `tests/fixtures/{rss,html,llm,config}/`.

## Architecture

RSS-AI is an AI-powered RSS reader combining feed aggregation with semantic search and content analysis. It exposes functionality via a CLI (`clap` derive API) with `serve` and `config` subcommands.

**Data pipeline:** feeds are fetched (`feed`) → HTML content extracted (`extractor` using `scraper` + `html2text`) → stored in SQLite (`db` via `sqlx`) → indexed for full-text search (`search` via `tantivy`) and vector similarity search (`vector` via `hnsw_rs`) → auto-tagged (`tagger`) and cross-linked (`linker`) → queried via natural language (`query`) or MCP protocol (`mcp`). The `scheduler` handles periodic feed polling.

**Key design decisions:**
- Pure Rust vector search (`hnsw_rs`) instead of `usearch` to avoid C++ build dependency
- `scraper` + `html2text` for content extraction instead of immature readability crates
- `sqlx` with compile-time query verification and migrations
- Structured logging via `tracing` with env-filter
- `thiserror` for library errors, `anyhow` for application errors

## CI

GitHub Actions on push/PR to `main`: fmt check → clippy → test → release build. Uses `dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2`.

## Branch Convention

Feature branches follow `issue-N/description` pattern (e.g., `issue-1/project-scaffolding`).

## Versioning

Uses Semantic Version. Every update should change the application version number in cargo.toml
