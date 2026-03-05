/// Domain model structs for database rows and input types.

// ── Row types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub poll_interval_minutes: i64,
    pub last_polled_at: Option<String>,
    pub last_error: Option<String>,
    pub error_count: i64,
    pub active: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Article {
    pub id: i64,
    pub feed_id: i64,
    pub guid: Option<String>,
    pub url: String,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub content_hash: Option<String>,
    pub content_extracted: i64,
    pub embedding_generated: i64,
    pub tags_generated: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub article_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArticleTag {
    pub article_id: i64,
    pub tag_id: i64,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArticleLink {
    pub id: i64,
    pub source_article_id: i64,
    pub target_article_id: i64,
    pub relationship: String,
    pub strength: f64,
    pub created_at: String,
}

// ── Input / helper types ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NewArticle {
    pub feed_id: i64,
    pub guid: Option<String>,
    pub url: String,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TagWithConfidence {
    pub name: String,
    pub confidence: f64,
}

/// A node and its edges in the article link graph.
#[derive(Debug, Clone)]
pub struct LinkGraphNode {
    pub article_id: i64,
    pub linked_article_ids: Vec<i64>,
}
