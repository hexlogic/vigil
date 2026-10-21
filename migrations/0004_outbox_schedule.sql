-- When a saved notification is tried again, for those whose quick retries ran out on a
-- failure that may pass. NULL means a delivery is in progress, or was when the process
-- stopped. rounds counts the tries that ran out so far.
-- Never edit this file once it has been applied; add a new migration instead.
ALTER TABLE notification_outbox ADD COLUMN next_attempt_at INTEGER;
ALTER TABLE notification_outbox ADD COLUMN rounds INTEGER NOT NULL DEFAULT 0;
CREATE INDEX notification_outbox_due ON notification_outbox (next_attempt_at);
