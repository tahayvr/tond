// openapi/generate.rs — build docs/openapi.json from the route table
//
// build() walks routes::route_table() (handler extractors) and
// catalog::entries() (summaries, responses) and assembles an OpenAPI 3.1
// document. JSON Schemas come from schemars: request bodies and queries
// with the Deserialize contract, responses with the Serialize contract (a
// type whose two shapes differ gets a separate `<Name>Input` schema).
//
// The tests below fail when a route has no catalog entry (or vice versa),
// when a $ref dangles, and when docs/openapi.json is out of date:
//   just openapi    (TOND_UPDATE_OPENAPI=1 cargo test -p tond-server openapi)

use std::collections::{BTreeMap, BTreeSet, HashMap};

use schemars::{
    SchemaGenerator,
    generate::{Contract, SchemaSettings},
};
use serde_json::{Map, Value, json};
use tond_common::{ERROR_CODES, exit};

use super::catalog::{self, Doc, Resp};
use super::describe::{Arg, RouteInfo};
use crate::idempotency;

/// Version of the API contract (the document), not of the Tond release.
pub const API_VERSION: &str = "1.0.0";

const SCHEMAS: &str = "/components/schemas";

fn generator(contract: Contract) -> SchemaGenerator {
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = SCHEMAS.into();
    settings.meta_schema = None;
    settings.contract = contract;
    settings.into_generator()
}

/// Problems that make the document wrong (the tests assert there are none).
#[derive(Debug, Default)]
pub struct Problems(pub Vec<String>);

pub struct Built {
    pub doc: Value,
    pub problems: Problems,
}

pub fn build() -> Built {
    let mut problems = Problems::default();
    let mut req_gen = generator(Contract::Deserialize);
    let mut resp_gen = generator(Contract::Serialize);
    let routes = crate::routes::route_table(&mut req_gen);

    // Catalog coverage, both ways.
    let mut docs: HashMap<&str, Doc> = HashMap::new();
    for d in catalog::entries() {
        if docs.insert(d.key, d.clone()).is_some() {
            problems
                .0
                .push(format!("duplicate catalog entry {}", d.key));
        }
    }
    let route_keys: BTreeSet<String> = routes.iter().map(RouteInfo::key).collect();
    for r in &routes {
        if !docs.contains_key(r.key().as_str()) {
            problems.0.push(format!(
                "route {} ({}) has no entry in openapi/catalog.rs",
                r.key(),
                r.handler
            ));
        }
    }
    for key in docs.keys() {
        if !route_keys.contains(*key) {
            problems
                .0
                .push(format!("catalog entry {key} matches no route in routes.rs"));
        }
    }

    let op_ids = operation_ids(&routes);
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for r in &routes {
        let Some(doc) = docs.get(r.key().as_str()) else {
            continue;
        };
        let op = operation(r, doc, &op_ids[&r.key()], &mut resp_gen);
        paths
            .entry(r.path.to_string())
            .or_default()
            .insert(r.method.to_ascii_lowercase(), op);
    }
    let mut paths_value = serde_json::to_value(&paths).unwrap_or_default();

    // Merge definitions: responses first, then requests (renaming differing ones).
    let mut schemas = resp_gen.take_definitions(true);
    let mut renames = HashMap::new();
    let req_defs = req_gen.take_definitions(true);
    for (name, schema) in &req_defs {
        match schemas.get(name) {
            Some(existing) if existing != schema => {
                renames.insert(name.clone(), format!("{name}Input"));
            }
            _ => {}
        }
    }
    for (name, mut schema) in req_defs {
        rewrite_refs(&mut schema, &renames);
        let target = renames.get(&name).cloned().unwrap_or(name);
        schemas.entry(target).or_insert(schema);
    }
    rewrite_request_refs(&mut paths_value, &renames);
    for (name, schema) in fixed_schemas() {
        schemas.insert(name, schema);
    }
    let schemas: BTreeMap<String, Value> = schemas.into_iter().collect();

    let doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Tond API",
            "version": API_VERSION,
            "summary": "Deploy and run apps, databases and cron jobs on one server.",
            "description": DESCRIPTION,
            "license": {"name": "Apache-2.0", "identifier": "Apache-2.0"},
        },
        "servers": [{"url": "/", "description": "The Tond server (dashboard origin)"}],
        "tags": tags(),
        "paths": paths_value,
        "components": {
            "schemas": schemas,
            "responses": error_responses(),
            "parameters": {
                "IdempotencyKey": {
                    "name": "Idempotency-Key",
                    "in": "header",
                    "required": false,
                    "description": "Makes a retry safe: the first response to this key (for this caller, method and path) is stored for 24 hours and replayed for repeats with the same body. A different body with the same key is refused with 422 IDEMPOTENCY_KEY_REUSED; a repeat while the first is still running gets 409 IDEMPOTENCY_KEY_IN_USE. Replays carry `Idempotent-Replayed: true`.",
                    "schema": {"type": "string", "minLength": 1, "maxLength": 255},
                    "example": "3f2b8c1e-5d4a-4f7b-9a61-2c0e8d9b7a10",
                }
            },
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "tond_pat_<64 hex>",
                    "description": "An API token from `tond token create` or Settings → API tokens. Its scopes (and its owner's role) decide what it may do; see `x-tond-scopes` on each operation.",
                },
                "sessionCookie": {
                    "type": "apiKey",
                    "in": "cookie",
                    "name": "__Host-tond_session",
                    "description": "The dashboard session, set by POST /api/v1/auth/login. Mutations with a cookie also need csrfHeader.",
                },
                "csrfHeader": {
                    "type": "apiKey",
                    "in": "header",
                    "name": "X-Tond-CSRF",
                    "description": "Any value; required with sessionCookie on POST/PUT/PATCH/DELETE (and the Origin must be the dashboard's).",
                },
            },
        },
    });

    check_refs(&doc, &mut problems);
    Built { doc, problems }
}

const DESCRIPTION: &str = "\
The HTTP API of a Tond server: everything the dashboard and the `tond` CLI do.

**Versioning.** Everything is under `/api/v1` (WebSockets included). Additive changes \
(new endpoints, new optional fields, new error codes) happen within v1; anything else \
gets a new version. `/health` and `/metrics` stay at the root.

**Authentication.** Send `Authorization: Bearer <token>` with an API token (scoped; see \
`x-tond-scopes` on each operation, and `GET /api/v1/tokens/catalog`). The dashboard uses \
the `__Host-tond_session` cookie plus the `X-Tond-CSRF` header on mutations. On the \
server itself the root-only Unix socket (`/opt/tond/data/tond.sock`) needs no credentials \
and holds every scope. WebSockets also accept `?token=<token>`.

**Responses.** Every JSON response is an envelope: `{\"success\": true, \"data\": …}` or \
`{\"success\": false, \"data\": null, \"error\": {\"code\", \"message\", \"hint\", \"field\"}}`. \
Switch on `error.code` (see the ErrorCode schema: each code has a fixed HTTP status and \
`tond` CLI exit code); `message` is for humans and may change.

**Retries.** Mutations accept an `Idempotency-Key` header (see the parameter); replays \
return the stored response.

**Rate limits.** Operations marked with a 429 response share a per-client token bucket; \
the 429 carries `Retry-After`.";

fn tags() -> Value {
    json!([
        {"name": "Apps", "description": "Create, configure and control apps."},
        {"name": "Deployments", "description": "Deploy, follow and roll back."},
        {"name": "Environment", "description": "Environment variables (values are encrypted at rest)."},
        {"name": "Sources", "description": "Git forges, registries, webhooks and deploy keys."},
        {"name": "Previews", "description": "Preview deployments of branches and pull requests."},
        {"name": "Databases", "description": "Managed databases."},
        {"name": "Cron", "description": "Scheduled container runs."},
        {"name": "Projects", "description": "Groups of apps, databases and cron jobs."},
        {"name": "Snapshots", "description": "Encrypted snapshots, restores and storage providers."},
        {"name": "Observability", "description": "Metrics, traffic, events, access logs and uptime."},
        {"name": "Alerts", "description": "Alert rules, live alerts and history."},
        {"name": "Notifications", "description": "Notification channels."},
        {"name": "System", "description": "The host, Docker disk, heartbeat and Tond itself."},
        {"name": "Settings", "description": "Server-wide settings."},
        {"name": "Certificates", "description": "TLS certificates."},
        {"name": "Terminal", "description": "Browser terminals into containers and the host."},
        {"name": "Auth", "description": "Dashboard sign-in, sessions and two-factor auth."},
        {"name": "Users", "description": "Dashboard users."},
        {"name": "Tokens", "description": "Scoped API tokens."},
        {"name": "Audit log", "description": "The hash-chained audit log and its shipping."},
        {"name": "Webhooks", "description": "Endpoints called by Git forges (signed, no token)."},
        {"name": "Meta", "description": "Probes, metrics and this document."},
    ])
}

/// The tag of a path: the first matching prefix wins.
fn tag_of(path: &str) -> &'static str {
    const RULES: &[(&str, &str)] = &[
        ("/api/v1/apps/{id}/deployments", "Deployments"),
        ("/api/v1/apps/{id}/deploy", "Deployments"),
        ("/api/v1/apps/{id}/rollback", "Deployments"),
        ("/api/v1/apps/{id}/upload", "Deployments"),
        ("/api/v1/apps/{id}/env", "Environment"),
        ("/api/v1/apps/{id}/preview-env", "Environment"),
        ("/api/v1/apps/{id}/preview", "Previews"),
        ("/api/v1/previews", "Observability"),
        ("/api/v1/apps/{id}/metrics", "Observability"),
        ("/api/v1/apps/{id}/requests", "Observability"),
        ("/api/v1/apps/{id}/events", "Observability"),
        ("/api/v1/apps/{id}/access-log", "Observability"),
        ("/api/v1/apps/{id}/uptime", "Observability"),
        ("/api/v1/apps/{id}/logs", "Observability"),
        ("/api/v1/apps/{id}/webhook", "Sources"),
        ("/api/v1/apps/{id}/deploy-key", "Sources"),
        ("/api/v1/apps/{id}/source", "Sources"),
        ("/api/v1/apps/{id}/auto-deploy", "Sources"),
        ("/api/v1/apps/{id}/github", "Sources"),
        ("/api/v1/apps/{id}/gitlab", "Sources"),
        ("/api/v1/apps/{id}/registry", "Sources"),
        ("/api/v1/apps/{id}/databases", "Databases"),
        ("/api/v1/apps/{id}/project", "Projects"),
        ("/api/v1/apps", "Apps"),
        ("/api/v1/compose", "Apps"),
        ("/api/v1/templates", "Apps"),
        ("/api/v1/domains", "Apps"),
        ("/api/v1/databases", "Databases"),
        ("/api/v1/cron", "Cron"),
        ("/api/v1/projects", "Projects"),
        ("/api/v1/snapshots", "Snapshots"),
        ("/api/v1/storage-providers", "Snapshots"),
        ("/api/v1/alerts", "Alerts"),
        ("/api/v1/integrations", "Notifications"),
        ("/api/v1/audit-log", "Audit log"),
        ("/api/v1/system/uptime", "Observability"),
        ("/api/v1/system/requests", "Observability"),
        ("/api/v1/system/metrics", "Observability"),
        ("/api/v1/uptime", "Observability"),
        ("/api/v1/system", "System"),
        ("/api/v1/settings", "Settings"),
        ("/api/v1/onboarding", "Settings"),
        ("/api/v1/certs", "Certificates"),
        ("/api/v1/terminal", "Terminal"),
        ("/api/v1/auth", "Auth"),
        ("/api/v1/users", "Users"),
        ("/api/v1/tokens", "Tokens"),
        ("/api/v1/webhooks", "Webhooks"),
        ("/webhooks", "Webhooks"),
        ("/api/v1/github-apps", "Sources"),
        ("/api/v1/gitlab-tokens", "Sources"),
        ("/api/v1/registry-credentials", "Sources"),
        ("/api/v1/git", "Sources"),
        ("/api/v1/images", "Sources"),
    ];
    RULES
        .iter()
        .find(|(prefix, _)| path.starts_with(prefix))
        .map_or("Meta", |(_, tag)| tag)
}

/// operationId per route: the handler name, qualified by its module when
/// two handlers share a name.
fn operation_ids(routes: &[RouteInfo]) -> HashMap<String, String> {
    let last = |h: &str| h.rsplit("::").next().unwrap_or(h).to_string();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in routes {
        *counts.entry(last(&r.handler)).or_default() += 1;
    }
    routes
        .iter()
        .map(|r| {
            let name = last(&r.handler);
            let id = if counts[&name] > 1 {
                let parts: Vec<&str> = r.handler.split("::").collect();
                let module = parts.len().checked_sub(2).map_or("root", |i| parts[i]);
                format!("{module}_{name}")
            } else {
                name
            };
            (r.key(), id)
        })
        .collect()
}

fn is_mutation(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

fn operation(r: &RouteInfo, doc: &Doc, op_id: &str, resp_gen: &mut SchemaGenerator) -> Value {
    let mut op = Map::new();
    op.insert("operationId".into(), json!(op_id));
    op.insert("summary".into(), json!(doc.summary));
    if let Some(d) = doc.description {
        op.insert("description".into(), json!(d));
    }
    op.insert("tags".into(), json!([tag_of(r.path)]));

    // Parameters: path, query, Idempotency-Key, WebSocket token.
    let mut params = Vec::new();
    for name in path_params(r.path) {
        let description = match (name.as_str(), r.path.starts_with("/api/v1/apps/{id}")) {
            ("id", true) => "App id or name.",
            ("id", false) if r.path.starts_with("/api/v1/projects/{id}") => "Project id or name.",
            ("id", false) if r.path.starts_with("/api/v1/databases/{id}") => "Database id or name.",
            _ => "",
        };
        let mut p =
            json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}});
        if !description.is_empty() {
            p["description"] = json!(description);
        }
        params.push(p);
    }
    for arg in &r.args {
        if let Arg::Query(schema) = arg {
            params.extend(query_params(schema.as_value()));
        }
    }
    let websocket = r.has(|a| matches!(a, Arg::WebSocket));
    if websocket {
        params.push(json!({
            "name": "token", "in": "query", "required": false,
            "description": "API token, for WebSocket clients that can't set headers.",
            "schema": {"type": "string"},
        }));
    }
    if idempotency::applies(r.method, r.path) {
        params.push(json!({"$ref": "#/components/parameters/IdempotencyKey"}));
    }
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }

    // Request body.
    for arg in &r.args {
        let body = match arg {
            Arg::JsonBody(schema) => Some(json!({
                "required": true,
                "content": {"application/json": {"schema": schema.as_value()}},
            })),
            Arg::Multipart => Some(json!({
                "required": true,
                "content": {"multipart/form-data": {"schema": {
                    "type": "object",
                    "required": ["file"],
                    "properties": {"file": {"type": "string", "contentMediaType": "application/gzip", "description": "A .tar.gz of the source."}},
                }}},
            })),
            Arg::RawBody(ct) => Some(json!({
                "required": true,
                "content": {*ct: {"schema": {"type": "string"}}},
            })),
            _ => None,
        };
        if let Some(body) = body {
            op.insert("requestBody".into(), body);
        }
    }

    // Security and scopes.
    let auth = r.scopes();
    let optional = r.has(|a| matches!(a, Arg::OptionalAuth));
    let local_only = r.has(|a| matches!(a, Arg::LocalOnly));
    if let Some(scopes) = auth {
        let cookie = if is_mutation(r.method) {
            json!({"sessionCookie": [], "csrfHeader": []})
        } else {
            json!({"sessionCookie": []})
        };
        let mut security = vec![json!({"bearerAuth": []}), cookie];
        if optional {
            security.push(json!({}));
        }
        op.insert("security".into(), Value::Array(security));
        let mut all: Vec<String> = scopes.iter().map(|s| s.as_str().to_string()).collect();
        all.extend(doc.extra_scopes.iter().map(|s| s.to_string()));
        all.dedup();
        op.insert("x-tond-scopes".into(), json!(all));
    } else if optional {
        op.insert(
            "security".into(),
            json!([{}, {"bearerAuth": []}, {"sessionCookie": []}]),
        );
        op.insert("x-tond-scopes".into(), json!(doc.extra_scopes));
    } else {
        op.insert("security".into(), json!([]));
        op.insert("x-tond-scopes".into(), json!(doc.extra_scopes));
    }
    if local_only {
        op.insert("x-tond-local-socket-only".into(), json!(true));
    }
    if websocket {
        op.insert("x-tond-websocket".into(), json!(true));
    }

    // Responses.
    let mut responses = Map::new();
    let (status, success) = success_response(doc, resp_gen);
    responses.insert(status, success);
    let has_input = op.contains_key("requestBody") || op.contains_key("parameters");
    if has_input {
        responses.insert("400".into(), error_ref("BadRequest"));
    }
    if auth.is_some() || local_only {
        responses.insert("401".into(), error_ref("Unauthorized"));
        responses.insert("403".into(), error_ref("Forbidden"));
    }
    if !path_params(r.path).is_empty() {
        responses.insert("404".into(), error_ref("NotFound"));
    }
    if idempotency::applies(r.method, r.path) {
        responses.insert("409".into(), error_ref("Conflict"));
        responses.insert("422".into(), error_ref("Unprocessable"));
    }
    if r.has(|a| matches!(a, Arg::RateLimited)) {
        responses.insert("429".into(), error_ref("TooManyRequests"));
    }
    responses.insert("500".into(), error_ref("ServerError"));
    op.insert("responses".into(), Value::Object(responses));
    Value::Object(op)
}

fn success_response(doc: &Doc, resp_gen: &mut SchemaGenerator) -> (String, Value) {
    let description = match doc.status {
        201 => "Created.",
        202 => "Accepted: the work continues in the background.",
        _ => "OK.",
    };
    let envelope = |data: Value| {
        json!({
            "description": description,
            "content": {"application/json": {"schema": {
                "type": "object",
                "required": ["success", "data"],
                "properties": {
                    "success": {"const": true},
                    "data": data,
                    "message": {"type": "string", "description": "Informational note, when there is one."},
                },
            }}},
        })
    };
    let value = match &doc.response {
        Resp::Data(f) => envelope(f(resp_gen).to_value()),
        Resp::Message => envelope(json!({"$ref": "#/components/schemas/Message"})),
        Resp::Object(what) => envelope(json!({"description": what})),
        Resp::Null => envelope(json!({"type": "null"})),
        Resp::Raw(ct, what) => {
            let schema = if ct.starts_with("application/json") {
                json!({"type": "object"})
            } else {
                json!({"type": "string"})
            };
            json!({"description": what, "content": {*ct: {"schema": schema}}})
        }
        Resp::WebSocket(frames) => {
            return (
                "101".into(),
                json!({"description": format!("Switching Protocols to a WebSocket. {frames}")}),
            );
        }
    };
    (doc.status.to_string(), value)
}

fn error_ref(name: &str) -> Value {
    json!({"$ref": format!("#/components/responses/{name}")})
}

fn error_example(code: &str, message: &str, hint: Option<&str>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(h) = hint {
        error["hint"] = json!(h);
    }
    json!({"success": false, "data": null, "error": error})
}

fn error_responses() -> Value {
    let resp = |description: &str, examples: Vec<(&str, Value)>| {
        let examples: Map<String, Value> = examples
            .into_iter()
            .map(|(k, v)| (k.to_string(), json!({"value": v})))
            .collect();
        json!({
            "description": description,
            "content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/ErrorEnvelope"},
                "examples": examples,
            }},
        })
    };
    json!({
        "BadRequest": resp("The request is invalid (INVALID_INPUT, INVALID_JSON).", vec![
            ("invalid_input", json!({"success": false, "data": null, "error": {"code": "INVALID_INPUT", "message": "App name must be alphanumeric (hyphens allowed)", "field": "name"}})),
        ]),
        "Unauthorized": resp("Missing, invalid or expired credentials.", vec![
            ("unauthenticated", error_example("UNAUTHENTICATED", "Authentication required. Use Authorization: Bearer <token> or sign in to the dashboard.", Some("Send Authorization: Bearer <token>, or sign in to the dashboard."))),
        ]),
        "Forbidden": resp("Authenticated, but not allowed (SCOPE_DENIED, RESOURCE_DENIED, APPROVAL_REQUIRED, …).", vec![
            ("scope_denied", error_example("SCOPE_DENIED", "This API token doesn't have the apps:deploy scope. Create a token that includes it.", Some("Use a token that includes the scope, or ask an admin for a role that has it."))),
        ]),
        "NotFound": resp("The resource does not exist.", vec![
            ("app_not_found", error_example("APP_NOT_FOUND", "App not found: web", Some("List apps with `tond apps list` (or GET /api/v1/apps)."))),
        ]),
        "Conflict": resp("Conflicts with the current state (DEPLOY_IN_PROGRESS, APP_ALREADY_EXISTS, IDEMPOTENCY_KEY_IN_USE, …).", vec![
            ("in_use", error_example("IDEMPOTENCY_KEY_IN_USE", "A request with this Idempotency-Key is still being processed.", Some("Retry shortly with the same key to get its result."))),
        ]),
        "Unprocessable": resp("The Idempotency-Key was used with a different request (IDEMPOTENCY_KEY_REUSED).", vec![
            ("reused", error_example("IDEMPOTENCY_KEY_REUSED", "This Idempotency-Key was already used with a different request body.", Some("Use a new Idempotency-Key for a different request."))),
        ]),
        "TooManyRequests": resp("Rate limited (RATE_LIMITED); see Retry-After.", vec![
            ("rate_limited", error_example("RATE_LIMITED", "Rate limit exceeded. Try again later.", None)),
        ]),
        "ServerError": resp("The server failed (INTERNAL, DATABASE_ERROR, DOCKER_ERROR).", vec![
            ("internal", error_example("INTERNAL", "Internal error: …", Some("Check tond-server's logs (`journalctl -u tond-server`)."))),
        ]),
    })
}

/// Schemas that aren't derived from a Rust type.
fn fixed_schemas() -> Vec<(String, Value)> {
    let codes: Vec<&str> = ERROR_CODES.iter().map(|c| c.code).collect();
    let catalog: Vec<Value> = ERROR_CODES
        .iter()
        .map(|c| {
            json!({"code": c.code, "status": c.status, "exit_code": c.exit, "description": c.description})
        })
        .collect();
    let exits: Vec<Value> = exit::TABLE
        .iter()
        .map(|(n, name, meaning)| json!({"exit_code": n, "name": name, "meaning": meaning}))
        .collect();
    vec![
        (
            "ErrorCode".into(),
            json!({
                "type": "string",
                "enum": codes,
                "description": "Stable error code. Each has a fixed HTTP status and `tond` CLI exit code (x-tond-codes, x-tond-exit-codes). New codes may be added within v1; handle unknown codes by HTTP status.",
                "x-tond-codes": catalog,
                "x-tond-exit-codes": exits,
            }),
        ),
        (
            "ErrorBody".into(),
            json!({
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": {"$ref": "#/components/schemas/ErrorCode"},
                    "message": {"type": "string", "description": "Human-readable; may change between releases."},
                    "hint": {"type": "string", "description": "What to do next."},
                    "field": {"type": "string", "description": "The request field the error is about (dotted path)."},
                },
            }),
        ),
        (
            "ErrorEnvelope".into(),
            json!({
                "type": "object",
                "required": ["success", "data", "error"],
                "properties": {
                    "success": {"const": false},
                    "data": {"type": "null"},
                    "error": {"$ref": "#/components/schemas/ErrorBody"},
                },
            }),
        ),
        (
            "Message".into(),
            json!({
                "type": "object",
                "required": ["message"],
                "properties": {"message": {"type": "string"}},
            }),
        ),
    ]
}

/// `{name}` segments of a path template.
fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|s| s.trim_start_matches('*').to_string())
        .collect()
}

/// One query parameter per property of an (object) query schema.
fn query_params(schema: &Value) -> Vec<Value> {
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    props
        .iter()
        .map(|(name, prop)| {
            let mut p = json!({
                "name": name,
                "in": "query",
                "required": required.contains(name.as_str()),
                "schema": prop,
            });
            if let Some(d) = prop.get("description") {
                p["description"] = d.clone();
            }
            p
        })
        .collect()
}

fn rewrite_refs(value: &mut Value, renames: &HashMap<String, String>) {
    if renames.is_empty() {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref")
                && let Some(name) = r.strip_prefix("#/components/schemas/")
                && let Some(new) = renames.get(name)
            {
                *r = format!("#/components/schemas/{new}");
            }
            for v in map.values_mut() {
                rewrite_refs(v, renames);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rewrite_refs(v, renames)),
        _ => {}
    }
}

/// Rewrite refs in request bodies and parameters only (responses keep the
/// Serialize-contract names).
fn rewrite_request_refs(paths: &mut Value, renames: &HashMap<String, String>) {
    let Some(paths) = paths.as_object_mut() else {
        return;
    };
    for item in paths.values_mut() {
        let Some(ops) = item.as_object_mut() else {
            continue;
        };
        for op in ops.values_mut() {
            if let Some(body) = op.get_mut("requestBody") {
                rewrite_refs(body, renames);
            }
            if let Some(params) = op.get_mut("parameters") {
                rewrite_refs(params, renames);
            }
        }
    }
}

fn check_refs(doc: &Value, problems: &mut Problems) {
    fn walk(v: &Value, doc: &Value, problems: &mut Problems) {
        match v {
            Value::Object(map) => {
                if let Some(Value::String(r)) = map.get("$ref") {
                    let pointer = r.trim_start_matches('#');
                    if doc.pointer(pointer).is_none() {
                        problems.0.push(format!("dangling $ref {r}"));
                    }
                }
                map.values().for_each(|v| walk(v, doc, problems));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, doc, problems)),
            _ => {}
        }
    }
    walk(doc, doc, problems);
}

/// The document as written to docs/openapi.json.
pub fn render() -> (String, Problems) {
    let built = build();
    let mut text = serde_json::to_string_pretty(&built.doc).unwrap_or_default();
    text.push('\n');
    (text, built.problems)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn spec_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi.json")
    }

    #[test]
    fn every_route_is_documented_and_every_ref_resolves() {
        let built = build();
        assert!(
            built.problems.0.is_empty(),
            "OpenAPI problems:\n  {}",
            built.problems.0.join("\n  ")
        );
    }

    #[test]
    fn operation_ids_are_unique_and_path_params_declared() {
        let doc = build().doc;
        let mut ids = BTreeSet::new();
        for (path, item) in doc["paths"].as_object().into_iter().flatten() {
            for (method, op) in item.as_object().into_iter().flatten() {
                let id = op["operationId"].as_str().unwrap_or_default().to_string();
                assert!(ids.insert(id.clone()), "duplicate operationId {id}");
                let declared: BTreeSet<&str> = op["parameters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["in"] == "path")
                    .filter_map(|p| p["name"].as_str())
                    .collect();
                for name in path_params(path) {
                    assert!(
                        declared.contains(name.as_str()),
                        "{method} {path} doesn't declare path parameter {name}"
                    );
                }
                assert!(
                    op["x-tond-scopes"].is_array(),
                    "{method} {path} lacks x-tond-scopes"
                );
                assert!(
                    op["responses"]["500"].is_object(),
                    "{method} {path} lacks errors"
                );
            }
        }
    }

    #[test]
    fn scopes_come_from_the_handlers() {
        let doc = build().doc;
        let op = |m: &str, p: &str| doc["paths"][p][m].clone();
        assert_eq!(
            op("post", "/api/v1/apps/{id}/deploy")["x-tond-scopes"],
            json!(["apps:deploy"])
        );
        assert_eq!(
            op("get", "/api/v1/databases")["x-tond-scopes"],
            json!(["db:read"])
        );
        assert_eq!(op("get", "/api/v1/openapi.json")["security"], json!([]));
        assert_eq!(
            op("post", "/api/v1/apps")["requestBody"]["content"]["application/json"]["schema"]["$ref"]
                .as_str()
                .map(|r| r.starts_with("#/components/schemas/CreateAppRequest")),
            Some(true)
        );
        assert_eq!(
            op("post", "/api/v1/apps/{id}/deploy")["parameters"]
                .as_array()
                .map(|p| p
                    .iter()
                    .any(|p| p["$ref"] == "#/components/parameters/IdempotencyKey")),
            Some(true),
            "deploys take an Idempotency-Key"
        );
        assert_eq!(
            op(
                "get",
                "/api/v1/apps/{id}/deployments/{deployment_id}/stream"
            )["x-tond-websocket"],
            json!(true)
        );
        let codes = &doc["components"]["schemas"]["ErrorCode"]["enum"];
        for code in [
            "APP_NOT_FOUND",
            "DEPLOY_IN_PROGRESS",
            "SCOPE_DENIED",
            "APPROVAL_REQUIRED",
            "BUILD_FAILED",
        ] {
            assert!(
                codes
                    .as_array()
                    .is_some_and(|c| c.iter().any(|v| v == code)),
                "{code} missing from ErrorCode"
            );
        }
    }

    /// docs/openapi.json must match the code. Regenerate with `just openapi`.
    #[test]
    fn openapi_json_is_up_to_date() {
        let (text, problems) = render();
        assert!(problems.0.is_empty(), "{problems:?}");
        let path = spec_path();
        let gz_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/openapi/openapi.json.gz");
        if std::env::var_os("TOND_UPDATE_OPENAPI").is_some() {
            std::fs::write(&path, &text)
                .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            std::io::Write::write_all(&mut enc, text.as_bytes())
                .unwrap_or_else(|e| panic!("gzip: {e}"));
            let gz = enc.finish().unwrap_or_else(|e| panic!("gzip: {e}"));
            std::fs::write(&gz_path, gz)
                .unwrap_or_else(|e| panic!("writing {}: {e}", gz_path.display()));
            return;
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == text,
            "docs/openapi.json is out of date with the routes and types. Run `just openapi` and commit the result."
        );
        assert!(
            crate::openapi::spec_text() == text,
            "the embedded openapi.json.gz is stale. Run `just openapi` and rebuild."
        );
    }
}
