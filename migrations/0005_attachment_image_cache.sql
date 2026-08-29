CREATE TABLE IF NOT EXISTS issue_media_cache (
    site_id TEXT NOT NULL,
    issue_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    bytes BLOB NOT NULL,
    stored_order INTEGER NOT NULL,
    PRIMARY KEY (site_id, issue_id, attachment_id),
    FOREIGN KEY (site_id, issue_id) REFERENCES issues(site_id, issue_id) ON DELETE CASCADE,
    CHECK (length(attachment_id) BETWEEN 1 AND 255),
    CHECK (length(mime_type) BETWEEN 1 AND 255),
    CHECK (length(bytes) BETWEEN 1 AND 8388608)
);

CREATE INDEX IF NOT EXISTS issue_media_cache_eviction_idx
    ON issue_media_cache(stored_order ASC, site_id ASC, issue_id ASC, attachment_id ASC);
