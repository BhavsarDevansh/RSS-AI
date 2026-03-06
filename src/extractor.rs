/// Article content extraction and extraction pipeline integration.
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use reqwest::header;
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};
use url::Url;

use crate::config::Config;
use crate::db;

#[derive(Debug, thiserror::Error)]
pub enum ExtractorError {
    #[error("invalid article URL '{0}'")]
    InvalidUrl(String),

    #[error("robots.txt disallowed crawling for {url}")]
    RobotsDisallowed { url: String },

    #[error("HTTP error fetching {url}: {status}")]
    HttpStatus { url: String, status: u16 },

    #[error("HTTP request failed for {url}: {source}")]
    Request { url: String, source: reqwest::Error },

    #[error("database error: {0}")]
    Db(#[from] db::DbError),
}

#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub text: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub word_count: usize,
    pub content_hash: String,
    pub potentially_incomplete: bool,
}

#[derive(Debug, Clone, Default)]
struct RobotsRules {
    allow_paths: Vec<String>,
    disallow_paths: Vec<String>,
}

impl RobotsRules {
    fn allows(&self, path: &str) -> bool {
        let allow_len = self
            .allow_paths
            .iter()
            .filter(|p| !p.is_empty() && path.starts_with(p.as_str()))
            .map(std::string::String::len)
            .max()
            .unwrap_or(0);

        let disallow_len = self
            .disallow_paths
            .iter()
            .filter(|p| !p.is_empty() && path.starts_with(p.as_str()))
            .map(std::string::String::len)
            .max()
            .unwrap_or(0);

        if disallow_len == 0 {
            return true;
        }

        allow_len >= disallow_len
    }
}

static DOMAIN_LAST_REQUEST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
static ROBOTS_CACHE: OnceLock<Mutex<HashMap<String, RobotsRules>>> = OnceLock::new();

/// Fetch and extract readable content from a single article URL.
pub async fn extract_content(
    url: &str,
    config: &Config,
) -> Result<ExtractedContent, ExtractorError> {
    let parsed_url = Url::parse(url).map_err(|_| ExtractorError::InvalidUrl(url.to_string()))?;
    let domain = domain_key(&parsed_url)?;
    let client = build_client(config)?;

    let allowed = is_allowed_by_robots(&client, &parsed_url, config).await;
    if !allowed {
        return Err(ExtractorError::RobotsDisallowed {
            url: url.to_string(),
        });
    }

    throttle_domain(&domain).await;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ExtractorError::Request {
            url: url.to_string(),
            source: e,
        })?;

    if !response.status().is_success() {
        return Err(ExtractorError::HttpStatus {
            url: url.to_string(),
            status: response.status().as_u16(),
        });
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();

    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| ExtractorError::Request {
            url: url.to_string(),
            source: e,
        })?;

    let mut body = String::from_utf8_lossy(&body_bytes).to_string();
    body = truncate_utf8(&body, config.extraction.max_article_size_bytes as usize);

    let is_html = content_type.contains("text/html")
        || content_type.contains("application/xhtml+xml")
        || body.contains("<html")
        || body.contains("<article")
        || body.contains("<body");

    let (mut text, title, author, published_at) = if is_html {
        extract_from_html(&body)
    } else {
        (normalize_whitespace(&body), None, None, None)
    };

    text = normalize_whitespace(&text);
    text = truncate_utf8(&text, config.extraction.max_article_size_bytes as usize);

    let word_count = count_words(&text);
    let content_hash = compute_content_hash(&text);
    let potentially_incomplete = is_potentially_incomplete(&body, &text);

    Ok(ExtractedContent {
        text,
        title,
        author,
        published_at,
        word_count,
        content_hash,
        potentially_incomplete,
    })
}

/// Process all articles where content has not yet been extracted.
pub async fn process_pending_articles(
    pool: &SqlitePool,
    config: &Config,
) -> Result<usize, ExtractorError> {
    let pending: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, url FROM articles WHERE content_extracted = 0 ORDER BY id")
            .fetch_all(pool)
            .await
            .map_err(db::DbError::from)?;

    let mut processed = 0usize;

    for (article_id, url) in pending {
        match extract_content(&url, config).await {
            Ok(extracted) => {
                let published = extracted.published_at.map(|d| d.to_rfc3339());
                let update = db::articles::ExtractedArticleUpdate {
                    content: &extracted.text,
                    content_hash: &extracted.content_hash,
                    word_count: extracted.word_count as i64,
                    title: extracted.title.as_deref(),
                    author: extracted.author.as_deref(),
                    published_at: published.as_deref(),
                };

                db::articles::update_article_content_with_metadata(pool, article_id, &update)
                    .await?;

                processed += 1;
            }
            Err(err) => {
                tracing::warn!(article_id, url = %url, error = %err, "content extraction failed");
            }
        }
    }

    Ok(processed)
}

fn build_client(config: &Config) -> Result<reqwest::Client, ExtractorError> {
    reqwest::Client::builder()
        .user_agent(&config.extraction.user_agent)
        .timeout(Duration::from_secs(
            config.extraction.request_timeout_seconds as u64,
        ))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| ExtractorError::Request {
            url: String::new(),
            source: e,
        })
}

fn domain_key(url: &Url) -> Result<String, ExtractorError> {
    let host = url
        .host_str()
        .ok_or_else(|| ExtractorError::InvalidUrl(url.as_str().to_string()))?;

    let key = if let Some(port) = url.port() {
        format!("{}://{}:{}", url.scheme(), host, port)
    } else {
        format!("{}://{}", url.scheme(), host)
    };

    Ok(key)
}

async fn throttle_domain(domain: &str) {
    let map = DOMAIN_LAST_REQUEST.get_or_init(|| Mutex::new(HashMap::new()));

    loop {
        let now = Instant::now();
        let wait = {
            let mut lock = map.lock().await;
            if let Some(last) = lock.get(domain).copied() {
                let next_allowed = last + Duration::from_secs(1);
                if next_allowed > now {
                    next_allowed - now
                } else {
                    lock.insert(domain.to_string(), now);
                    Duration::ZERO
                }
            } else {
                lock.insert(domain.to_string(), now);
                Duration::ZERO
            }
        };

        if wait.is_zero() {
            break;
        }
        sleep(wait).await;
    }
}

async fn is_allowed_by_robots(client: &reqwest::Client, url: &Url, config: &Config) -> bool {
    let domain = match domain_key(url) {
        Ok(domain) => domain,
        Err(_) => return true,
    };

    let cache = ROBOTS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(rules) = cache.lock().await.get(&domain).cloned() {
        return rules.allows(url.path());
    }

    let robots_url = format!("{domain}/robots.txt");
    throttle_domain(&domain).await;

    let response = match client.get(&robots_url).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::debug!(robots_url = %robots_url, error = %err, "robots fetch failed, allowing request");
            return true;
        }
    };

    if !response.status().is_success() {
        return true;
    }

    let body = match response.text().await {
        Ok(text) => text,
        Err(err) => {
            tracing::debug!(robots_url = %robots_url, error = %err, "robots read failed, allowing request");
            return true;
        }
    };

    let rules = parse_robots_rules(&body, &config.extraction.user_agent);
    let allowed = rules.allows(url.path());

    cache.lock().await.insert(domain, rules);
    allowed
}

fn parse_robots_rules(robots_txt: &str, user_agent: &str) -> RobotsRules {
    let mut wildcard = RobotsRules::default();
    let mut specific = RobotsRules::default();

    let ua_token = user_agent
        .split('/')
        .next()
        .unwrap_or(user_agent)
        .trim()
        .to_lowercase();

    let mut current_agents: Vec<String> = Vec::new();

    for raw_line in robots_txt.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let (key, value) = match line.split_once(':') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().to_string()),
            None => continue,
        };

        if key == "user-agent" {
            let value = value.to_lowercase();
            current_agents.push(value);
            continue;
        }

        if key != "allow" && key != "disallow" {
            continue;
        }

        if current_agents.is_empty() {
            continue;
        }

        let applies_specific = current_agents
            .iter()
            .any(|ua| ua == &ua_token || ua == &format!("{}*", ua_token));
        let applies_wildcard = current_agents.iter().any(|ua| ua == "*");

        if !applies_specific && !applies_wildcard {
            continue;
        }

        let normalized_value = value.trim().to_string();

        if applies_specific {
            if key == "allow" {
                specific.allow_paths.push(normalized_value.clone());
            } else {
                specific.disallow_paths.push(normalized_value.clone());
            }
        }

        if applies_wildcard {
            if key == "allow" {
                wildcard.allow_paths.push(normalized_value);
            } else {
                wildcard.disallow_paths.push(normalized_value);
            }
        }
    }

    if !specific.allow_paths.is_empty() || !specific.disallow_paths.is_empty() {
        specific
    } else {
        wildcard
    }
}

fn extract_from_html(
    html: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<DateTime<Utc>>,
) {
    let document = Html::parse_document(html);

    let title = extract_title(&document);
    let author = extract_author(&document);
    let published_at = extract_published_at(&document);

    let text = extract_readable_text(&document, html);

    (text, title, author, published_at)
}

fn extract_readable_text(document: &Html, full_html: &str) -> String {
    if let Some(root) = select_primary_root(document) {
        let text = collect_readable_blocks(root);
        if !text.is_empty() {
            return text;
        }

        let fallback = html2text::from_read(root.html().as_bytes(), 100).unwrap_or_default();
        let normalized = normalize_whitespace(&fallback);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    let fallback = html2text::from_read(full_html.as_bytes(), 100).unwrap_or_default();
    normalize_whitespace(&fallback)
}

fn select_primary_root<'a>(document: &'a Html) -> Option<ElementRef<'a>> {
    let candidates = [
        ("article", 3000usize),
        ("main", 2000usize),
        ("[role='main']", 1500usize),
        (".post", 1000usize),
        (".article", 1000usize),
        (".entry-content", 900usize),
        (".content", 600usize),
        ("body", 0usize),
    ];

    let mut best: Option<(usize, ElementRef<'_>)> = None;

    for (selector, bonus) in candidates {
        let Ok(sel) = Selector::parse(selector) else {
            continue;
        };

        for element in document.select(&sel) {
            if is_boilerplate(&element) {
                continue;
            }

            let text_len = element.text().collect::<Vec<_>>().join(" ").trim().len();
            let score = text_len + bonus;

            match best {
                Some((best_score, _)) if best_score >= score => {}
                _ => best = Some((score, element)),
            }
        }
    }

    best.map(|(_, element)| element)
}

fn collect_readable_blocks(root: ElementRef<'_>) -> String {
    let selector = Selector::parse("h1, h2, h3, h4, h5, h6, p, li, pre, blockquote")
        .expect("valid block selector");
    let mut blocks = Vec::new();

    for element in root.select(&selector) {
        if is_boilerplate(&element) {
            continue;
        }

        let tag = element.value().name();
        let text = normalize_whitespace(&element.text().collect::<Vec<_>>().join(" "));
        if text.is_empty() {
            continue;
        }

        let formatted = match tag {
            "h1" => format!("# {text}"),
            "h2" | "h3" | "h4" | "h5" | "h6" => format!("## {text}"),
            "li" => format!("- {text}"),
            _ => text,
        };

        if blocks.last() != Some(&formatted) {
            blocks.push(formatted);
        }
    }

    blocks.join("\n\n")
}

fn is_boilerplate(element: &ElementRef<'_>) -> bool {
    for node in element.ancestors() {
        let Some(ancestor) = ElementRef::wrap(node) else {
            continue;
        };

        let tag = ancestor.value().name();
        if matches!(
            tag,
            "nav"
                | "header"
                | "footer"
                | "aside"
                | "script"
                | "style"
                | "noscript"
                | "form"
                | "svg"
        ) {
            return true;
        }

        if has_boilerplate_keyword(ancestor.value().attr("class"))
            || has_boilerplate_keyword(ancestor.value().attr("id"))
        {
            return true;
        }
    }

    false
}

fn has_boilerplate_keyword(value: Option<&str>) -> bool {
    let Some(raw) = value else {
        return false;
    };

    let value = raw.to_lowercase();
    let keywords = [
        "nav",
        "menu",
        "header",
        "footer",
        "sidebar",
        "comment",
        "advert",
        "ad-",
        "promo",
        "subscribe",
        "related",
        "social",
        "share",
    ];

    keywords.iter().any(|k| value.contains(k))
}

fn extract_title(document: &Html) -> Option<String> {
    for selector in [
        "meta[property='og:title']",
        "meta[name='twitter:title']",
        "meta[name='title']",
    ] {
        if let Some(content) = meta_content(document, selector) {
            return Some(content);
        }
    }

    for selector in [
        "article h1",
        "article h2",
        "main article h1",
        "main article h2",
        "title",
        "main h1",
        "h1",
    ] {
        let Ok(sel) = Selector::parse(selector) else {
            continue;
        };

        if let Some(element) = document.select(&sel).next() {
            let title = normalize_whitespace(&element.text().collect::<Vec<_>>().join(" "));
            if !title.is_empty() {
                return Some(title);
            }
        }
    }

    None
}

fn extract_author(document: &Html) -> Option<String> {
    for selector in [
        "meta[name='author']",
        "meta[property='article:author']",
        "meta[name='parsely-author']",
    ] {
        if let Some(content) = meta_content(document, selector) {
            return Some(strip_by_prefix(&content));
        }
    }

    for selector in [
        "[rel='author']",
        ".author",
        ".byline",
        "[itemprop='author']",
    ] {
        let Ok(sel) = Selector::parse(selector) else {
            continue;
        };

        if let Some(element) = document.select(&sel).next() {
            let text = normalize_whitespace(&element.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() {
                return Some(strip_by_prefix(&text));
            }
        }
    }

    None
}

fn extract_published_at(document: &Html) -> Option<DateTime<Utc>> {
    for selector in [
        "meta[property='article:published_time']",
        "meta[property='og:published_time']",
        "meta[name='pubdate']",
        "meta[name='date']",
        "meta[name='publish-date']",
    ] {
        if let Some(content) = meta_content(document, selector)
            && let Some(dt) = parse_datetime(&content)
        {
            return Some(dt);
        }
    }

    let time_selector = Selector::parse("time").expect("valid time selector");
    for element in document.select(&time_selector) {
        if let Some(datetime) = element.value().attr("datetime")
            && let Some(dt) = parse_datetime(datetime)
        {
            return Some(dt);
        }

        let text = normalize_whitespace(&element.text().collect::<Vec<_>>().join(" "));
        if let Some(dt) = parse_datetime(&text) {
            return Some(dt);
        }
    }

    None
}

fn meta_content(document: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    let element = document.select(&sel).next()?;
    let content = element.value().attr("content")?;
    let cleaned = normalize_whitespace(content);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn parse_datetime(input: &str) -> Option<DateTime<Utc>> {
    let candidate = input.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(candidate) {
        return Some(dt.with_timezone(&Utc));
    }

    if let Ok(dt) = DateTime::parse_from_rfc2822(candidate) {
        return Some(dt.with_timezone(&Utc));
    }

    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(candidate, format) {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }

    for format in ["%Y-%m-%d", "%B %d, %Y", "%b %d, %Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(candidate, format)
            && let Some(dt) = date.and_hms_opt(0, 0, 0)
        {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }

    None
}

fn strip_by_prefix(author: &str) -> String {
    author
        .trim()
        .trim_start_matches("By ")
        .trim_start_matches("by ")
        .trim()
        .to_string()
}

fn normalize_whitespace(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut previous_was_space = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            if !previous_was_space {
                normalized.push(' ');
                previous_was_space = true;
            }
        } else {
            normalized.push(ch);
            previous_was_space = false;
        }
    }

    normalized.trim().to_string()
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }

    let mut last_boundary = 0usize;
    for (idx, _) in input.char_indices() {
        if idx > max_bytes {
            break;
        }
        last_boundary = idx;
    }

    if last_boundary == 0 {
        return String::new();
    }

    input[..last_boundary].to_string()
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn compute_content_hash(text: &str) -> String {
    let normalized = normalize_whitespace(text);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_potentially_incomplete(raw_html: &str, extracted_text: &str) -> bool {
    let lower = raw_html.to_lowercase();
    let paywall_markers = [
        "subscriber only",
        "premium content",
        "subscribe now",
        "subscribe to continue",
        "sign in to continue",
        "paywall",
    ];

    let has_paywall_marker = paywall_markers.iter().any(|m| lower.contains(m));
    let short_content = count_words(extracted_text) < 250;

    has_paywall_marker && short_content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::NewArticle;
    use crate::db::{articles, feeds};
    use crate::test_utils::db::test_pool;
    use crate::test_utils::fixtures::read_fixture;
    use crate::test_utils::mock_http::MockFeedServer;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn test_config() -> Config {
        let mut cfg = Config::default();
        cfg.service.data_dir = "/tmp/rss-ai-test".to_string();
        cfg.extraction.request_timeout_seconds = 1;
        cfg
    }

    #[test]
    fn extracts_content_from_news_blog_and_technical_layouts() {
        let news = read_fixture("html/news_article.html");
        let blog = read_fixture("html/blog_post.html");
        let technical = read_fixture("html/technical_article.html");

        let (news_text, news_title, news_author, news_published) = extract_from_html(&news);
        let (blog_text, blog_title, _, _) = extract_from_html(&blog);
        let (technical_text, technical_title, _, _) = extract_from_html(&technical);

        assert!(news_text.contains("Alliance members convened in Brussels"));
        assert!(!news_text.contains("Advertisement"));
        assert!(!news_text.contains("Comments"));
        assert_eq!(
            news_title.as_deref(),
            Some("NATO Summit Addresses Eastern European Security Concerns")
        );
        assert_eq!(news_author.as_deref(), Some("Jane Doe"));
        assert_eq!(
            news_published.map(|dt| dt.to_rfc3339()),
            Some("2024-01-01T10:00:00+00:00".to_string())
        );

        assert!(blog_text.contains("Vector search has become a cornerstone"));
        assert!(!blog_text.contains("Related Posts"));
        assert_eq!(
            blog_title.as_deref(),
            Some("Understanding Modern Vector Search")
        );

        assert!(technical_text.contains("SQLite is remarkably fast out of the box"));
        assert!(technical_text.contains("## Essential PRAGMAs"));
        assert_eq!(
            technical_title.as_deref(),
            Some("SQLite Performance Tuning for Embedded Applications")
        );
    }

    #[test]
    fn extraction_output_strips_html_tags() {
        let html = read_fixture("html/minimal.html");
        let (text, _, _, _) = extract_from_html(&html);

        assert!(text.contains("This is a minimal HTML document"));
        assert!(!text.contains('<'));
        assert!(!text.contains('>'));
    }

    #[test]
    fn truncates_content_by_max_bytes() {
        let text = "hello wonderful world";
        let truncated = truncate_utf8(text, 8);
        assert_eq!(truncated, "hello wo");
    }

    #[test]
    fn content_hash_is_stable_for_equivalent_whitespace() {
        let first = compute_content_hash("Hello   world\nfrom   RSS-AI");
        let second = compute_content_hash("Hello world from RSS-AI");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn extract_content_handles_404() {
        let server = MockFeedServer::start().await;
        server.mount_not_found("/missing").await;

        let cfg = test_config();
        let url = format!("{}/missing", server.url());
        let err = extract_content(&url, &cfg).await.unwrap_err();

        assert!(matches!(
            err,
            ExtractorError::HttpStatus { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn extract_content_handles_timeout() {
        let server = MockFeedServer::start().await;

        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(2))
                    .set_body_string(
                        "<html><body><article><p>slow page</p></article></body></html>",
                    )
                    .insert_header("content-type", "text/html"),
            )
            .mount(server.server())
            .await;

        let cfg = test_config();
        let url = format!("{}/slow", server.url());
        let err = extract_content(&url, &cfg).await.unwrap_err();

        assert!(matches!(err, ExtractorError::Request { .. }));
    }

    #[tokio::test]
    async fn extract_content_handles_non_html() {
        let server = MockFeedServer::start().await;

        Mock::given(method("GET"))
            .and(path("/doc.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("simple text payload from non html")
                    .insert_header("content-type", "application/pdf"),
            )
            .mount(server.server())
            .await;

        let cfg = test_config();
        let url = format!("{}/doc.pdf", server.url());
        let extracted = extract_content(&url, &cfg).await.unwrap();

        assert!(extracted.text.contains("simple text payload"));
        assert!(extracted.word_count > 0);
    }

    #[tokio::test]
    async fn extract_content_handles_empty_pages() {
        let server = MockFeedServer::start().await;
        server.mount_article("/empty", "").await;

        let cfg = test_config();
        let url = format!("{}/empty", server.url());
        let extracted = extract_content(&url, &cfg).await.unwrap();

        assert!(extracted.text.is_empty());
        assert_eq!(extracted.word_count, 0);
    }

    #[tokio::test]
    async fn extract_content_marks_paywalled_pages_as_potentially_incomplete() {
        let server = MockFeedServer::start().await;
        let html = read_fixture("html/paywall_article.html");
        server.mount_article("/paywall", &html).await;

        let cfg = test_config();
        let url = format!("{}/paywall", server.url());
        let extracted = extract_content(&url, &cfg).await.unwrap();

        assert!(extracted.potentially_incomplete);
        assert!(
            extracted
                .text
                .contains("global economy faces a complex landscape")
        );
    }

    #[tokio::test]
    async fn extract_content_truncates_output_by_config_limit() {
        let server = MockFeedServer::start().await;
        server
            .mount_article(
                "/long",
                "<html><body><article><p>0123456789abcdef0123456789abcdef0123456789abcdef</p></article></body></html>",
            )
            .await;

        let mut cfg = test_config();
        cfg.extraction.max_article_size_bytes = 20;

        let url = format!("{}/long", server.url());
        let extracted = extract_content(&url, &cfg).await.unwrap();

        assert!(extracted.text.len() <= 20);
    }

    #[tokio::test]
    async fn respects_domain_rate_limit() {
        let server = MockFeedServer::start().await;
        server
            .mount_article(
                "/a1",
                "<html><body><article><p>first article</p></article></body></html>",
            )
            .await;
        server
            .mount_article(
                "/a2",
                "<html><body><article><p>second article</p></article></body></html>",
            )
            .await;

        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
            .mount(server.server())
            .await;

        let cfg = test_config();
        let url1 = format!("{}/a1", server.url());
        let url2 = format!("{}/a2", server.url());

        let start = Instant::now();
        extract_content(&url1, &cfg).await.unwrap();
        extract_content(&url2, &cfg).await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(900),
            "expected at least ~1s delay, got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn respects_robots_txt() {
        let server = MockFeedServer::start().await;

        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private"),
            )
            .mount(server.server())
            .await;

        server
            .mount_article(
                "/private/article",
                "<html><body><article><p>hidden</p></article></body></html>",
            )
            .await;

        let cfg = test_config();
        let url = format!("{}/private/article", server.url());
        let err = extract_content(&url, &cfg).await.unwrap_err();

        assert!(matches!(err, ExtractorError::RobotsDisallowed { .. }));
    }

    #[tokio::test]
    async fn process_pending_articles_updates_database() {
        let pool = test_pool().await;
        let server = MockFeedServer::start().await;
        let html = read_fixture("html/news_article.html");
        server.mount_article("/article", &html).await;

        let feed = feeds::add_feed(&pool, "https://example.com/rss", None)
            .await
            .unwrap();
        let url = format!("{}/article", server.url());

        let article = articles::insert_article(
            &pool,
            &NewArticle {
                feed_id: feed.id,
                guid: Some("extract-1".to_string()),
                url,
                title: "Placeholder".to_string(),
                author: None,
                published_at: None,
                summary: None,
                content: None,
                content_hash: None,
            },
        )
        .await
        .unwrap();

        let cfg = test_config();
        let processed = process_pending_articles(&pool, &cfg).await.unwrap();
        assert_eq!(processed, 1);

        let updated = articles::get_article(&pool, article.id).await.unwrap();
        assert_eq!(updated.content_extracted, 1);
        assert!(updated.content.is_some());
        assert!(updated.content_hash.is_some());
        assert!(updated.word_count.unwrap_or_default() > 0);
        assert_eq!(updated.author.as_deref(), Some("Jane Doe"));
    }
}
