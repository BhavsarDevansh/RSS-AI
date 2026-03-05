-- Baseline migration: establishes migration infrastructure.
-- Future migrations (from issue #2 onward) will create domain tables.

CREATE TABLE _schema_version (
    version INTEGER NOT NULL PRIMARY KEY,
    description TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO _schema_version (version, description)
VALUES (1, 'baseline migration');
