//! Identity HTTP handlers for passkey registration and authentication

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use elastos_identity::{
    AuthenticationResponse, CreationOptions, IdentityManager, RegistrationResponse, RequestOptions,
};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::time::SecureTimestamp;
use elastos_runtime::session::{Session, SessionRegistry};

/// Shared state for identity endpoints
#[derive(Clone)]
pub struct IdentityState {
    pub manager: Arc<tokio::sync::Mutex<IdentityManager>>,
    pub session_registry: Arc<SessionRegistry>,
    pub audit_log: Option<Arc<AuditLog>>,
}

#[derive(Serialize)]
pub struct StatusResponse {
    registered: bool,
    authenticated: bool,
    user_id: Option<String>,
}

#[derive(Serialize)]
pub struct UserIdResponse {
    user_id: String,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    error: String,
}

/// Derive WebAuthn RP ID and origin from the request.
///
/// Uses the `Origin` header (set by browsers on cross-origin requests) to get
/// the actual page origin. Falls back to `Referer`, then `Host`.
/// This is critical because the browser page may be on port 4100 while
/// the API is on port 3000 — WebAuthn origin must match the page, not the API.
fn derive_rp(headers: &HeaderMap) -> (String, String) {
    // Prefer Origin header (e.g., "https://localhost:4100")
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) {
        if let Ok(url) = url::Url::parse(origin) {
            let hostname = url.host_str().unwrap_or("localhost").to_string();
            return (hostname, origin.trim_end_matches('/').to_string());
        }
    }

    // Fallback to Referer header
    if let Some(referer) = headers.get("referer").and_then(|h| h.to_str().ok()) {
        if let Ok(url) = url::Url::parse(referer) {
            let hostname = url.host_str().unwrap_or("localhost").to_string();
            let origin = url.origin().ascii_serialization();
            return (hostname, origin);
        }
    }

    // Last resort: Host header (same-origin requests)
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    let hostname = host.split(':').next().unwrap_or("localhost");
    let rp_origin = format!("https://{}", host);
    (hostname.to_string(), rp_origin)
}

fn error_response(status: StatusCode, msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

/// GET /api/identity/status
pub async fn identity_status(
    State(state): State<IdentityState>,
    session: axum::Extension<Session>,
) -> impl IntoResponse {
    let manager = state.manager.lock().await;
    let mut status = manager.status();
    status.authenticated = session.owner.is_some();
    if status.authenticated {
        status.user_id = session.owner.clone();
    }
    Json(StatusResponse {
        registered: status.registered,
        authenticated: status.authenticated,
        user_id: status.user_id,
    })
}

/// POST /api/identity/register/begin
pub async fn register_begin(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    session: axum::Extension<Session>,
) -> Result<Json<CreationOptions>, (StatusCode, Json<ErrorResponse>)> {
    let (rp_id, _rp_origin) = derive_rp(&headers);
    let mut manager = state.manager.lock().await;
    match manager.begin_registration(&session.token, &rp_id) {
        Ok(options) => Ok(Json(options)),
        Err(e) => Err(error_response(StatusCode::BAD_REQUEST, &e.to_string())),
    }
}

/// POST /api/identity/register/complete
pub async fn register_complete(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    session: axum::Extension<Session>,
    Json(response): Json<RegistrationResponse>,
) -> Result<Json<UserIdResponse>, (StatusCode, Json<ErrorResponse>)> {
    let (rp_id, rp_origin) = derive_rp(&headers);
    let mut manager = state.manager.lock().await;
    match manager.complete_registration(&session.token, &response, &rp_id, &rp_origin) {
        Ok(user_id) => {
            drop(manager);
            state
                .session_registry
                .get_session_mut(&session.token, |s| {
                    s.set_owner(user_id.clone());
                })
                .await;

            if let Some(ref audit) = state.audit_log {
                audit.emit(
                    elastos_runtime::primitives::audit::AuditEvent::IdentityRegistered {
                        timestamp: SecureTimestamp::now(),
                        user_id: user_id.clone(),
                        method: "passkey".to_string(),
                    },
                );
            }

            Ok(Json(UserIdResponse { user_id }))
        }
        Err(e) => Err(error_response(StatusCode::BAD_REQUEST, &e.to_string())),
    }
}

/// POST /api/identity/authenticate/begin
pub async fn authenticate_begin(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    session: axum::Extension<Session>,
) -> Result<Json<RequestOptions>, (StatusCode, Json<ErrorResponse>)> {
    let (rp_id, _rp_origin) = derive_rp(&headers);
    let mut manager = state.manager.lock().await;
    match manager.begin_authentication(&session.token, &rp_id) {
        Ok(options) => Ok(Json(options)),
        Err(e) => Err(error_response(StatusCode::BAD_REQUEST, &e.to_string())),
    }
}

/// POST /api/identity/authenticate/complete
pub async fn authenticate_complete(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    session: axum::Extension<Session>,
    Json(response): Json<AuthenticationResponse>,
) -> Result<Json<UserIdResponse>, (StatusCode, Json<ErrorResponse>)> {
    let (rp_id, rp_origin) = derive_rp(&headers);
    let mut manager = state.manager.lock().await;
    match manager.complete_authentication(&session.token, &response, &rp_id, &rp_origin) {
        Ok(user_id) => {
            drop(manager);
            state
                .session_registry
                .get_session_mut(&session.token, |s| {
                    s.set_owner(user_id.clone());
                })
                .await;

            if let Some(ref audit) = state.audit_log {
                audit.emit(
                    elastos_runtime::primitives::audit::AuditEvent::AuthAttempt {
                        timestamp: SecureTimestamp::now(),
                        identity: user_id.clone(),
                        success: true,
                        method: "passkey".to_string(),
                    },
                );
            }

            Ok(Json(UserIdResponse { user_id }))
        }
        Err(e) => {
            if let Some(ref audit) = state.audit_log {
                audit.emit(
                    elastos_runtime::primitives::audit::AuditEvent::AuthAttempt {
                        timestamp: SecureTimestamp::now(),
                        identity: "unknown".to_string(),
                        success: false,
                        method: "passkey".to_string(),
                    },
                );
            }
            Err(error_response(StatusCode::UNAUTHORIZED, &e.to_string()))
        }
    }
}
