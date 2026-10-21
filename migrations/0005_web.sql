-- Settings that belong to the instance (the admin password hash) and the browser sessions of
-- the web UI. A session is identified by the blake3 hash of its cookie value, so a copy of
-- the database does not hold anything that can be used to sign in.
-- Never edit this file once it has been applied; add a new migration instead.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE sessions (
    token_hash   BLOB PRIMARY KEY NOT NULL,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
) STRICT;
CREATE INDEX sessions_expires_at ON sessions (expires_at);
