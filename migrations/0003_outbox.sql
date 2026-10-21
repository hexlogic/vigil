-- Notifications that are saved before they are delivered, and removed once they are done, so a
-- restart or a crash while one is waiting to retry does not lose it.
-- Never edit this file once it has been applied; add a new migration instead.

-- payload is the event the message is built from (JSON, zstd). snapshot_id has no foreign key:
-- snapshot retention may prune the snapshot before the notification is delivered.
CREATE TABLE notification_outbox (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id    TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    target_id   TEXT NOT NULL REFERENCES notification_targets (id) ON DELETE CASCADE,
    snapshot_id TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    payload     BLOB NOT NULL
) STRICT;
