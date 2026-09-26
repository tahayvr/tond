-- Webhook hardening.

-- Every app has a webhook secret; verification is mandatory. New apps get one
-- at creation (App::new); backfill any row that is missing it.
UPDATE apps
SET webhook_secret = lower(hex(randomblob(32)))
WHERE webhook_secret IS NULL OR webhook_secret = '';

-- Replay protection: delivery IDs / payload digests seen recently. Pruned by
-- age and capped in size by the webhook handler (handlers/webhooks.rs).
CREATE TABLE webhook_deliveries(
  key TEXT PRIMARY KEY NOT NULL,
  received_at TEXT NOT NULL
);
CREATE INDEX idx_webhook_deliveries_received_at ON webhook_deliveries(received_at);
