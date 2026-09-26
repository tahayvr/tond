-- Docker Compose as a first-class app type.
--
-- compose_services: one row per service of a compose app, synced from the
-- compose file on save and on every deploy. User intent (the container port
-- Tond publishes for routing) survives redeploys; what Docker reports
-- (container, state, health) is refreshed on deploy, on service actions and
-- when the dashboard lists the services.
CREATE TABLE compose_services(
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  -- order in the compose file
  position INTEGER NOT NULL DEFAULT 0,
  -- image ref from the compose file (NULL for build-only services)
  image TEXT,
  -- 1 when the service has a build: section
  builds INTEGER NOT NULL DEFAULT 0,
  -- JSON array of container ports the file declares (ports: and expose:)
  ports TEXT NOT NULL DEFAULT '[]',
  -- 1 when the service defines a healthcheck (compose healthchecks are
  -- waited for on deploy)
  has_healthcheck INTEGER NOT NULL DEFAULT 0,
  -- container port Tond publishes on 127.0.0.1:host_port for routing
  published_port INTEGER,
  -- loopback host port allocated for published_port (stable across deploys)
  host_port INTEGER,
  -- container port host_port maps to in the running containers (differs
  -- from published_port until the next deploy after a change)
  routed_port INTEGER,
  container_id TEXT,
  -- "pending" | "running" | "restarting" | "paused" | "created" | "exited"
  -- | "dead" | "missing"
  status TEXT NOT NULL DEFAULT 'pending',
  -- "healthy" | "unhealthy" | "starting"; NULL without a healthcheck
  health TEXT,
  exit_code INTEGER,
  -- the pinned local image the running container uses
  running_image TEXT,
  -- its image id (sha256:...)
  image_digest TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY(app_id, name)
) WITHOUT ROWID;

-- Domains routed by tond-proxy. Generic on purpose: a row maps a domain to
-- an app, and for compose apps to one of its services and the container port
-- Tond publishes for it. `service` is NULL for single-container apps (the
-- app's own domain and port).
CREATE TABLE app_domains(
  domain TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  service TEXT,
  port INTEGER,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX app_domains_app_idx ON app_domains(app_id, service);

-- What a compose deployment ran, per service: the image it was built or
-- pulled as and the pinned local tag (tond-<app>/<service>:<deployment>) with
-- its image id, so a rollback restores exactly that set.
CREATE TABLE deployment_services(
  deployment_id TEXT NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
  service TEXT NOT NULL,
  -- "build" | "pull" | "reuse" (rollback)
  source TEXT NOT NULL,
  -- image ref from the compose file, or the build context
  source_image TEXT,
  -- pinned local tag
  image TEXT,
  -- image id (sha256:...) of the pinned tag, plus the registry digest of a
  -- pulled image when known ("sha256:...@repo@sha256:...")
  digest TEXT,
  -- "pending" | "building" | "pulling" | "ready" | "starting" | "healthy"
  -- | "running" | "failed"
  status TEXT NOT NULL DEFAULT 'pending',
  PRIMARY KEY(deployment_id, service)
) WITHOUT ROWID;

-- The compose file a deployment ran (as written by the user or read from the
-- repository), for rollback.
ALTER TABLE deployments ADD COLUMN compose_source TEXT;
-- Git-sourced compose apps: path of the compose file inside the repository.
ALTER TABLE apps ADD COLUMN compose_path TEXT;

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
CREATE TRIGGER route_compose_services_delete AFTER DELETE ON compose_services
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_compose_services_update AFTER UPDATE OF host_port ON compose_services
WHEN OLD.host_port IS NOT NEW.host_port
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_compose_services_insert AFTER INSERT ON compose_services
WHEN NEW.host_port IS NOT NULL
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
