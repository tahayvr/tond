-- Compose services that left the compose file (a rollback to an older file,
-- or an edit) are parked instead of deleted: the row keeps the user's
-- published port and app_domains keeps its domains, but host_port is cleared
-- so tond-proxy stops routing them (route_compose_services_update fires).
-- The service comes back with its domains when a later file declares it again.
ALTER TABLE compose_services ADD COLUMN parked INTEGER NOT NULL DEFAULT 0;
-- Replicas the file asks for (deploy.replicas / scale; 1 when unset). A
-- service Tond publishes for routing must run one replica: every replica
-- would bind the same loopback host port.
ALTER TABLE compose_services ADD COLUMN replicas INTEGER NOT NULL DEFAULT 1;
