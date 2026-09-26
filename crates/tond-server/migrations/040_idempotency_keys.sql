-- Idempotency keys (idempotency.rs): the first successful response to a
-- mutation sent with `Idempotency-Key`, replayed for repeats within 24 h.
-- Scoped per caller (actor), method and concrete path. `status` is NULL
-- while the first request is still running. `response` is the body,
-- encrypted with TOND_SECRET_KEY (it can hold a new token or database
-- password). Bounded: at most a few thousand rows, pruned by age.
CREATE TABLE idempotency_keys (
    actor TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status INTEGER,
    content_type TEXT,
    response TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (actor, method, path, key)
) WITHOUT ROWID;

CREATE INDEX idx_idempotency_keys_created ON idempotency_keys (created_at);
