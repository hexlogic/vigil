-- Notification delivery results and unique target names.
-- Never edit this file once it has been applied; add a new migration instead.

-- Nothing wrote to notification_targets before this migration, so the index cannot fail on
-- existing data.
CREATE UNIQUE INDEX notification_targets_name ON notification_targets (name);

-- One row per target per notification, written after the check that caused it has been
-- recorded (delivery retries can take minutes). watch_id is NULL for test notifications.
-- snapshot_id has no foreign key: snapshot retention prunes old snapshots.
CREATE TABLE delivery_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id      TEXT REFERENCES watches (id) ON DELETE CASCADE,
    target_id     TEXT NOT NULL REFERENCES notification_targets (id) ON DELETE CASCADE,
    snapshot_id   TEXT,
    at            INTEGER NOT NULL,
    attempts      INTEGER NOT NULL,
    outcome       TEXT NOT NULL CHECK (outcome IN ('sent', 'failed')),
    error_message TEXT
) STRICT;
CREATE INDEX delivery_log_watch_id ON delivery_log (watch_id, id DESC);
CREATE INDEX delivery_log_target_id ON delivery_log (target_id, id DESC);
