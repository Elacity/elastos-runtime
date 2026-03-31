//! Authentication middleware for the HTTP API
//!
//! Extracts and validates session tokens from Authorization headers.
//! Also provides per-session rate limiting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Body,
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::Mutex;

use elastos_runtime::session::{Session, SessionRegistry};

/// State shared with middleware
#[derive(Clone)]
pub struct ApiState {
    pub session_registry: Arc<SessionRegistry>,
}

/// Extract bearer token from Authorization header
fn extract_bearer_token(req: &Request<Body>) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.to_string())
}

/// Authentication middleware - validates session token
///
/// On success, inserts the Session into request extensions.
/// On failure, returns 401 Unauthorized.
pub async fn auth_middleware(
    State(state): State<ApiState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let token = match extract_bearer_token(&req) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Missing Authorization header. Expected: Bearer <token>",
            )
                .into_response();
        }
    };

    // Validate token and get session
    let session = match state.session_registry.validate_token(&token).await {
        Some(s) => s,
        None => {
            return (StatusCode::UNAUTHORIZED, "Invalid or expired session token").into_response();
        }
    };

    // Update last activity
    state.session_registry.touch_session(&token).await;

    // Insert session into request extensions for handlers to use
    req.extensions_mut().insert(session);

    next.run(req).await
}

/// Shell-only middleware - requires session to be a shell
///
/// Must be used AFTER auth_middleware (expects Session in extensions).
pub async fn shell_only_middleware(req: Request<Body>, next: Next) -> Response {
    let session = match req.extensions().get::<Session>() {
        Some(s) => s,
        None => {
            // This shouldn't happen if auth_middleware ran first
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session not found in request extensions",
            )
                .into_response();
        }
    };

    if !session.is_shell() {
        return (
            StatusCode::FORBIDDEN,
            "This endpoint requires shell privileges",
        )
            .into_response();
    }

    next.run(req).await
}

/// Extension trait for extracting session from request
pub trait SessionExt {
    fn session(&self) -> Option<&Session>;
}

impl<B> SessionExt for Request<B> {
    fn session(&self) -> Option<&Session> {
        self.extensions().get::<Session>()
    }
}

// === Rate Limiting ===

/// Per-session token bucket state.
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Simple per-session token-bucket rate limiter.
///
/// Each session gets `rate` tokens per second (burst = `rate`).
/// When a session runs out of tokens the request is rejected with 429.
pub struct RateLimiter {
    /// Max tokens per second per session.
    rate: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// Create a rate limiter that allows `rate` requests per second per session.
    pub fn new(rate: f64) -> Self {
        Self {
            rate,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Try to consume one token for the given session.
    /// Returns `true` if allowed, `false` if rate-limited.
    pub async fn check(&self, session_id: &str) -> bool {
        let mut buckets = self.buckets.lock().await;
        let now = Instant::now();
        let bucket = buckets.entry(session_id.to_string()).or_insert(Bucket {
            tokens: self.rate,
            last_refill: now,
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.rate);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// State for rate-limited routes.
#[derive(Clone)]
pub struct RateLimitState {
    pub session_registry: Arc<SessionRegistry>,
    pub rate_limiter: Arc<RateLimiter>,
}

/// Rate-limiting middleware — must be used AFTER auth_middleware.
///
/// Returns 429 Too Many Requests when the per-session bucket is empty.
pub async fn rate_limit_middleware(
    State(state): State<RateLimitState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let session_id = req
        .extensions()
        .get::<Session>()
        .map(|s| s.id.as_str().to_string())
        .unwrap_or_default();

    if !state.rate_limiter.check(&session_id).await {
        return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
    }

    next.run(req).await
}

// === Identity Challenge Limiter ===

/// Limits the number of concurrent pending identity challenges per session.
///
/// Prevents resource exhaustion via unbounded challenge creation.
pub struct ChallengeLimiter {
    max_pending: usize,
    pending: Mutex<HashMap<String, usize>>,
}

impl ChallengeLimiter {
    pub fn new(max_pending: usize) -> Self {
        Self {
            max_pending,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Try to acquire a challenge slot. Returns `true` if allowed.
    pub async fn try_acquire(&self, session_id: &str) -> bool {
        let mut pending = self.pending.lock().await;
        let count = pending.entry(session_id.to_string()).or_insert(0);
        if *count >= self.max_pending {
            false
        } else {
            *count += 1;
            true
        }
    }

    /// Release a challenge slot (call after challenge completes or expires).
    pub async fn release(&self, session_id: &str) {
        let mut pending = self.pending.lock().await;
        if let Some(count) = pending.get_mut(session_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pending.remove(session_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[test]
    fn test_extract_bearer_token() {
        // Valid token
        let req = Request::builder()
            .header("Authorization", "Bearer my-token-123")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), Some("my-token-123".to_string()));

        // Missing header
        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(extract_bearer_token(&req), None);

        // Wrong scheme
        let req = Request::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), None);

        // Malformed (no space after Bearer)
        let req = Request::builder()
            .header("Authorization", "Bearer")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), None);

        // Empty token after Bearer (has space but no token)
        let req = Request::builder()
            .header("Authorization", "Bearer ")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), Some("".to_string()));
    }

    #[tokio::test]
    async fn test_rate_limiter_allows_burst() {
        let limiter = RateLimiter::new(5.0);
        // Should allow a burst of 5 requests
        for _ in 0..5 {
            assert!(limiter.check("sess-1").await);
        }
        // 6th should be rejected
        assert!(!limiter.check("sess-1").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_independent_sessions() {
        let limiter = RateLimiter::new(2.0);
        assert!(limiter.check("sess-a").await);
        assert!(limiter.check("sess-a").await);
        assert!(!limiter.check("sess-a").await);
        // Different session should still work
        assert!(limiter.check("sess-b").await);
    }

    #[tokio::test]
    async fn test_challenge_limiter() {
        let limiter = ChallengeLimiter::new(2);
        assert!(limiter.try_acquire("s1").await);
        assert!(limiter.try_acquire("s1").await);
        assert!(!limiter.try_acquire("s1").await); // at limit
        limiter.release("s1").await;
        assert!(limiter.try_acquire("s1").await); // slot freed
    }
}
