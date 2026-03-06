/// Full-text search indexing and querying via Tantivy.
use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{
    DateTime as TantivyDateTime, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument,
    Term,
};

use crate::config::Config;

// ── Error type ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("directory open error: {0}")]
    OpenDirectory(#[from] tantivy::directory::error::OpenDirectoryError),

    #[error("query parse error: {0}")]
    QueryParse(#[from] tantivy::query::QueryParserError),

    #[error("database error: {0}")]
    Db(#[from] crate::db::DbError),

    #[error("index directory error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Schema field handles ────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct SchemaFields {
    pub article_id: Field,
    pub title: Field,
    pub content: Field,
    pub summary: Field,
    pub author: Field,
    pub tags: Field,
    pub feed_title: Field,
    pub published_at: Field,
}

// ── SearchIndex handle ──────────────────────────────────────────────

/// Pre-computed search handle that caches the schema fields, reader, and
/// query parser configuration. Create once and reuse across searches.
pub struct SearchIndex {
    index: Index,
    fields: SchemaFields,
    reader: IndexReader,
    default_search_fields: Vec<Field>,
}

impl SearchIndex {
    pub fn open(config: &Config) -> Result<Self, SearchError> {
        let index_path = config.data_dir().join("tantivy_index");
        Self::open_at(&index_path)
    }

    pub fn open_at(path: &Path) -> Result<Self, SearchError> {
        let index = open_or_create_index_at(path)?;
        Self::from_index(index)
    }

    pub fn from_index(index: Index) -> Result<Self, SearchError> {
        let fields = schema_fields(&index);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let default_search_fields =
            vec![fields.title, fields.content, fields.summary, fields.tags];
        Ok(Self {
            index,
            fields,
            reader,
            default_search_fields,
        })
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn fields(&self) -> SchemaFields {
        self.fields
    }

    pub fn reader(&self) -> &IndexReader {
        &self.reader
    }

    pub fn writer(&self) -> Result<IndexWriter, SearchError> {
        create_writer(&self.index)
    }

    /// Execute a search query. The query parser is built per-call (cheap: no
    /// allocation beyond a few vecs) but field handles and the reader are
    /// pre-resolved.
    pub fn search(
        &self,
        query_str: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>, SearchError> {
        search_inner(
            &self.reader,
            &self.index,
            self.fields,
            &self.default_search_fields,
            query_str,
            options,
        )
    }
}

// ── Public types ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub offset: usize,
    pub date_from: Option<chrono::DateTime<chrono::Utc>>,
    pub date_to: Option<chrono::DateTime<chrono::Utc>>,
    pub feed_filter: Option<Vec<String>>,
    pub sort_by: SortBy,
    pub recency_boost: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            offset: 0,
            date_from: None,
            date_to: None,
            feed_filter: None,
            sort_by: SortBy::Relevance,
            recency_boost: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Relevance,
    Date,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub article_id: i64,
    pub score: f32,
    pub title: String,
    pub snippet: String,
    pub published_at: Option<String>,
    pub feed_title: String,
}

/// Data needed to index an article. Avoids coupling to db models directly.
pub struct ArticleIndexData<'a> {
    pub article_id: i64,
    pub title: &'a str,
    pub content: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub author: Option<&'a str>,
    pub tags: &'a [String],
    pub feed_title: Option<&'a str>,
    pub published_at: Option<&'a str>,
}

// ── Constants ───────────────────────────────────────────────────────

const DEFAULT_WRITER_MEMORY_BYTES: usize = 50_000_000; // 50 MB
const SNIPPET_MAX_LEN: usize = 200;
/// When post-filters are active, fetch this many extra candidates per
/// filtered-out result to compensate. 3× is a good balance.
const OVERFETCH_FACTOR: usize = 3;

// ── Schema construction ─────────────────────────────────────────────

fn build_schema() -> (Schema, SchemaFields) {
    let mut builder = Schema::builder();

    let article_id = builder.add_u64_field("article_id", STORED | INDEXED);

    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("en_stem")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();

    let text_not_stored = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );

    let string_stored = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::Basic),
        )
        .set_stored();

    let title = builder.add_text_field("title", text_options.clone());
    let content = builder.add_text_field("content", text_options);
    let summary = builder.add_text_field("summary", text_not_stored.clone());
    let author = builder.add_text_field("author", string_stored.clone());
    let tags = builder.add_text_field("tags", text_not_stored);
    let feed_title = builder.add_text_field("feed_title", string_stored);

    let date_options = DateOptions::default()
        .set_stored()
        .set_indexed()
        .set_precision(DateTimePrecision::Seconds);
    let published_at = builder.add_date_field("published_at", date_options);

    let schema = builder.build();
    let fields = SchemaFields {
        article_id,
        title,
        content,
        summary,
        author,
        tags,
        feed_title,
        published_at,
    };

    (schema, fields)
}

// ── Index management ────────────────────────────────────────────────

/// Open or create a Tantivy index at `{data_dir}/tantivy_index/`.
pub fn open_or_create_index(config: &Config) -> Result<Index, SearchError> {
    let index_path = config.data_dir().join("tantivy_index");
    open_or_create_index_at(&index_path)
}

/// Open or create a Tantivy index at an arbitrary path (useful for tests).
pub fn open_or_create_index_at(path: &Path) -> Result<Index, SearchError> {
    std::fs::create_dir_all(path)?;
    let (schema, _fields) = build_schema();
    let index = Index::open_or_create(tantivy::directory::MmapDirectory::open(path)?, schema)?;
    Ok(index)
}

/// Get the schema fields from an existing index.
pub fn schema_fields(index: &Index) -> SchemaFields {
    let schema = index.schema();
    SchemaFields {
        article_id: schema.get_field("article_id").unwrap(),
        title: schema.get_field("title").unwrap(),
        content: schema.get_field("content").unwrap(),
        summary: schema.get_field("summary").unwrap(),
        author: schema.get_field("author").unwrap(),
        tags: schema.get_field("tags").unwrap(),
        feed_title: schema.get_field("feed_title").unwrap(),
        published_at: schema.get_field("published_at").unwrap(),
    }
}

/// Create a shared reader with reload-on-commit policy.
pub fn create_reader(index: &Index) -> Result<IndexReader, SearchError> {
    Ok(index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()?)
}

/// Create a writer with the default memory budget (50 MB).
pub fn create_writer(index: &Index) -> Result<IndexWriter, SearchError> {
    Ok(index.writer(DEFAULT_WRITER_MEMORY_BYTES)?)
}

/// Add or update a single article in the index. Commits immediately.
pub fn index_article(
    writer: &mut IndexWriter,
    fields: &SchemaFields,
    data: &ArticleIndexData<'_>,
) -> Result<(), SearchError> {
    writer.delete_term(Term::from_field_u64(fields.article_id, data.article_id as u64));
    let doc = build_document(fields, data);
    writer.add_document(doc)?;
    writer.commit()?;
    Ok(())
}

/// Batch index multiple articles in one commit.
pub fn index_articles_batch(
    writer: &mut IndexWriter,
    fields: &SchemaFields,
    articles: &[ArticleIndexData<'_>],
) -> Result<usize, SearchError> {
    for data in articles {
        writer.delete_term(Term::from_field_u64(fields.article_id, data.article_id as u64));
        let doc = build_document(fields, data);
        writer.add_document(doc)?;
    }
    writer.commit()?;
    Ok(articles.len())
}

/// Remove an article from the index by article_id.
pub fn delete_article(
    writer: &mut IndexWriter,
    fields: &SchemaFields,
    article_id: i64,
) -> Result<(), SearchError> {
    writer.delete_term(Term::from_field_u64(fields.article_id, article_id as u64));
    writer.commit()?;
    Ok(())
}

/// Drop all documents and rebuild the index from the database.
pub async fn rebuild_index(
    index: &Index,
    pool: &sqlx::SqlitePool,
) -> Result<usize, SearchError> {
    let fields = schema_fields(index);
    let mut writer = create_writer(index)?;
    writer.delete_all_documents()?;
    writer.commit()?;

    // Fetch all articles with extracted content
    let articles: Vec<crate::db::Article> = sqlx::query_as(
        "SELECT * FROM articles WHERE content_extracted = 1 ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .map_err(crate::db::DbError::from)?;

    // Fetch feed titles for all feeds
    let feeds: Vec<crate::db::Feed> = sqlx::query_as("SELECT * FROM feeds")
        .fetch_all(pool)
        .await
        .map_err(crate::db::DbError::from)?;

    let feed_titles: std::collections::HashMap<i64, String> = feeds
        .into_iter()
        .filter_map(|f| f.title.map(|t| (f.id, t)))
        .collect();

    // Batch-fetch all article-tag associations in a single query
    let all_tag_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT at.article_id, t.name
         FROM article_tags at
         JOIN tags t ON t.id = at.tag_id
         ORDER BY at.article_id",
    )
    .fetch_all(pool)
    .await
    .map_err(crate::db::DbError::from)?;

    let mut tags_by_article: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    for (article_id, tag_name) in all_tag_rows {
        tags_by_article
            .entry(article_id)
            .or_default()
            .push(tag_name);
    }

    let empty_tags: Vec<String> = Vec::new();
    for article in &articles {
        let tags = tags_by_article.get(&article.id).unwrap_or(&empty_tags);
        let feed_title = feed_titles.get(&article.feed_id).map(String::as_str);

        let data = ArticleIndexData {
            article_id: article.id,
            title: &article.title,
            content: article.content.as_deref(),
            summary: article.summary.as_deref(),
            author: article.author.as_deref(),
            tags,
            feed_title,
            published_at: article.published_at.as_deref(),
        };

        let doc = build_document(&fields, &data);
        writer.add_document(doc)?;
    }

    writer.commit()?;
    Ok(articles.len())
}

// ── Search ──────────────────────────────────────────────────────────

/// Search the index with the given query string and options.
/// Prefer `SearchIndex::search()` for repeated queries — it avoids
/// re-resolving schema fields on every call.
pub fn search(
    reader: &IndexReader,
    index: &Index,
    query_str: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchResult>, SearchError> {
    let fields = schema_fields(index);
    let default_fields = vec![fields.title, fields.content, fields.summary, fields.tags];
    search_inner(reader, index, fields, &default_fields, query_str, options)
}

fn search_inner(
    reader: &IndexReader,
    index: &Index,
    fields: SchemaFields,
    default_search_fields: &[Field],
    query_str: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchResult>, SearchError> {
    let searcher = reader.searcher();

    let mut query_parser = QueryParser::for_index(index, default_search_fields.to_vec());
    query_parser.set_field_boost(fields.title, 2.0);
    query_parser.set_field_boost(fields.content, 1.0);
    query_parser.set_field_boost(fields.summary, 1.5);

    let parsed_query = query_parser.parse_query(query_str)?;

    // Overfetch when post-filters are active to compensate for discarded results
    let has_post_filter =
        options.date_from.is_some() || options.date_to.is_some() || options.feed_filter.is_some();
    let top_n = if has_post_filter {
        (options.offset + options.limit) * OVERFETCH_FACTOR
    } else {
        options.offset + options.limit
    };

    let top_docs = searcher.search(&parsed_query, &TopDocs::with_limit(top_n))?;

    // Pre-compute timestamp bounds to avoid per-result chrono conversions
    let ts_from = options.date_from.map(|d| d.timestamp());
    let ts_to = options.date_to.map(|d| d.timestamp());

    // Pre-compute query terms for snippet generation (done once, not per doc)
    let query_lower = query_str.to_lowercase();
    let snippet_terms: Vec<&str> = query_lower
        .split_whitespace()
        .filter(|t| !matches!(*t, "and" | "or" | "not"))
        .map(|t| t.trim_matches('"'))
        .filter(|t| !t.is_empty())
        .collect();

    let now_ts = if options.recency_boost {
        chrono::Utc::now().timestamp()
    } else {
        0
    };

    let mut results = Vec::with_capacity(options.limit);
    let mut skipped = 0usize;

    for (mut score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;

        // Extract published_at as a raw timestamp — avoids string round-tripping
        let pub_timestamp_secs: Option<i64> = doc
            .get_first(fields.published_at)
            .and_then(|v| v.as_datetime())
            .map(|dt| dt.into_timestamp_secs());

        // Date range filtering on raw timestamps (no string parsing)
        if let Some(from_ts) = ts_from {
            match pub_timestamp_secs {
                Some(ts) if ts < from_ts => continue,
                None => continue,
                _ => {}
            }
        }
        if let Some(to_ts) = ts_to {
            match pub_timestamp_secs {
                Some(ts) if ts > to_ts => continue,
                None => continue,
                _ => {}
            }
        }

        let feed_title = doc
            .get_first(fields.feed_title)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Feed title filter
        if let Some(ref feed_filter) = options.feed_filter
            && !feed_filter.iter().any(|f| f == feed_title)
        {
            continue;
        }

        // Handle offset via skip counter (after filtering, so offset is
        // relative to the filtered result set)
        if skipped < options.offset {
            skipped += 1;
            continue;
        }

        // Recency boost on raw timestamps — no string parsing
        if options.recency_boost
            && let Some(pub_ts) = pub_timestamp_secs
        {
            let age_days = ((now_ts - pub_ts) as f32 / 86400.0).max(0.0);
            let boost = (-age_days / 30.0 * 0.693).exp();
            score *= 1.0 + boost;
        }

        let article_id = doc
            .get_first(fields.article_id)
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as i64;

        let title = doc
            .get_first(fields.title)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content_text = doc
            .get_first(fields.content)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let snippet = generate_snippet(content_text, &snippet_terms);

        let published_at = pub_timestamp_secs.and_then(|ts| {
            chrono::DateTime::from_timestamp(ts, 0).map(|d| d.to_rfc3339())
        });

        results.push(SearchResult {
            article_id,
            score,
            title,
            snippet,
            published_at,
            feed_title: feed_title.to_string(),
        });

        if results.len() >= options.limit {
            break;
        }
    }

    // Re-sort if date-based sorting requested
    if options.sort_by == SortBy::Date {
        results.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    }

    Ok(results)
}

// ── Private helpers ─────────────────────────────────────────────────

fn build_document(fields: &SchemaFields, data: &ArticleIndexData<'_>) -> TantivyDocument {
    let mut doc = TantivyDocument::new();
    doc.add_u64(fields.article_id, data.article_id as u64);
    doc.add_text(fields.title, data.title);
    doc.add_text(fields.content, data.content.unwrap_or(""));
    doc.add_text(fields.summary, data.summary.unwrap_or(""));
    doc.add_text(fields.author, data.author.unwrap_or(""));
    doc.add_text(fields.tags, data.tags.join(" "));
    doc.add_text(fields.feed_title, data.feed_title.unwrap_or(""));

    if let Some(pub_str) = data.published_at
        && let Some(dt) = parse_to_tantivy_datetime(pub_str)
    {
        doc.add_date(fields.published_at, dt);
    }

    doc
}

fn parse_to_tantivy_datetime(s: &str) -> Option<TantivyDateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(TantivyDateTime::from_timestamp_secs(dt.timestamp()));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(TantivyDateTime::from_timestamp_secs(
            dt.and_utc().timestamp(),
        ));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0)?;
        return Some(TantivyDateTime::from_timestamp_secs(
            dt.and_utc().timestamp(),
        ));
    }
    None
}

/// Generate a context snippet around the first matching query term.
/// `terms` should be pre-lowercased and stripped of query syntax.
fn generate_snippet(content: &str, terms: &[&str]) -> String {
    // Only lowercase the first portion of content we need to scan — up to
    // the first match + SNIPPET_MAX_LEN should be enough.  For very long
    // articles this avoids lowercasing megabytes of text.
    let scan_limit = content.len().min(SNIPPET_MAX_LEN * 10);
    let scan_region = &content[..scan_limit];
    let scan_lower = scan_region.to_lowercase();

    let mut best_pos = None;
    for term in terms {
        if let Some(pos) = scan_lower.find(term)
            && (best_pos.is_none() || pos < best_pos.unwrap())
        {
            best_pos = Some(pos);
        }
    }

    let start = best_pos.unwrap_or(0);
    let snippet_start = if start > 40 {
        content[..start]
            .rfind(' ')
            .map(|p| p + 1)
            .unwrap_or(start.saturating_sub(40))
    } else {
        0
    };

    let end = (snippet_start + SNIPPET_MAX_LEN).min(content.len());
    let snippet_end = if end < content.len() {
        content[end..].find(' ').map(|p| end + p).unwrap_or(end)
    } else {
        end
    };

    let mut snippet = content[snippet_start..snippet_end].to_string();
    if snippet_start > 0 {
        snippet.insert_str(0, "...");
    }
    if snippet_end < content.len() {
        snippet.push_str("...");
    }
    snippet
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> (SearchIndex, IndexWriter, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let si = SearchIndex::open_at(dir.path()).unwrap();
        let writer = si.writer().unwrap();
        (si, writer, dir)
    }

    fn sample_data<'a>(tags: &'a [String]) -> ArticleIndexData<'a> {
        ArticleIndexData {
            article_id: 1,
            title: "Rust Programming Language",
            content: Some("Rust is a systems programming language focused on safety and performance. It prevents memory errors without a garbage collector."),
            summary: Some("An introduction to Rust"),
            author: Some("Jane Doe"),
            tags,
            feed_title: Some("Tech Blog"),
            published_at: Some("2025-06-15T12:00:00+00:00"),
        }
    }

    #[test]
    fn test_index_and_search_by_keyword() {
        let (si, mut writer, _dir) = test_index();
        let tags = vec!["programming".to_string(), "rust".to_string()];
        let data = sample_data(&tags);

        index_article(&mut writer, &si.fields(), &data).unwrap();
        si.reader().reload().unwrap();

        let results = si.search("rust safety", &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].article_id, 1);
        assert!(!results[0].title.is_empty());
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_phrase_search() {
        let (si, mut writer, _dir) = test_index();
        let fields = si.fields();
        let tags = vec![];
        let data = ArticleIndexData {
            article_id: 1,
            title: "Fuel Prices Today",
            content: Some("The fuel prices have risen dramatically in the past week. Fuel prices affect the economy broadly."),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("News"),
            published_at: Some("2025-06-15T12:00:00+00:00"),
        };
        index_article(&mut writer, &fields, &data).unwrap();

        let data2 = ArticleIndexData {
            article_id: 2,
            title: "Fuel Efficiency Tips",
            content: Some("Here are some tips to save fuel. Prices of other commodities are stable."),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("News"),
            published_at: Some("2025-06-14T12:00:00+00:00"),
        };
        index_article(&mut writer, &fields, &data2).unwrap();
        si.reader().reload().unwrap();

        let results = si
            .search("\"fuel prices\"", &SearchOptions::default())
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].article_id, 1);
    }

    #[test]
    fn test_date_range_filtering() {
        let (si, mut writer, _dir) = test_index();
        let fields = si.fields();
        let tags = vec![];

        let old = ArticleIndexData {
            article_id: 1,
            title: "Old Article",
            content: Some("This is an old article about technology"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: Some("2024-01-15T12:00:00+00:00"),
        };
        index_article(&mut writer, &fields, &old).unwrap();

        let new = ArticleIndexData {
            article_id: 2,
            title: "New Article",
            content: Some("This is a new article about technology"),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Blog"),
            published_at: Some("2025-06-15T12:00:00+00:00"),
        };
        index_article(&mut writer, &fields, &new).unwrap();
        si.reader().reload().unwrap();

        let options = SearchOptions {
            date_from: Some(
                chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            ),
            ..Default::default()
        };

        let results = si.search("technology", &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].article_id, 2);
    }

    #[test]
    fn test_search_results_include_snippets() {
        let (si, mut writer, _dir) = test_index();
        let fields = si.fields();
        let tags = vec![];
        let data = ArticleIndexData {
            article_id: 1,
            title: "Memory Safety",
            content: Some("Rust provides memory safety without garbage collection. The borrow checker ensures references are always valid. This makes Rust unique among systems languages."),
            summary: None,
            author: None,
            tags: &tags,
            feed_title: Some("Tech"),
            published_at: Some("2025-06-15T12:00:00+00:00"),
        };
        index_article(&mut writer, &fields, &data).unwrap();
        si.reader().reload().unwrap();

        let results = si
            .search("borrow checker", &SearchOptions::default())
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].snippet.is_empty());
    }

    #[test]
    fn test_rebuild_index_from_database() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pool = crate::test_utils::db::test_pool().await;

            let feed =
                crate::db::feeds::add_feed(&pool, "https://example.com/rss", Some("My Feed"))
                    .await
                    .unwrap();

            let article = crate::db::articles::insert_article(
                &pool,
                &crate::db::NewArticle {
                    feed_id: feed.id,
                    guid: Some("g1".to_string()),
                    url: "https://example.com/post".to_string(),
                    title: "Indexed Article".to_string(),
                    author: Some("Author".to_string()),
                    published_at: Some("2025-06-15T12:00:00+00:00".to_string()),
                    summary: Some("Summary text".to_string()),
                    content: None,
                    content_hash: None,
                },
            )
            .await
            .unwrap();

            crate::db::articles::update_article_content(
                &pool,
                article.id,
                "Full extracted content about Rust programming",
                "hash123",
            )
            .await
            .unwrap();

            let dir = tempfile::tempdir().unwrap();
            let index = open_or_create_index_at(dir.path()).unwrap();
            let count = rebuild_index(&index, &pool).await.unwrap();
            assert_eq!(count, 1);

            let si = SearchIndex::from_index(index).unwrap();
            si.reader().reload().unwrap();
            let results = si
                .search("Rust programming", &SearchOptions::default())
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].article_id, article.id);
            assert_eq!(results[0].feed_title, "My Feed");
        });
    }

    #[test]
    fn test_delete_article_removes_from_search() {
        let (si, mut writer, _dir) = test_index();
        let fields = si.fields();
        let tags = vec![];
        let data = sample_data(&tags);

        index_article(&mut writer, &fields, &data).unwrap();
        si.reader().reload().unwrap();

        let results = si.search("rust", &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 1);

        delete_article(&mut writer, &fields, 1).unwrap();
        si.reader().reload().unwrap();

        let results = si.search("rust", &SearchOptions::default()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_zero_results_returns_empty() {
        let (si, _writer, _dir) = test_index();

        let results = si
            .search("nonexistent query terms xyz", &SearchOptions::default())
            .unwrap();
        assert!(results.is_empty());
    }
}
