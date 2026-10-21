-- Tokens for the REST API. A token is identified by the blake3 hash of its value, so a copy of
-- the database does not hold anything that can be used to call the API. `prefix` is the first
-- few characters of the token, kept so that a person can tell which token is which.
-- Never edit this file once it has been applied; add a new migration instead.
CREATE TABLE api_tokens (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL,
    token_hash   BLOB NOT NULL,
    prefix       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER
) STRICT;
CREATE UNIQUE INDEX api_tokens_name ON api_tokens (name);
CREATE UNIQUE INDEX api_tokens_hash ON api_tokens (token_hash);
