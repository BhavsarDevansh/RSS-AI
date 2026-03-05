# RSS-AI

An AI-powered RSS reader that builds a personal knowledge base from your feeds. It combines feed aggregation with semantic search, automatic tagging, cross-article linking, event detection, and entity tracking — turning a stream of articles into an interconnected knowledge graph you can query in natural language. Built in pure Rust.

## Features

- **Feed aggregation** — poll RSS/Atom feeds on configurable intervals with concurrent fetching
- **Content extraction** — fetch and clean full article text from HTML pages using readability-style extraction
- **Full-text search** — index articles with [Tantivy](https://github.com/quickwit-oss/tantivy) for fast keyword search
- **Vector similarity search** — find semantically related articles using [HNSW](https://crates.io/crates/hnsw_rs) (pure Rust, no C++ build dependency)
- **Auto-tagging** — LLM-generated tags with confidence scores, including entity extraction (people, organisations, places)
- **Knowledge graph** — discover relationships between articles (similar topic, follow-up, contradicts, same event) and build entity-to-entity connections from co-occurrence patterns
- **Event detection** — cluster articles into named events, track causal chains, and trace how developments connect over time
- **Natural language queries** — hybrid search (keyword + vector) with LLM-synthesized reports, complete with sources and confidence
- **MCP server** — expose the knowledge base to AI assistants (Claude Desktop, Claude Code, etc.) via the [Model Context Protocol](https://modelcontextprotocol.io/) over stdio and SSE transports
- **Local-first** — SQLite database with WAL mode, runs against any OpenAI-compatible API (e.g. [Ollama](https://ollama.ai/))

## Architecture

```
RSS/Atom feeds
  → fetched & parsed (feed-rs, reqwest)
    → HTML content extracted (scraper + html2text)
      → stored in SQLite (sqlx, WAL mode)
        → full-text indexed (tantivy)
        → vector indexed (hnsw_rs)
          → auto-tagged + entities extracted (LLM)
          → cross-linked + events detected (LLM)
            → queried via natural language or MCP
```

### Design Decisions

- **Pure Rust vector search** (`hnsw_rs`) instead of `usearch` to avoid C++ build dependency
- **`scraper` + `html2text`** for content extraction instead of immature readability crates
- **`sqlx`** with compile-time query verification and embedded migrations
- **Structured logging** via `tracing` with env-filter
- **`thiserror`** for library errors, **`anyhow`** for application errors
- **Entities are thin** — name, type, aliases; relationships derived from co-occurrence, not extra LLM calls
- **Full articles in query responses** — summaries are lossy; the downstream consumer gets raw material for deep analysis

## Getting Started

### Prerequisites

- Rust 1.85+ (edition 2024)
- An OpenAI-compatible API for LLM features (optional — defaults to Ollama at `localhost:11434`)

### Build

```bash
cargo build --release
```

### Configure

Generate the default config file:

```bash
cargo run -- config --generate
```

This writes a commented TOML template to `~/.config/rss-ai/config.toml`:

```toml
[service]
data_dir = "~/.local/share/rss-ai"   # SQLite DB, search index, vector store
log_level = "info"

[polling]
default_interval_minutes = 30
max_concurrent_fetches = 4

[llm]
api_base_url = "http://localhost:11434"
model = "llama3.2"
embedding_model = "nomic-embed-text"
embedding_dimensions = 768

[extraction]
user_agent = "rss-ai/0.4.0"
request_timeout_seconds = 30

[mcp]
stdio_enabled = true
sse_enabled = false
```

All settings can be overridden with environment variables using the pattern `RSS_AI_<SECTION>_<KEY>`:

```bash
RSS_AI_SERVICE_LOG_LEVEL=debug cargo run -- serve
```

### Run

```bash
cargo run -- serve                    # start with default config
cargo run -- serve --config path.toml # start with custom config
cargo run -- config                   # print current config
```

## Database

RSS-AI uses SQLite with WAL mode, foreign keys, and automatic migrations. The database is created at `{data_dir}/rss_ai.db` on first run.

**Tables:** `feeds`, `articles`, `tags`, `article_tags`, `article_links`

Triggers maintain denormalized `article_count` on tags. Cascade deletes ensure removing a feed cleans up all associated articles, tags, and links. All CRUD operations return `Result<T, DbError>` with typed errors for duplicates, not-found, and constraint violations.

## Project Status

RSS-AI is under active development. The core data pipeline is being built issue-by-issue:

| # | Component | Status |
|---|-----------|--------|
| 1 | Project scaffolding & CI | Done |
| 2 | Configuration system | Done |
| 3 | SQLite database layer | Done |
| 4 | RSS/Atom feed fetching | Open |
| 5 | Content extraction | Open |
| 6 | Full-text search (Tantivy) | Open |
| 7 | Embedding generation | Open |
| 8 | Vector similarity search | Open |
| 9 | Auto-tagging | Open |
| 10 | Article linking & knowledge graph | Open |
| 11 | Query & analysis engine | Open |
| 12 | MCP server (stdio) | Open |
| 13 | MCP server (SSE/HTTP) | Open |
| 14 | Background scheduler | Open |
| 15 | systemd service | Open |
| 16 | Event detection & causal chains | Open |
| 26 | Entity layer & knowledge graph nodes | Open |

## Development

```bash
cargo build                    # debug build
cargo test                     # run all tests
cargo fmt --check              # check formatting
cargo clippy -- -D warnings    # lint with warnings as errors
```

### Testing

Tests use in-memory SQLite databases for isolation. The `test_utils` module provides:

- `db::test_pool()` — in-memory SQLite pool with migrations applied
- `fixtures::read_fixture(path)` — load test data from `tests/fixtures/`
- `mock_http::MockFeedServer` — wiremock-based mock RSS server
- `mock_llm::MockLlmServer` — wiremock-based mock OpenAI-compatible API

See [TESTING.md](TESTING.md) for details.

### Branch Convention

Feature branches follow `issue-N/description` (e.g. `issue-3/database-layer`).

### CI

GitHub Actions runs on push/PR to `main`: format check, clippy, tests, release build.

## License

[GPL-3.0](LICENSE)
