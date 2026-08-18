-- The field_changed update kind extends the persisted kind range without
-- changing the meaning or identity of any pre-existing event.
CREATE TABLE update_events_v4 (
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
    CHECK (kind BETWEEN 0 AND 10),
    CHECK (occurred_nanos BETWEEN 0 AND 999999999),
    CHECK (read_state IN (0, 1)),
    CHECK (notification_delivery BETWEEN 0 AND 3)
);

INSERT INTO update_events_v4
    SELECT event_id, site_id, issue_id, issue_key, kind, occurred_seconds,
           occurred_nanos, read_state, notification_delivery, snapshot
    FROM update_events;

CREATE TABLE event_user_sets_v4 (
    event_id TEXT NOT NULL,
    site_id TEXT NOT NULL,
    user_set_id TEXT NOT NULL,
    PRIMARY KEY (event_id, site_id, user_set_id),
    FOREIGN KEY (event_id, site_id) REFERENCES update_events_v4(event_id, site_id) ON DELETE CASCADE,
    FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE
);

INSERT INTO event_user_sets_v4
    SELECT event_id, site_id, user_set_id
    FROM event_user_sets;

DROP TABLE event_user_sets;
DROP TABLE update_events;
ALTER TABLE update_events_v4 RENAME TO update_events;
ALTER TABLE event_user_sets_v4 RENAME TO event_user_sets;

CREATE INDEX update_events_feed_idx
    ON update_events(site_id, occurred_seconds DESC, occurred_nanos DESC, event_id ASC);
CREATE INDEX update_events_unread_idx
    ON update_events(site_id, read_state, occurred_seconds DESC, occurred_nanos DESC);
CREATE INDEX event_user_sets_lookup_idx
    ON event_user_sets(site_id, user_set_id, event_id);
