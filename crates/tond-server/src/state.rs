// state.rs — shared application state injected into every Axum handler
//
// AppState is cloned cheaply into every request handler via Axum's
// State<AppState> extractor. All fields are internally Arc-backed
// so Clone is O(1).
//
//   db           — SQLite connection pool. Path from TOND_DB_PATH
//                  (default: /opt/tond/data/tond.db). Created with ?mode=rwc so
//                  the file is created on first run, then kept at 0660 so
//                  tond-proxy (group tond-proxy) can share it.
//
//   docker       — Bollard Docker client connected via the local Unix
//                  socket. Verified at startup by calling docker.version().
//
//   rate_limiter — Per-IP token bucket. Config: 60 token burst, 10 tok/s
//                  refill. Shared across all handler clones via Arc<Mutex>.
//
//   settings     — Cached server settings (dashboard_domain).
//                  Loaded from the DB at startup; writes go to both the in-memory
//                  cache and the DB so the proxy can still read the DB on restart.

use bollard::Docker;
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tond_common::ServerSettings;

use crate::{auth::RateLimiter, db};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub docker: Docker,
    pub rate_limiter: RateLimiter,
    pub settings: Arc<RwLock<ServerSettings>>,
}

impl AppState {
    pub async fn new() -> anyhow::Result<Self> {
        let db_path =
            std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&db_path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let db = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", db_path)).await?;
        db::share_with_proxy(Path::new(&db_path));

        let docker = Docker::connect_with_local_defaults()?;

        // Verify Docker connectivity
        let info = docker.version().await?;
        tracing::info!(
            "Connected to Docker engine: {}",
            info.version.unwrap_or_default()
        );
        tracing::info!("Database: {}", db_path);

        // Rate limiter: 60 requests per IP, refill 10/sec (burst of 60, sustained 10/sec)
        let rate_limiter = RateLimiter::new(60, 10.0);

        // Settings are populated after migrations run (see main.rs).
        let settings = Arc::new(RwLock::new(ServerSettings::default()));

        Ok(Self {
            db,
            docker,
            rate_limiter,
            settings,
        })
    }
}
