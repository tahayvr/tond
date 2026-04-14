// auth/rate_limit.rs — per-IP token bucket rate limiter and Axum extractor
//
// Two types are defined here:
//
//   RateLimiter
//     The actual limiter. Holds a HashMap<IP, Bucket> behind an
//     Arc<Mutex> so it can be shared across all request handler clones
//     via AppState. Each bucket tracks available tokens and the timestamp
//     of the last refill. On every check():
//       - Tokens are replenished based on elapsed time * refill_rate
//       - If ≥1 token is available, one is consumed and the request is
//         allowed (returns true)
//       - Otherwise the request is rejected (returns false)
//     Default config: capacity=60 (burst), refill_rate=10.0 tok/s.
//
//   RateLimit (Axum extractor)
//     Zero-sized guard, analogous to RequireAuth. Adding `_rate: RateLimit`
//     to a handler signature runs this check first. The client IP is
//     extracted from X-Forwarded-For (first value) then X-Real-IP, falling
//     back to "unknown". If the limiter rejects the IP, Axum returns 429
//     before the handler runs.

use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tond_common::ApiResponse;

use crate::state::AppState;

/// Simple per-IP token bucket rate limiter.
///
/// Each IP gets `capacity` tokens, refilled at `refill_rate` tokens/second.
/// When tokens run out, requests are rejected with 429.
#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<String, Bucket>>>,
    capacity: u32,
    refill_rate: f64, // tokens per second
}

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `capacity`: max burst size per IP
    /// - `refill_rate`: tokens restored per second
    pub fn new(capacity: u32, refill_rate: f64) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            capacity,
            refill_rate,
        }
    }

    /// Try to consume one token for the given IP. Returns true if allowed.
    pub async fn check(&self, ip: &str) -> bool {
        let mut state = self.state.lock().await;
        let now = Instant::now();

        let bucket = state.entry(ip.to_string()).or_insert(Bucket {
            tokens: self.capacity as f64,
            last_refill: now,
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_rate).min(self.capacity as f64);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Axum extractor that enforces rate limiting per client IP.
/// Must be placed in AppState so it can be accessed.
pub struct RateLimit;

impl FromRequestParts<AppState> for RateLimit {
    type Rejection = (StatusCode, Json<ApiResponse<()>>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Extract client IP from headers or connection info
        let ip = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                parts
                    .headers
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());

        if state.rate_limiter.check(&ip).await {
            Ok(RateLimit)
        } else {
            Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(ApiResponse::err("Rate limit exceeded. Try again later.")),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{Duration, sleep};

    use super::*;

    #[tokio::test]
    async fn rate_limiter_allows_up_to_capacity_then_blocks() {
        let limiter = RateLimiter::new(2, 0.0);

        assert!(
            limiter.check("203.0.113.1").await,
            "the first request within capacity should be allowed"
        );
        assert!(
            limiter.check("203.0.113.1").await,
            "the second request within capacity should be allowed"
        );
        assert!(
            !limiter.check("203.0.113.1").await,
            "requests beyond the configured token capacity should be rejected"
        );
    }

    #[tokio::test]
    async fn rate_limiter_refills_tokens_over_time() {
        let limiter = RateLimiter::new(1, 100.0);

        assert!(
            limiter.check("203.0.113.1").await,
            "the first request should consume the initial token"
        );
        assert!(
            !limiter.check("203.0.113.1").await,
            "a second immediate request should be rejected before refill"
        );

        sleep(Duration::from_millis(20)).await;

        assert!(
            limiter.check("203.0.113.1").await,
            "the limiter should allow requests again after enough time has passed to refill a token"
        );
    }

    #[tokio::test]
    async fn rate_limiter_tracks_ips_independently() {
        let limiter = RateLimiter::new(1, 0.0);

        assert!(
            limiter.check("203.0.113.1").await,
            "first IP should get its own token bucket"
        );
        assert!(
            limiter.check("198.51.100.5").await,
            "second IP should have an independent token bucket"
        );
        assert!(
            !limiter.check("203.0.113.1").await,
            "exhausting one IP's bucket should not affect the other IP's bucket"
        );
    }
}
