-- Domain tables for RSS-AI knowledge base.

-- ── feeds ──────────────────────────────────────────────────────────

CREATE TABLE feeds (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    url          TEXT    NOT NULL UNIQUE,
    title        TEXT,
    description  TEXT,
    site_url     TEXT,
    poll_interval_minutes INTEGER NOT NULL DEFAULT 30,
    last_polled_at TEXT,
    last_error     TEXT,
    error_count    INTEGER NOT NULL DEFAULT 0,
    active       INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- ── articles ───────────────────────────────────────────────────────

CREATE TABLE articles (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_id         INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    guid            TEXT,
    url             TEXT    NOT NULL,
    title           TEXT    NOT NULL,
    author          TEXT,
    published_at    TEXT,
    summary         TEXT,
    content         TEXT,
    content_hash    TEXT,
    content_extracted INTEGER NOT NULL DEFAULT 0,
    embedding_generated INTEGER NOT NULL DEFAULT 0,
    tags_generated  INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(feed_id, guid),
    UNIQUE(url)
);

CREATE INDEX idx_articles_feed_id      ON articles(feed_id);
CREATE INDEX idx_articles_url          ON articles(url);
CREATE INDEX idx_articles_published_at ON articles(published_at);
CREATE INDEX idx_articles_content_hash ON articles(content_hash);

-- ── tags ───────────────────────────────────────────────────────────

CREATE TABLE tags (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT    NOT NULL UNIQUE,
    article_count INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- ── article_tags (join table) ──────────────────────────────────────

CREATE TABLE article_tags (
    article_id  INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    confidence  REAL    NOT NULL DEFAULT 1.0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (article_id, tag_id)
);

CREATE INDEX idx_article_tags_tag_id ON article_tags(tag_id);

-- ── article_links (cross-references between articles) ──────────────

CREATE TABLE article_links (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    source_article_id  INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    target_article_id  INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    relationship       TEXT    NOT NULL DEFAULT 'related',
    strength           REAL    NOT NULL DEFAULT 1.0,
    created_at         TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_article_id, target_article_id)
);

CREATE INDEX idx_article_links_source ON article_links(source_article_id);
CREATE INDEX idx_article_links_target ON article_links(target_article_id);

-- ── triggers: maintain tags.article_count ──────────────────────────

CREATE TRIGGER trg_article_tags_insert
AFTER INSERT ON article_tags
BEGIN
    UPDATE tags SET article_count = article_count + 1
    WHERE id = NEW.tag_id;
END;

CREATE TRIGGER trg_article_tags_delete
AFTER DELETE ON article_tags
BEGIN
    UPDATE tags SET article_count = article_count - 1
    WHERE id = OLD.tag_id;
END;

-- ── update schema version ──────────────────────────────────────────

INSERT INTO _schema_version (version, description)
VALUES (2, 'domain tables: feeds, articles, tags, article_tags, article_links');
