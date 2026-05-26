//! Identity HTTP handlers for passkey registration and authentication

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use elastos_identity::{
    AuthenticationOutcome, AuthenticationResponse, CreationOptions, IdentityManager,
    RegistrationOutcome, RegistrationResponse, RequestOptions, StoredCredential,
};
use elastos_runtime::auth::{
    AuthSessionGrantV1, PasskeyWebAuthnBinding, ProofBinding, RuntimeAuditEventV1,
};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::time::SecureTimestamp;
use elastos_runtime::session::{Session, SessionRegistry};
use rand::RngCore;

const PASSKEY_AUTH_SESSION_TTL_SECS: u64 = 12 * 60 * 60;

/// Shared state for identity endpoints
#[derive(Clone)]
pub struct IdentityState {
    pub manager: Arc<tokio::sync::Mutex<IdentityManager>>,
    pub session_registry: Arc<SessionRegistry>,
    pub audit_log: Option<Arc<AuditLog>>,
    pub data_dir: PathBuf,
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
    principal_id: String,
    proof_binding_id: String,
    session_id: String,
    expires_at: u64,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebAuthnRp {
    pub id: String,
    pub origin: String,
}

/// Derive WebAuthn RP ID and origin from the request.
///
/// Uses the browser-supplied page origin when available. If no page-origin
/// header is present, derives the same-origin authority from `Host`.
/// This is critical because the browser page may be on port 4100 while the API
/// is on port 3000; WebAuthn origin must match the page, not the API.
pub(crate) fn derive_rp(headers: &HeaderMap) -> anyhow::Result<WebAuthnRp> {
    // Prefer Origin header (e.g., "https://localhost:4100").
    // If the browser supplies it, treat malformed or insecure values as
    // authority failures instead of falling back to Host.
    if let Some(origin) = header_value(headers, "origin")? {
        return rp_from_url(origin, "Origin", true);
    }

    // Referer is accepted only when Origin is absent. If present, it must parse
    // and be a secure browser origin.
    if let Some(referer) = header_value(headers, "referer")? {
        return rp_from_url(referer, "Referer", false);
    }

    // Host is only used for same-origin requests that have no browser page
    // origin headers.
    let host = header_value(headers, "host")?.unwrap_or("localhost");
    rp_from_host(host)
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> anyhow::Result<Option<&'a str>> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| anyhow::anyhow!("invalid {} header", name))
        })
        .transpose()
}

fn rp_from_url(
    value: &str,
    header_name: &str,
    require_origin_only: bool,
) -> anyhow::Result<WebAuthnRp> {
    let url = url::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("invalid WebAuthn {} header", header_name))?;
    let scheme = url.scheme();
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("WebAuthn {} origin missing host", header_name))?;
    if require_origin_only
        && (url.path() != "/" || url.query().is_some() || url.fragment().is_some())
    {
        anyhow::bail!("WebAuthn Origin header must be an origin, not a URL path");
    }
    if !is_allowed_webauthn_origin(scheme, host) {
        anyhow::bail!(
            "WebAuthn {} origin must be https or loopback http",
            header_name
        );
    }
    Ok(WebAuthnRp {
        id: host.to_ascii_lowercase(),
        origin: url.origin().ascii_serialization(),
    })
}

fn rp_from_host(host: &str) -> anyhow::Result<WebAuthnRp> {
    let authority = normalize_authority(host)?;
    let authority_url = url::Url::parse(&format!("http://{authority}/"))
        .map_err(|_| anyhow::anyhow!("invalid WebAuthn host authority"))?;
    let host_name = authority_url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("invalid WebAuthn host authority"))?
        .to_string();
    let scheme = if is_loopback_host(&host_name) {
        "http"
    } else {
        "https"
    };
    let origin_url = url::Url::parse(&format!("{scheme}://{authority}/"))
        .map_err(|_| anyhow::anyhow!("invalid WebAuthn host authority"))?;
    Ok(WebAuthnRp {
        id: host_name.to_ascii_lowercase(),
        origin: origin_url.origin().ascii_serialization(),
    })
}

fn normalize_authority(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty()
        || value.contains('/')
        || value.contains('@')
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid WebAuthn host authority");
    }
    Ok(value.to_string())
}

fn is_allowed_webauthn_origin(scheme: &str, host: &str) -> bool {
    scheme == "https" || (scheme == "http" && is_loopback_host(host))
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || host.starts_with("127.")
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
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let mut manager = state.manager.lock().await;
    require_existing_registration_authority(&manager, &session)?;
    require_guest_registration_policy(&state.data_dir, manager.status().registered)?;
    match manager.begin_registration(&session.token, &rp.id) {
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
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let mut manager = state.manager.lock().await;
    require_existing_registration_authority(&manager, &session)?;
    require_guest_registration_policy(&state.data_dir, manager.status().registered)?;
    match manager.complete_registration(&session.token, &response, &rp.id, &rp.origin) {
        Ok(outcome) => {
            let user_id = outcome.user_id.clone();
            let grant = match issue_passkey_session_grant_for_registration(&state, &outcome) {
                Ok(grant) => grant,
                Err(err) => return Err(error_response(StatusCode::BAD_REQUEST, &err.to_string())),
            };
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

            Ok(Json(user_id_response(user_id, grant)))
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
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let mut manager = state.manager.lock().await;
    match manager.begin_authentication(&session.token, &rp.id) {
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
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let mut manager = state.manager.lock().await;
    match manager.complete_authentication(&session.token, &response, &rp.id, &rp.origin) {
        Ok(outcome) => {
            let user_id = outcome.user_id.clone();
            let grant = match issue_passkey_session_grant_for_authentication(&state, &outcome) {
                Ok(grant) => grant,
                Err(err) => return Err(error_response(StatusCode::BAD_REQUEST, &err.to_string())),
            };
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

            Ok(Json(user_id_response(user_id, grant)))
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

fn require_existing_registration_authority(
    manager: &IdentityManager,
    session: &Session,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if manager.status().registered && session.owner.is_none() {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "existing passkey registration requires an authenticated session",
        ));
    }
    Ok(())
}

fn require_guest_registration_policy(
    data_dir: &Path,
    registered: bool,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if !registered
        && crate::auth::active_passkey_principal_count(data_dir)
            .map_err(|err| error_response(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()))?
            == 0
    {
        return Ok(());
    }
    if crate::auth::guest_registration_enabled(data_dir)
        .map_err(|err| error_response(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()))?
    {
        return Ok(());
    }
    Err(error_response(
        StatusCode::FORBIDDEN,
        "guest passkey registration is disabled",
    ))
}

fn issue_passkey_session_grant_for_registration(
    state: &IdentityState,
    outcome: &RegistrationOutcome,
) -> anyhow::Result<AuthSessionGrantV1> {
    issue_passkey_session_grant(
        state,
        &outcome.user_id,
        &outcome.credential,
        &outcome.origin,
        outcome.user_verified,
        "passkey registration verified and session granted",
    )
}

fn issue_passkey_session_grant_for_authentication(
    state: &IdentityState,
    outcome: &AuthenticationOutcome,
) -> anyhow::Result<AuthSessionGrantV1> {
    issue_passkey_session_grant(
        state,
        &outcome.user_id,
        &outcome.credential,
        &outcome.origin,
        outcome.user_verified,
        "passkey authentication verified and session granted",
    )
}

fn issue_passkey_session_grant(
    state: &IdentityState,
    _user_id: &str,
    credential: &StoredCredential,
    origin: &str,
    user_verified: bool,
    reason: &str,
) -> anyhow::Result<AuthSessionGrantV1> {
    let now = crate::auth::now_ts();
    let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
        credential_id: credential.credential_id.clone(),
        public_key: credential.public_key.clone(),
        sign_count: credential.sign_count,
        user_verified,
        origin: origin.to_string(),
        rp_id: credential.rp_id.clone(),
        created_at: now,
        last_used_at: now,
        revoked_at: None,
    });
    let role = if crate::auth::active_passkey_principal_count(&state.data_dir)? == 0 {
        crate::auth::RuntimePrincipalRole::Admin
    } else {
        crate::auth::RuntimePrincipalRole::Guest
    };
    let principal_id =
        crate::auth::passkey_credential_principal_id(&credential.rp_id, &credential.credential_id)?;
    let principal = crate::auth::upsert_principal_for_binding_as_role(
        &state.data_dir,
        binding,
        principal_id,
        role,
        now,
    )?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", random_hex(16)),
        session_id: format!("auth:{}", random_hex(16)),
        principal_id: principal.principal_id.clone(),
        proof_binding_id: principal.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now.saturating_add(PASSKEY_AUTH_SESSION_TTL_SECS),
        apps: vec![
            crate::api::gateway::HOME_CAPSULE_ID.to_string(),
            "system".to_string(),
        ],
    };
    crate::auth::store_session_grant(&state.data_dir, grant.clone())?;
    crate::auth::append_audit_event(
        &state.data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!("audit:{}", random_hex(16)),
            event_type: "auth.session.granted".to_string(),
            principal_id: Some(grant.principal_id.clone()),
            proof_binding_id: Some(grant.proof_binding_id.clone()),
            session_id: Some(grant.session_id.clone()),
            challenge_id: None,
            capsule_id: None,
            result: "ok".to_string(),
            reason: reason.to_string(),
            occurred_at: now,
            signer_did: None,
            signature: None,
        },
    )?;
    Ok(grant)
}

fn user_id_response(user_id: String, grant: AuthSessionGrantV1) -> UserIdResponse {
    UserIdResponse {
        user_id,
        principal_id: grant.principal_id,
        proof_binding_id: grant.proof_binding_id,
        session_id: grant.session_id,
        expires_at: grant.expires_at,
    }
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn rp_origin_uses_http_for_localhost_host_authority() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:3000"));

        let rp = derive_rp(&headers).unwrap();

        assert_eq!(rp.id, "localhost");
        assert_eq!(rp.origin, "http://localhost:3000");
    }

    #[test]
    fn rp_origin_uses_https_for_public_host_authority() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("elastos.elacitylabs.com"));

        let rp = derive_rp(&headers).unwrap();

        assert_eq!(rp.id, "elastos.elacitylabs.com");
        assert_eq!(rp.origin, "https://elastos.elacitylabs.com");
    }

    #[test]
    fn rp_origin_prefers_browser_origin_header() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:3000"));
        headers.insert(
            "origin",
            HeaderValue::from_static("https://elastos.elacitylabs.com"),
        );

        let rp = derive_rp(&headers).unwrap();

        assert_eq!(rp.id, "elastos.elacitylabs.com");
        assert_eq!(rp.origin, "https://elastos.elacitylabs.com");
    }

    #[test]
    fn rp_origin_uses_referer_origin_when_origin_is_missing() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:3000"));
        headers.insert(
            "referer",
            HeaderValue::from_static("http://localhost:4100/apps/home/"),
        );

        let rp = derive_rp(&headers).unwrap();

        assert_eq!(rp.id, "localhost");
        assert_eq!(rp.origin, "http://localhost:4100");
    }

    #[test]
    fn rp_origin_rejects_insecure_public_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("http://elastos.elacitylabs.com"),
        );

        let err = derive_rp(&headers).unwrap_err().to_string();

        assert!(err.contains("https or loopback http"));
    }

    #[test]
    fn rp_origin_rejects_malformed_origin_instead_of_using_host() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("elastos.elacitylabs.com"));
        headers.insert("origin", HeaderValue::from_static("not a url"));

        let err = derive_rp(&headers).unwrap_err().to_string();

        assert!(err.contains("invalid WebAuthn Origin header"));
    }

    #[test]
    fn rp_origin_rejects_malformed_referer_instead_of_using_host() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("elastos.elacitylabs.com"));
        headers.insert("referer", HeaderValue::from_static("not a url"));

        let err = derive_rp(&headers).unwrap_err().to_string();

        assert!(err.contains("invalid WebAuthn Referer header"));
    }

    #[test]
    fn rp_origin_rejects_origin_header_with_path() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://elastos.elacitylabs.com/apps/home/"),
        );

        let err = derive_rp(&headers).unwrap_err().to_string();

        assert!(err.contains("must be an origin"));
    }

    #[test]
    fn rp_origin_rejects_path_like_host_authority() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "host",
            HeaderValue::from_static("elastos.elacitylabs.com/apps/home"),
        );

        let err = derive_rp(&headers).unwrap_err().to_string();

        assert!(err.contains("invalid WebAuthn host authority"));
    }

    #[test]
    fn direct_identity_registration_respects_guest_gate() {
        let data_dir = tempfile::tempdir().unwrap();

        assert!(require_guest_registration_policy(data_dir.path(), false).is_ok());

        let (status, Json(body)) =
            require_guest_registration_policy(data_dir.path(), true).unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.error, "guest passkey registration is disabled");

        crate::auth::set_guest_registration_enabled(data_dir.path(), true, 10).unwrap();
        assert!(require_guest_registration_policy(data_dir.path(), true).is_ok());
    }
}
