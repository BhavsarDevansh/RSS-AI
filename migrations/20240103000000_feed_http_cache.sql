-- Add HTTP conditional request columns to feeds.

ALTER TABLE feeds ADD COLUMN etag TEXT;
ALTER TABLE feeds ADD COLUMN last_modified TEXT;

INSERT INTO _schema_version (version, description)
VALUES (3, 'add etag and last_modified to feeds for conditional requests');
