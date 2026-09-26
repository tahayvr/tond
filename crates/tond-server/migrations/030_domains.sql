-- Phase 3 · domains: several domains per app, redirects, DNS providers for
-- ACME DNS-01 (wildcards), uploaded certificates.

-- Every domain an app answers on. `apps.domain` stays as a read-only mirror
-- of the primary domain (maintained by the triggers below); write here.
CREATE TABLE app_domains(
  domain TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  -- NULL: the app's main container; else a compose service name
  service TEXT,
  -- container port of `service` (NULL: the app's container_port)
  port INTEGER,
  -- "primary" | "alias" | "redirect"
  kind TEXT NOT NULL DEFAULT 'alias' CHECK(kind IN ('primary', 'alias', 'redirect')),
  -- redirect target (another domain of the same app), set iff kind = 'redirect'
  redirect_to TEXT,
  redirect_code INTEGER NOT NULL DEFAULT 308 CHECK(redirect_code IN (301, 308)),
  created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  CHECK((kind = 'redirect') = (redirect_to IS NOT NULL))
);
CREATE INDEX app_domains_app_idx ON app_domains(app_id);
CREATE UNIQUE INDEX app_domains_one_primary ON app_domains(app_id) WHERE kind = 'primary';

INSERT INTO app_domains(domain, app_id, kind)
SELECT domain, id, 'primary' FROM apps WHERE domain IS NOT NULL AND domain != '';

-- apps.domain mirrors the primary domain.
CREATE TRIGGER app_domains_primary_insert AFTER INSERT ON app_domains
WHEN NEW.kind = 'primary'
BEGIN
  UPDATE apps SET domain = NEW.domain WHERE id = NEW.app_id;
END;
CREATE TRIGGER app_domains_primary_update AFTER UPDATE ON app_domains
WHEN OLD.kind = 'primary' OR NEW.kind = 'primary'
BEGIN
  UPDATE apps SET domain = (
    SELECT domain FROM app_domains WHERE app_id = NEW.app_id AND kind = 'primary'
  ) WHERE id = NEW.app_id;
END;
CREATE TRIGGER app_domains_primary_delete AFTER DELETE ON app_domains
WHEN OLD.kind = 'primary'
BEGIN
  UPDATE apps SET domain = (
    SELECT domain FROM app_domains WHERE app_id = OLD.app_id AND kind = 'primary'
  ) WHERE id = OLD.app_id;
END;
-- An app created with a domain gets it as its primary domain.
CREATE TRIGGER apps_domain_seed AFTER INSERT ON apps
WHEN NEW.domain IS NOT NULL AND NEW.domain != ''
BEGIN
  INSERT OR IGNORE INTO app_domains(domain, app_id, kind) VALUES (NEW.domain, NEW.id, 'primary');
END;

-- tond-proxy reloads its routing table when any domain changes.
CREATE TRIGGER route_app_domains_insert AFTER INSERT ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_app_domains_delete AFTER DELETE ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_app_domains_update AFTER UPDATE ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
-- The apps domain decides which wildcard certificate tond-proxy orders.
CREATE TRIGGER route_settings_apps_domain_insert AFTER INSERT ON settings
WHEN NEW.key = 'apps_domain'
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_settings_apps_domain_update AFTER UPDATE ON settings
WHEN NEW.key = 'apps_domain' AND OLD.value IS NOT NEW.value
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

-- DNS hosting APIs Tond may publish ACME DNS-01 TXT records with. The token
-- is write-only: encrypted (secret_key.rs, Purpose::DnsProviderToken) and
-- never returned. tond-proxy reads `zones` (read-only) to decide which
-- domains use DNS-01; it never reads or needs the token.
CREATE TABLE dns_providers(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  -- "cloudflare" | "hetzner" | "digitalocean" | "route53" | "challtestsrv" (test only)
  kind TEXT NOT NULL,
  -- JSON array of zone names this provider may write, e.g. ["example.com"]
  zones TEXT NOT NULL DEFAULT '[]',
  -- non-secret settings (Route 53 access key id, challtestsrv URL), JSON
  config TEXT NOT NULL DEFAULT '{}',
  token_enc TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_used_at TEXT,
  last_error TEXT
);
CREATE TRIGGER route_dns_providers_insert AFTER INSERT ON dns_providers
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_dns_providers_delete AFTER DELETE ON dns_providers
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_dns_providers_zones AFTER UPDATE OF zones ON dns_providers
WHEN OLD.zones IS NOT NEW.zones
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

-- How the CA validated an ACME certificate: "http-01" | "dns-01".
-- cert_type is now "acme" | "custom" (uploaded; never renewed).
ALTER TABLE certificates ADD COLUMN challenge TEXT;
