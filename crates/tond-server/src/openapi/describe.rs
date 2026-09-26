// openapi/describe.rs — read a handler's contract off its extractor types
//
// `describe(&handler)` works for any async fn handler: `Describe` is
// implemented for functions of up to 12 arguments whose argument types all
// implement `ArgInfo`, the same trick axum's `Handler` uses. Each extractor
// type says what it means for the spec:
//   Authz<scope::X, …>  authenticated, needs those scopes (x-tond-scopes)
//   Principal           authenticated, no scope
//   MaybePrincipal      optional authentication
//   RateLimit           per-IP rate limit (429)
//   Json<T>             JSON request body with T's schema
//   Query<T>            query parameters from T's fields
//   Multipart, Bytes…   other request bodies
//   WebSocketUpgrade    the route is a WebSocket
// A new extractor type fails to compile here until it says what it means.

use axum::{
    Json,
    body::Bytes,
    extract::{
        ConnectInfo, Multipart, Path, Query, RawQuery, Request, State, ws::WebSocketUpgrade,
    },
    http::{HeaderMap, Method, Uri},
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use tond_common::{Scope, ScopeSet};

use crate::auth::principal::ScopeReq;
use crate::auth::{Authz, ClientIp, LocalPeer, MaybePrincipal, Principal, RateLimit};
use crate::state::AppState;

/// What one handler argument contributes to the operation.
#[derive(Debug, Clone)]
pub enum Arg {
    /// No effect on the contract (State, Path, headers…).
    Ignore,
    RateLimited,
    /// Authenticated; the listed scopes are all required.
    Auth(Vec<Scope>),
    OptionalAuth,
    /// Only the root-only Unix socket (the CLI on the host) may call it.
    LocalOnly,
    JsonBody(Schema),
    Query(Schema),
    /// A non-JSON body with this media type.
    RawBody(&'static str),
    Multipart,
    WebSocket,
}

/// Contract info for an extractor type.
pub trait ArgInfo {
    fn info(generator: &mut SchemaGenerator) -> Arg;
}

/// A handler whose arguments all implement `ArgInfo`.
pub trait Describe<M> {
    fn args(generator: &mut SchemaGenerator) -> Vec<Arg>;
}

macro_rules! impl_describe {
    ($($t:ident),*) => {
        impl<F, Fut, $($t,)*> Describe<($($t,)*)> for F
        where
            F: Fn($($t),*) -> Fut,
            $($t: ArgInfo,)*
        {
            #[allow(unused_variables)]
            fn args(generator: &mut SchemaGenerator) -> Vec<Arg> {
                vec![$($t::info(generator)),*]
            }
        }
    };
}

impl_describe!();
impl_describe!(A1);
impl_describe!(A1, A2);
impl_describe!(A1, A2, A3);
impl_describe!(A1, A2, A3, A4);
impl_describe!(A1, A2, A3, A4, A5);
impl_describe!(A1, A2, A3, A4, A5, A6);
impl_describe!(A1, A2, A3, A4, A5, A6, A7);
impl_describe!(A1, A2, A3, A4, A5, A6, A7, A8);
impl_describe!(A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_describe!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_describe!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
impl_describe!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);

/// The arguments of `handler`, described.
pub fn describe<F: Describe<M>, M>(_handler: &F, generator: &mut SchemaGenerator) -> Vec<Arg> {
    F::args(generator)
}

/// One row of the route table.
#[derive(Debug, Clone)]
pub struct RouteInfo {
    pub method: &'static str,
    pub path: &'static str,
    /// e.g. `handlers::databases::list_databases`
    pub handler: String,
    pub args: Vec<Arg>,
}

impl RouteInfo {
    pub fn new(method: &'static str, path: &'static str, handler: &str, args: Vec<Arg>) -> Self {
        Self {
            method,
            path,
            handler: handler.split_whitespace().collect(),
            args,
        }
    }

    /// "GET /api/v1/apps"
    pub fn key(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn scopes(&self) -> Option<&[Scope]> {
        self.args.iter().find_map(|a| match a {
            Arg::Auth(s) => Some(s.as_slice()),
            _ => None,
        })
    }

    pub fn has(&self, pred: impl Fn(&Arg) -> bool) -> bool {
        self.args.iter().any(pred)
    }
}

// ── Extractors that don't change the contract ──

macro_rules! ignore {
    ($($t:ty),* $(,)?) => {
        $(impl ArgInfo for $t {
            fn info(_: &mut SchemaGenerator) -> Arg {
                Arg::Ignore
            }
        })*
    };
}

ignore!(HeaderMap, Method, Uri, ClientIp, RawQuery);

impl<T> ArgInfo for State<T> {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Ignore
    }
}

impl<T> ArgInfo for Path<T> {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Ignore
    }
}

impl<T> ArgInfo for ConnectInfo<T> {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Ignore
    }
}

impl<T> ArgInfo for axum::Extension<T> {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Ignore
    }
}

// ── Auth ──

impl ArgInfo for RateLimit {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::RateLimited
    }
}

impl<R: ScopeReq> ArgInfo for Authz<R> {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Auth(required_scopes::<R>())
    }
}

impl ArgInfo for Principal {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Auth(Vec::new())
    }
}

impl ArgInfo for MaybePrincipal {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::OptionalAuth
    }
}

impl ArgInfo for LocalPeer {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::LocalOnly
    }
}

/// The scopes `R` requires: those whose absence alone makes it fail.
pub fn required_scopes<R: ScopeReq>() -> Vec<Scope> {
    Scope::ALL
        .iter()
        .copied()
        .filter(|s| {
            let mut without = ScopeSet::empty();
            for other in Scope::ALL.iter().copied().filter(|o| o != s) {
                without.insert(other);
            }
            R::missing(without).is_some()
        })
        .collect()
}

// ── Bodies and queries ──

impl<T: JsonSchema> ArgInfo for Json<T> {
    fn info(generator: &mut SchemaGenerator) -> Arg {
        Arg::JsonBody(generator.subschema_for::<T>())
    }
}

impl<T: JsonSchema> ArgInfo for Option<Json<T>> {
    fn info(generator: &mut SchemaGenerator) -> Arg {
        Arg::JsonBody(generator.subschema_for::<T>())
    }
}

impl<T: JsonSchema> ArgInfo for Query<T> {
    fn info(generator: &mut SchemaGenerator) -> Arg {
        // Inline (not a $ref) so each field becomes a query parameter.
        Arg::Query(T::json_schema(generator))
    }
}

impl<T: JsonSchema> ArgInfo for Option<Query<T>> {
    fn info(generator: &mut SchemaGenerator) -> Arg {
        Arg::Query(T::json_schema(generator))
    }
}

impl ArgInfo for Multipart {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::Multipart
    }
}

impl ArgInfo for Bytes {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::RawBody("application/octet-stream")
    }
}

impl ArgInfo for String {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::RawBody("text/plain")
    }
}

impl ArgInfo for Request {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::RawBody("application/octet-stream")
    }
}

impl ArgInfo for WebSocketUpgrade {
    fn info(_: &mut SchemaGenerator) -> Arg {
        Arg::WebSocket
    }
}

#[allow(dead_code)]
fn _assert_state_is_used(_: State<AppState>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::scope;

    #[test]
    fn scope_requirements_are_read_from_the_type() {
        assert_eq!(required_scopes::<scope::AppsRead>(), vec![Scope::AppsRead]);
        let mut both = required_scopes::<(scope::AppsWrite, scope::EnvReadValues)>();
        both.sort_by_key(|s| s.as_str());
        let mut expected = vec![Scope::AppsWrite, Scope::EnvReadValues];
        expected.sort_by_key(|s| s.as_str());
        assert_eq!(both, expected);
    }
}
