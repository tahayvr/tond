-- Per-app outbound network policy (crates/tond-server/src/egress.rs), enforced
-- by tond-server in the iptables chain TOND-EGRESS (hooked into DOCKER-USER).
--   egress_policy          'open' (default) | 'restricted' | 'none'
--   egress_allowlist       JSON array of "<host-or-cidr>[:<port>[,<port>…]]"
--                          (used by 'restricted', and by previews set to it)
--   preview_egress_policy  the same three values for the app's previews,
--                          which run pull-request code: 'none' by default
ALTER TABLE apps ADD COLUMN egress_policy TEXT NOT NULL DEFAULT 'open'
  CHECK (egress_policy IN ('open', 'restricted', 'none'));
ALTER TABLE apps ADD COLUMN egress_allowlist TEXT NOT NULL DEFAULT '[]';
ALTER TABLE apps ADD COLUMN preview_egress_policy TEXT NOT NULL DEFAULT 'none'
  CHECK (preview_egress_policy IN ('open', 'restricted', 'none'));
