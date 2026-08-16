CREATE TABLE user_sets (
    site_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_seconds INTEGER NOT NULL,
    created_nanos INTEGER NOT NULL,
    updated_seconds INTEGER NOT NULL,
    updated_nanos INTEGER NOT NULL,
    PRIMARY KEY (site_id, id),
    UNIQUE (id),
    CHECK (created_nanos BETWEEN 0 AND 999999999),
    CHECK (updated_nanos BETWEEN 0 AND 999999999)
);

CREATE TABLE user_set_members (
    site_id TEXT NOT NULL,
    user_set_id TEXT NOT NULL,
    member_order INTEGER NOT NULL CHECK (member_order >= 0),
    account_id TEXT NOT NULL,
    PRIMARY KEY (site_id, user_set_id, member_order),
    UNIQUE (site_id, user_set_id, account_id),
    FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE
);

CREATE TABLE issues (
    site_id TEXT NOT NULL,
    issue_id TEXT NOT NULL,
    issue_key TEXT NOT NULL,
    summary TEXT NOT NULL,
    assignee_id TEXT,
    updated_seconds INTEGER NOT NULL,
    updated_nanos INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (site_id, issue_id),
    CHECK (updated_nanos BETWEEN 0 AND 999999999)
);

CREATE TABLE issue_membership (
    site_id TEXT NOT NULL,
    user_set_id TEXT NOT NULL,
    issue_id TEXT NOT NULL,
    PRIMARY KEY (site_id, user_set_id, issue_id),
    FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE,
    FOREIGN KEY (site_id, issue_id) REFERENCES issues(site_id, issue_id) ON DELETE CASCADE
);

CREATE TABLE sync_states (
    site_id TEXT NOT NULL,
    user_set_id TEXT NOT NULL,
    last_incremental_started_seconds INTEGER,
    last_incremental_started_nanos INTEGER,
    last_incremental_succeeded_seconds INTEGER,
    last_incremental_succeeded_nanos INTEGER,
    last_full_sync_seconds INTEGER,
    last_full_sync_nanos INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_error_kind INTEGER,
    PRIMARY KEY (site_id, user_set_id),
    FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE,
    CHECK (consecutive_failures >= 0),
    CHECK (last_error_kind IS NULL OR last_error_kind BETWEEN 0 AND 10),
    CHECK (last_incremental_started_nanos IS NULL OR last_incremental_started_nanos BETWEEN 0 AND 999999999),
    CHECK (last_incremental_succeeded_nanos IS NULL OR last_incremental_succeeded_nanos BETWEEN 0 AND 999999999),
    CHECK (last_full_sync_nanos IS NULL OR last_full_sync_nanos BETWEEN 0 AND 999999999)
);

CREATE TABLE update_events (
    event_id TEXT PRIMARY KEY,
    site_id TEXT NOT NULL,
    issue_id TEXT NOT NULL,
    issue_key TEXT NOT NULL,
    kind INTEGER NOT NULL,
    occurred_seconds INTEGER NOT NULL,
    occurred_nanos INTEGER NOT NULL,
    read_state INTEGER NOT NULL DEFAULT 0,
    notification_delivery INTEGER NOT NULL DEFAULT 0,
    snapshot TEXT NOT NULL,
    UNIQUE (event_id, site_id),
    FOREIGN KEY (site_id, issue_id) REFERENCES issues(site_id, issue_id) ON DELETE CASCADE,
    CHECK (kind BETWEEN 0 AND 8),
    CHECK (occurred_nanos BETWEEN 0 AND 999999999),
    CHECK (read_state IN (0, 1)),
    CHECK (notification_delivery BETWEEN 0 AND 3)
);

CREATE TABLE event_user_sets (
    event_id TEXT NOT NULL,
    site_id TEXT NOT NULL,
    user_set_id TEXT NOT NULL,
    PRIMARY KEY (event_id, site_id, user_set_id),
    FOREIGN KEY (event_id, site_id) REFERENCES update_events(event_id, site_id) ON DELETE CASCADE,
    FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE
);

CREATE INDEX issues_site_updated_idx
    ON issues(site_id, updated_seconds DESC, updated_nanos DESC, issue_key ASC);
CREATE INDEX issues_search_idx ON issues(site_id, issue_key, summary, assignee_id);
CREATE INDEX issue_membership_lookup_idx
    ON issue_membership(site_id, user_set_id, issue_id);
CREATE INDEX update_events_feed_idx
    ON update_events(site_id, occurred_seconds DESC, occurred_nanos DESC, event_id ASC);
CREATE INDEX update_events_unread_idx
    ON update_events(site_id, read_state, occurred_seconds DESC, occurred_nanos DESC);
CREATE INDEX event_user_sets_lookup_idx ON event_user_sets(site_id, user_set_id, event_id);
