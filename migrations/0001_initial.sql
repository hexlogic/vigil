-- Initial schema. Never edit this file once applied; add a new migration instead.
-- Conventions (docs/decisions.md): UUID v7 ids as TEXT, timestamps as unix
-- milliseconds, nested watch configuration as JSON TEXT, hashes as BLOB.

CREATE TABLE tags (
    id                    TEXT PRIMARY KEY NOT NULL,
    name                  TEXT NOT NULL UNIQUE,
    color                 TEXT NOT NULL DEFAULT '',
    default_interval_secs INTEGER,
    created_at            INTEGER NOT NULL
) STRICT;

CREATE TABLE notification_targets (
    id               TEXT PRIMARY KEY NOT NULL,
    name             TEXT NOT NULL,
    url              TEXT NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    default_template TEXT,
    created_at       INTEGER NOT NULL
) STRICT;

CREATE TABLE watches (
    id                             TEXT PRIMARY KEY NOT NULL,
    name                           TEXT NOT NULL,
    url                            TEXT NOT NULL,
    paused                         INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1)),
    fetch                          TEXT NOT NULL DEFAULT '{}',
    schedule                       TEXT NOT NULL,
    jitter_secs                    INTEGER NOT NULL DEFAULT 0,
    processor                      TEXT NOT NULL DEFAULT 'text'
                                   CHECK (processor IN ('text', 'json', 'feed')),
    pipeline                       TEXT NOT NULL DEFAULT '[]',
    conditions                     TEXT NOT NULL DEFAULT '{}',
    notification_template_override TEXT,
    snapshot_retention             INTEGER NOT NULL DEFAULT 100,
    last_checked_at                INTEGER,
    last_changed_at                INTEGER,
    last_error                     TEXT,
    consecutive_errors             INTEGER NOT NULL DEFAULT 0,
    last_hash                      BLOB,
    etag                           TEXT,
    last_modified                  TEXT,
    created_at                     INTEGER NOT NULL,
    updated_at                     INTEGER NOT NULL
) STRICT;

CREATE TABLE watch_tags (
    watch_id TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    tag_id   TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (watch_id, tag_id)
) STRICT;
CREATE INDEX watch_tags_tag_id ON watch_tags (tag_id);

CREATE TABLE watch_notifications (
    watch_id  TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES notification_targets (id) ON DELETE CASCADE,
    PRIMARY KEY (watch_id, target_id)
) STRICT;
CREATE INDEX watch_notifications_target_id ON watch_notifications (target_id);

CREATE TABLE tag_notifications (
    tag_id    TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES notification_targets (id) ON DELETE CASCADE,
    PRIMARY KEY (tag_id, target_id)
) STRICT;
CREATE INDEX tag_notifications_target_id ON tag_notifications (target_id);

CREATE TABLE snapshots (
    id           TEXT PRIMARY KEY NOT NULL,
    watch_id     TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    taken_at     INTEGER NOT NULL,
    hash         BLOB NOT NULL,
    content      BLOB NOT NULL, -- zstd-compressed processed text
    raw_size     INTEGER NOT NULL,
    http_status  INTEGER,
    content_type TEXT
) STRICT;
CREATE INDEX snapshots_watch_taken_at ON snapshots (watch_id, taken_at DESC);

CREATE TABLE check_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id      TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    at            INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    outcome       TEXT NOT NULL CHECK (outcome IN ('unchanged', 'changed', 'error')),
    error_message TEXT
) STRICT;
CREATE INDEX check_log_watch_at ON check_log (watch_id, at DESC);
