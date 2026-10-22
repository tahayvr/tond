// route_queries.rs — SQL tond-proxy routes on, shared with tond-server's tests
//
// tond-proxy reads these from tond.db (read-only). They live here so that
// tond-server's tests can run the exact query against rows its handlers
// wrote, proving that what the API accepts is what the proxy routes.

/// Compose service domains: one row per routable service domain, as
/// `(domain, app id, app name, service, host port, access_log,
/// access_log_query)`. Redirect rows, services without a host port (not
/// published or not deployed yet), apps that aren't up and template apps
/// whose admin account Tond is still creating (admin_setup 'pending': nobody
/// may reach them before it exists) are left out.
pub const COMPOSE_SERVICE_ROUTES: &str =
    "SELECT d.domain, a.id, a.name, d.service, s.host_port, a.access_log, a.access_log_query
     FROM app_domains d
     JOIN apps a ON a.id = d.app_id
     JOIN compose_services s ON s.app_id = d.app_id AND s.name = d.service
     WHERE d.service IS NOT NULL AND d.kind != 'redirect'
       AND a.status IN ('running', 'building', 'unhealthy') AND s.host_port IS NOT NULL
       AND (a.admin_setup IS NULL OR a.admin_setup != 'pending')";
