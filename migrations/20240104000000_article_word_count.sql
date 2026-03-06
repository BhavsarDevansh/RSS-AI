-- Add extracted article word count for content extraction pipeline.

ALTER TABLE articles ADD COLUMN word_count INTEGER;

INSERT INTO _schema_version (version, description)
VALUES (4, 'add word_count column to articles');
