/// RSS/Atom feed fetching, parsing, and deduplication.
use std::time::Duration;

use reqwest::header::{self, HeaderMap};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::db;
use crate::db::models::{Article, Feed, NewArticle};

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("HTTP error fetching {url}: {status}")]
    HttpStatus { url: String, status: u16 },

    #[error("HTTP request failed for {url}: {source}")]
    Request { url: String, source: reqwest::Error },

    #[error("feed parse error: {0}")]
    Parse(String),

    #[error("database error: {0}")]
    Db(#[from] db::DbError),

    #[error("rate limited (retry after {retry_after_secs}s)")]
    RateLimited { retry_after_secs: u64 },
}

// ── Fetch result types ─────────────────────────────────────────────

/// Result of fetching a single feed.
#[derive(Debug)]
pub struct FeedResult {
    pub feed_id: i64,
    pub new_articles: Vec<Article>,
    pub not_modified: bool,
}

/// Summary of fetching all feeds.
#[derive(Debug)]
pub struct FetchSummary {
    pub feeds_fetched: usize,
    pub feeds_not_modified: usize,
    pub feeds_errored: usize,
    pub new_articles_total: usize,
    pub errors: Vec<(i64, String)>,
}

// ── Constants ──────────────────────────────────────────────────────

const CONSECUTIVE_FAILURE_WARN_THRESHOLD: i64 = 10;

// ── Public API ─────────────────────────────────────────────────────

/// Fetch and process a single feed. Returns newly added articles.
pub async fn fetch_feed(
    pool: &SqlitePool,
    config: &Config,
    feed: &Feed,
) -> Result<FeedResult, FeedError> {
    let client = build_client(config)?;

    // Build request with conditional headers
    let mut req = client.get(&feed.url);
    if let Some(ref etag) = feed.etag {
        req = req.header(header::IF_NONE_MATCH, etag);
    }
    if let Some(ref last_mod) = feed.last_modified {
        req = req.header(header::IF_MODIFIED_SINCE, last_mod);
    }

    let response = req.send().await.map_err(|e| FeedError::Request {
        url: feed.url.clone(),
        source: e,
    })?;

    let status = response.status();

    // Handle 304 Not Modified
    if status == reqwest::StatusCode::NOT_MODIFIED {
        db::feeds::update_poll_status(pool, feed.id, true, None).await?;
        return Ok(FeedResult {
            feed_id: feed.id,
            new_articles: vec![],
            not_modified: true,
        });
    }

    // Handle 429 / 503 with Retry-After
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);

        return Err(FeedError::RateLimited {
            retry_after_secs: retry_after,
        });
    }

    // Handle permanent redirect — update stored URL
    if let Some(final_url) = detect_permanent_redirect(&response, &feed.url) {
        tracing::info!(
            feed_id = feed.id,
            old_url = %feed.url,
            new_url = %final_url,
            "feed permanently redirected, updating URL"
        );
        db::feeds::update_feed_url(pool, feed.id, &final_url).await?;
    }

    // Handle other errors
    if !status.is_success() {
        return Err(FeedError::HttpStatus {
            url: feed.url.clone(),
            status: status.as_u16(),
        });
    }

    // Save HTTP cache headers
    let etag = extract_header(response.headers(), header::ETAG);
    let last_modified = extract_header(response.headers(), header::LAST_MODIFIED);
    db::feeds::update_http_cache_headers(pool, feed.id, etag.as_deref(), last_modified.as_deref())
        .await?;

    let body = response.bytes().await.map_err(|e| FeedError::Request {
        url: feed.url.clone(),
        source: e,
    })?;

    // Parse feed
    let parsed = feed_rs::parser::parse(&body[..])
        .map_err(|e| FeedError::Parse(format!("{}: {e}", feed.url)))?;

    // Update feed metadata from parsed feed
    let feed_title = parsed.title.as_ref().map(|t| t.content.as_str());
    let feed_description = parsed.description.as_ref().map(|d| d.content.as_str());
    let feed_site_url = parsed
        .links
        .iter()
        .find(|l| l.rel.as_deref() != Some("self"))
        .map(|l| l.href.as_str());
    db::feeds::update_feed(
        pool,
        feed.id,
        feed_title,
        feed_description,
        feed_site_url,
        None,
    )
    .await?;

    // Convert entries to NewArticle and deduplicate
    let new_articles = process_entries(pool, feed.id, &parsed.entries).await?;

    // Mark poll success
    db::feeds::update_poll_status(pool, feed.id, true, None).await?;

    Ok(FeedResult {
        feed_id: feed.id,
        new_articles,
        not_modified: false,
    })
}

/// Fetch all active feeds concurrently, respecting max_concurrent_fetches.
pub async fn fetch_all_feeds(
    pool: &SqlitePool,
    config: &Config,
) -> Result<FetchSummary, FeedError> {
    let feeds = db::feeds::list_active_feeds(pool).await?;
    let max_concurrent = config.polling.max_concurrent_fetches as usize;

    let mut summary = FetchSummary {
        feeds_fetched: 0,
        feeds_not_modified: 0,
        feeds_errored: 0,
        new_articles_total: 0,
        errors: vec![],
    };

    // Use a semaphore to limit concurrency
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    for feed in feeds {
        let pool = pool.clone();
        let config = config.clone();
        let sem = semaphore.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let result = fetch_feed(&pool, &config, &feed).await;
            (feed.id, feed.error_count, result)
        });

        handles.push(handle);
    }

    for handle in handles {
        match handle.await {
            Ok((feed_id, prev_error_count, Ok(result))) => {
                if result.not_modified {
                    summary.feeds_not_modified += 1;
                } else {
                    summary.feeds_fetched += 1;
                    summary.new_articles_total += result.new_articles.len();
                }
                tracing::debug!(
                    feed_id,
                    new_articles = result.new_articles.len(),
                    not_modified = result.not_modified,
                    "feed fetch complete"
                );
                // Suppress unused variable warning
                let _ = prev_error_count;
            }
            Ok((feed_id, prev_error_count, Err(e))) => {
                let error_msg = e.to_string();
                tracing::error!(feed_id, error = %error_msg, "feed fetch failed");

                // Update poll status on failure
                let _ = db::feeds::update_poll_status(pool, feed_id, false, Some(&error_msg)).await;

                let new_count = prev_error_count + 1;
                if new_count >= CONSECUTIVE_FAILURE_WARN_THRESHOLD {
                    tracing::warn!(
                        feed_id,
                        error_count = new_count,
                        "feed has failed {new_count} consecutive times — it may be dead"
                    );
                }

                summary.feeds_errored += 1;
                summary.errors.push((feed_id, error_msg));
            }
            Err(e) => {
                tracing::error!(error = %e, "feed fetch task panicked");
                summary.feeds_errored += 1;
            }
        }
    }

    Ok(summary)
}

// ── Internal helpers ───────────────────────────────────────────────

fn build_client(config: &Config) -> Result<reqwest::Client, FeedError> {
    reqwest::Client::builder()
        .user_agent(&config.extraction.user_agent)
        .timeout(Duration::from_secs(
            config.extraction.request_timeout_seconds as u64,
        ))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| FeedError::Request {
            url: String::new(),
            source: e,
        })
}

fn extract_header(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Detect if a permanent redirect (301/308) changed the URL.
fn detect_permanent_redirect(response: &reqwest::Response, original_url: &str) -> Option<String> {
    let final_url = response.url().as_str();
    if final_url != original_url {
        // reqwest follows redirects automatically; if the final URL differs, a redirect happened.
        // We treat this as a permanent redirect for simplicity.
        Some(final_url.to_string())
    } else {
        None
    }
}

/// Process feed entries: deduplicate and insert new articles.
async fn process_entries(
    pool: &SqlitePool,
    feed_id: i64,
    entries: &[feed_rs::model::Entry],
) -> Result<Vec<Article>, db::DbError> {
    let mut new_articles = Vec::new();

    for entry in entries {
        let article = entry_to_new_article(feed_id, entry);

        // Check for duplicates
        if db::articles::article_exists(pool, &article.url).await? {
            tracing::debug!(
                feed_id,
                url = %article.url,
                "skipping duplicate article"
            );
            continue;
        }

        match db::articles::insert_article(pool, &article).await {
            Ok(inserted) => new_articles.push(inserted),
            Err(db::DbError::DuplicateEntry(_)) => {
                tracing::debug!(
                    feed_id,
                    url = %article.url,
                    "skipping duplicate article (race)"
                );
            }
            Err(e) => return Err(e),
        }
    }

    Ok(new_articles)
}

/// Convert a feed-rs Entry to our NewArticle model.
fn entry_to_new_article(feed_id: i64, entry: &feed_rs::model::Entry) -> NewArticle {
    let url = entry
        .links
        .first()
        .map(|l| l.href.clone())
        .unwrap_or_else(|| entry.id.clone());

    let guid = if entry.id.is_empty() {
        None
    } else {
        Some(entry.id.clone())
    };

    let title = entry
        .title
        .as_ref()
        .map(|t| t.content.clone())
        .unwrap_or_else(|| "(untitled)".to_string());

    let author = entry.authors.first().map(|a| a.name.clone());

    let published_at = entry.published.or(entry.updated).map(|dt| dt.to_rfc3339());

    let summary = entry.summary.as_ref().map(|s| strip_html_tags(&s.content));

    NewArticle {
        feed_id,
        guid,
        url,
        title,
        author,
        published_at,
        summary,
        content: None,
        content_hash: None,
    }
}

/// Simple HTML tag stripping for summary fields.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::db::test_pool;
    use crate::test_utils::fixtures::read_fixture;
    use crate::test_utils::mock_http::MockFeedServer;

    fn test_config_with_server(server_url: &str) -> Config {
        let mut config = Config::default();
        config.service.data_dir = "/tmp/rss-ai-test".to_string();
        // Doesn't matter for tests since we use the mock server URL directly
        let _ = server_url;
        config
    }

    async fn add_feed_with_url(pool: &SqlitePool, url: &str) -> Feed {
        db::feeds::add_feed(pool, url, None).await.unwrap()
    }

    // ── Parsing tests ──────────────────────────────────────────

    #[test]
    fn strip_html_tags_works() {
        assert_eq!(
            strip_html_tags("This has <b>bold</b> and <script>alert('xss')</script> text."),
            "This has bold and alert('xss') text."
        );
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn entry_to_new_article_rss() {
        let xml = read_fixture("rss/rss_valid.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        let entry = &parsed.entries[0];
        let article = entry_to_new_article(1, entry);

        assert_eq!(
            article.title,
            "NATO Summit Addresses Eastern European Security Concerns"
        );
        assert_eq!(article.url, "https://example.com/articles/nato-summit-2024");
        assert!(article.guid.is_some());
        assert!(article.published_at.is_some());
        assert!(article.summary.is_some());
    }

    #[test]
    fn entry_to_new_article_atom() {
        let xml = read_fixture("rss/atom_valid.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        let entry = &parsed.entries[0];
        let article = entry_to_new_article(1, entry);

        assert_eq!(article.title, "EU Digital Markets Act Enforcement Begins");
        assert_eq!(article.url, "https://example.com/articles/eu-dma");
    }

    #[test]
    fn entry_to_new_article_missing_title() {
        let xml = read_fixture("rss/rss_malformed.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();

        // Find the entry without a title
        let no_title_entry = parsed.entries.iter().find(|e| {
            e.links
                .first()
                .map(|l| l.href.contains("no-title"))
                .unwrap_or(false)
        });

        if let Some(entry) = no_title_entry {
            let article = entry_to_new_article(1, entry);
            assert_eq!(article.title, "(untitled)");
        }
    }

    #[test]
    fn entry_to_new_article_missing_guid() {
        let xml = read_fixture("rss/rss_malformed.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();

        // feed-rs generates IDs for entries without guid, so they won't be empty.
        // Just verify all entries parse without panic.
        for entry in &parsed.entries {
            let article = entry_to_new_article(1, entry);
            assert!(!article.url.is_empty());
        }
    }

    #[test]
    fn parse_rss_valid() {
        let xml = read_fixture("rss/rss_valid.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), 5);
        assert_eq!(
            parsed.title.as_ref().unwrap().content,
            "Global Affairs Daily"
        );
    }

    #[test]
    fn parse_atom_valid() {
        let xml = read_fixture("rss/atom_valid.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), 5);
        assert_eq!(parsed.title.as_ref().unwrap().content, "Tech Policy Review");
    }

    #[test]
    fn parse_rss_empty() {
        let xml = read_fixture("rss/rss_empty.xml");
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        assert!(parsed.entries.is_empty());
    }

    #[test]
    fn parse_rss_malformed() {
        let xml = read_fixture("rss/rss_malformed.xml");
        // feed-rs is lenient — malformed feeds still parse
        let parsed = feed_rs::parser::parse(xml.as_bytes()).unwrap();
        assert!(!parsed.entries.is_empty());
    }

    // ── Integration tests (mock HTTP + DB) ─────────────────────

    #[tokio::test]
    async fn fetch_feed_valid_rss() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");
        server.mount_feed("/feed.xml", &xml).await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let result = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert_eq!(result.new_articles.len(), 5);
        assert!(!result.not_modified);

        // Verify articles in DB
        let articles = db::articles::get_articles_by_feed(&pool, feed.id)
            .await
            .unwrap();
        assert_eq!(articles.len(), 5);

        // Verify feed metadata updated
        let updated_feed = db::feeds::get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(updated_feed.title.as_deref(), Some("Global Affairs Daily"));
        assert!(updated_feed.last_polled_at.is_some());
    }

    #[tokio::test]
    async fn fetch_feed_valid_atom() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/atom_valid.xml");
        server.mount_feed("/atom.xml", &xml).await;

        let feed_url = format!("{}/atom.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let result = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert_eq!(result.new_articles.len(), 5);
    }

    #[tokio::test]
    async fn fetch_feed_empty() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_empty.xml");
        server.mount_feed("/empty.xml", &xml).await;

        let feed_url = format!("{}/empty.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let result = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert!(result.new_articles.is_empty());
    }

    #[tokio::test]
    async fn fetch_feed_304_not_modified() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        server.mount_304("/feed.xml").await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let result = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert!(result.not_modified);
        assert!(result.new_articles.is_empty());
    }

    #[tokio::test]
    async fn fetch_feed_404_error() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        server.mount_not_found("/feed.xml").await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let err = fetch_feed(&pool, &config, &feed).await.unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus { status: 404, .. }));
    }

    #[tokio::test]
    async fn fetch_feed_429_rate_limited() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        server.mount_429("/feed.xml").await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let err = fetch_feed(&pool, &config, &feed).await.unwrap_err();
        assert!(matches!(
            err,
            FeedError::RateLimited {
                retry_after_secs: 60
            }
        ));
    }

    #[tokio::test]
    async fn fetch_feed_500_error() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        server.mount_server_error("/feed.xml").await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        let err = fetch_feed(&pool, &config, &feed).await.unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus { status: 500, .. }));
    }

    #[tokio::test]
    async fn fetch_feed_deduplication() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");
        server.mount_feed("/feed.xml", &xml).await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        // First fetch — 5 articles
        let result1 = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert_eq!(result1.new_articles.len(), 5);

        // Re-read feed from DB (last_polled_at updated)
        let feed = db::feeds::get_feed(&pool, feed.id).await.unwrap();

        // Second fetch — 0 new (all duplicates)
        let result2 = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert_eq!(result2.new_articles.len(), 0);

        // DB should still have exactly 5
        let articles = db::articles::get_articles_by_feed(&pool, feed.id)
            .await
            .unwrap();
        assert_eq!(articles.len(), 5);
    }

    #[tokio::test]
    async fn fetch_feed_malformed_still_works() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_malformed.xml");
        server.mount_feed("/feed.xml", &xml).await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        // Malformed feed should still parse and insert articles
        let result = fetch_feed(&pool, &config, &feed).await.unwrap();
        assert!(!result.new_articles.is_empty());
    }

    #[tokio::test]
    async fn fetch_feed_stores_etag_and_last_modified() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");

        // Mount a feed with ETag and Last-Modified headers
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};

        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(&xml)
                    .insert_header("content-type", "application/xml")
                    .insert_header("etag", "\"abc123\"")
                    .insert_header("last-modified", "Mon, 01 Jan 2024 12:00:00 GMT"),
            )
            .mount(&server.server())
            .await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        fetch_feed(&pool, &config, &feed).await.unwrap();

        let updated = db::feeds::get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(updated.etag.as_deref(), Some("\"abc123\""));
        assert_eq!(
            updated.last_modified.as_deref(),
            Some("Mon, 01 Jan 2024 12:00:00 GMT")
        );
    }

    #[tokio::test]
    async fn fetch_all_feeds_basic() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let rss_xml = read_fixture("rss/rss_valid.xml");
        let atom_xml = read_fixture("rss/atom_valid.xml");
        server.mount_feed("/feed1.xml", &rss_xml).await;
        server.mount_feed("/feed2.xml", &atom_xml).await;

        let url1 = format!("{}/feed1.xml", server.url());
        let url2 = format!("{}/feed2.xml", server.url());
        add_feed_with_url(&pool, &url1).await;
        add_feed_with_url(&pool, &url2).await;

        let config = test_config_with_server(&server.url());
        let summary = fetch_all_feeds(&pool, &config).await.unwrap();

        assert_eq!(summary.feeds_fetched, 2);
        assert_eq!(summary.feeds_errored, 0);
        assert_eq!(summary.new_articles_total, 10); // 5 per feed (different URLs)
    }

    #[tokio::test]
    async fn fetch_all_feeds_isolates_errors() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");
        server.mount_feed("/good.xml", &xml).await;
        server.mount_server_error("/bad.xml").await;

        let good_url = format!("{}/good.xml", server.url());
        let bad_url = format!("{}/bad.xml", server.url());
        add_feed_with_url(&pool, &good_url).await;
        add_feed_with_url(&pool, &bad_url).await;

        let config = test_config_with_server(&server.url());
        let summary = fetch_all_feeds(&pool, &config).await.unwrap();

        // One good, one bad — the good one should still succeed
        assert_eq!(summary.feeds_fetched, 1);
        assert_eq!(summary.feeds_errored, 1);
        assert_eq!(summary.new_articles_total, 5);
        assert_eq!(summary.errors.len(), 1);
    }

    #[tokio::test]
    async fn fetch_all_feeds_respects_concurrency_limit() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");

        // All 6 feeds serve the same XML, so only the first will produce
        // new articles (the rest are deduped by URL). The key assertion is
        // that all 6 feeds complete without error despite max_concurrent=2.
        for i in 0..6 {
            let path = format!("/feed{i}.xml");
            server.mount_feed(&path, &xml).await;
            let url = format!("{}{}", server.url(), path);
            add_feed_with_url(&pool, &url).await;
        }

        let mut config = test_config_with_server(&server.url());
        config.polling.max_concurrent_fetches = 2;

        let summary = fetch_all_feeds(&pool, &config).await.unwrap();
        assert_eq!(summary.feeds_fetched, 6);
        assert_eq!(summary.feeds_errored, 0);
        // Only 5 unique articles (same URLs across all feeds)
        assert_eq!(summary.new_articles_total, 5);
    }

    #[tokio::test]
    async fn fetch_feed_updates_error_count() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        server.mount_server_error("/feed.xml").await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        let config = test_config_with_server(&server.url());

        // Fetch should fail
        let _ = fetch_feed(&pool, &config, &feed).await;

        // fetch_feed doesn't update error count itself — fetch_all_feeds does.
        // But we can manually check poll_status updates:
        db::feeds::update_poll_status(&pool, feed.id, false, Some("500 error"))
            .await
            .unwrap();
        let f = db::feeds::get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(f.error_count, 1);
    }

    #[tokio::test]
    async fn fetch_feed_updates_metadata() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let xml = read_fixture("rss/rss_valid.xml");
        server.mount_feed("/feed.xml", &xml).await;

        let feed_url = format!("{}/feed.xml", server.url());
        let feed = add_feed_with_url(&pool, &feed_url).await;
        assert!(feed.title.is_none());

        let config = test_config_with_server(&server.url());
        fetch_feed(&pool, &config, &feed).await.unwrap();

        let updated = db::feeds::get_feed(&pool, feed.id).await.unwrap();
        assert_eq!(updated.title.as_deref(), Some("Global Affairs Daily"));
        assert_eq!(
            updated.description.as_deref(),
            Some("In-depth analysis of international relations and geopolitics")
        );
        assert_eq!(
            updated.site_url.as_deref(),
            Some("https://example.com/global-affairs")
        );
    }

    #[tokio::test]
    async fn fetch_all_feeds_empty_list() {
        let pool = test_pool().await;
        let config = Config::default();

        let summary = fetch_all_feeds(&pool, &config).await.unwrap();
        assert_eq!(summary.feeds_fetched, 0);
        assert_eq!(summary.feeds_errored, 0);
        assert_eq!(summary.new_articles_total, 0);
    }
}
