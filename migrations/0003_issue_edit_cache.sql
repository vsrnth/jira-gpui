CREATE TABLE IF NOT EXISTS issue_edit_users (
    site_id TEXT NOT NULL,
    locator_kind TEXT NOT NULL CHECK (locator_kind IN ('id', 'key')),
    locator_value TEXT NOT NULL,
    fetched_seconds INTEGER NOT NULL,
    fetched_nanos INTEGER NOT NULL CHECK (fetched_nanos BETWEEN 0 AND 999999999),
    snapshot TEXT NOT NULL,
    PRIMARY KEY (site_id, locator_kind, locator_value)
);

CREATE TABLE IF NOT EXISTS issue_edit_transitions (
    site_id TEXT NOT NULL,
    locator_kind TEXT NOT NULL CHECK (locator_kind IN ('id', 'key')),
    locator_value TEXT NOT NULL,
    fetched_seconds INTEGER NOT NULL,
    fetched_nanos INTEGER NOT NULL CHECK (fetched_nanos BETWEEN 0 AND 999999999),
    snapshot TEXT NOT NULL,
    PRIMARY KEY (site_id, locator_kind, locator_value)
);
