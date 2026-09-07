//! Identity HTTP handlers for passkey registration and authentication

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use elastos_identity::{
    AuthenticationOutcome, AuthenticationResponse, IdentityManager, RegistrationOutcome,
    RegistrationResponse, RequestOptions, StoredCredential,
};
use elastos_runtime::auth::AuthSessionGrantV1;
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::time::SecureTimestamp;
use elastos_runtime::session::{Session, SessionRegistry};

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
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    mut headers: HeaderMap,
    session: axum::Extension<Session>,
) -> Result<axum::response::Response, (StatusCode, Json<ErrorResponse>)> {
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let loopback =
        crate::api::auth_gateway::local_first_owner_registration(&headers, peer.map(|peer| peer.0));
    crate::api::auth_gateway::prepare_owner_claim(&mut headers, loopback).map_err(owner_error)?;
    let claim = crate::api::auth_gateway::owner_claim(&headers)
        .map_err(owner_error)?
        .unwrap_or_default();
    let mut manager = state.manager.lock().await;
    let owner_ceremony = format!("owner:{}", crate::auth::random_secret_hex());
    if let Some((ceremony, options)) = crate::auth::begin_owner_enrollment(
        &state.data_dir,
        &mut manager,
        &crate::auth::OwnerAdmission {
            origin: &rp.origin,
            rp_id: &rp.id,
            claimant: &claim,
            loopback,
        },
        &owner_ceremony,
        crate::auth::now_ts(),
    )
    .map_err(owner_error)?
    {
        let mut response = crate::api::auth_gateway::with_owner_claim_cookie(
            &headers,
            Json(options).into_response(),
        );
        response.headers_mut().insert(
            "x-elastos-owner-ceremony",
            ceremony
                .parse()
                .map_err(|_| owner_error(crate::auth::OwnerEnrollmentDenied.into()))?,
        );
        return Ok(response);
    }
    require_existing_registration_authority(&manager, &session)?;
    require_guest_registration_policy(&state.data_dir)?;
    match manager.begin_registration(&session.token, &rp.id, &rp.origin) {
        Ok(options) => Ok(Json(options).into_response()),
        Err(e) => Err(error_response(StatusCode::BAD_REQUEST, &e.to_string())),
    }
}

/// POST /api/identity/register/complete
pub async fn register_complete(
    State(state): State<IdentityState>,
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    session: axum::Extension<Session>,
    Json(response): Json<Option<RegistrationResponse>>,
) -> Result<Json<UserIdResponse>, (StatusCode, Json<ErrorResponse>)> {
    let rp =
        derive_rp(&headers).map_err(|e| error_response(StatusCode::FORBIDDEN, &e.to_string()))?;
    let loopback =
        crate::api::auth_gateway::local_first_owner_registration(&headers, peer.map(|peer| peer.0));
    let claim = crate::api::auth_gateway::owner_claim(&headers)
        .map_err(owner_error)?
        .unwrap_or_default();
    let ceremony = match headers.get("x-elastos-owner-ceremony") {
        Some(value) => value
            .to_str()
            .map_err(|_| owner_error(crate::auth::OwnerEnrollmentDenied.into()))?,
        None => "",
    };
    let mut manager = state.manager.lock().await;
    if let Some(grant) = crate::auth::complete_owner_enrollment(
        &state.data_dir,
        &mut manager,
        &crate::auth::OwnerAdmission {
            origin: &rp.origin,
            rp_id: &rp.id,
            claimant: &claim,
            loopback,
        },
        ceremony,
        response.as_ref(),
        None,
        crate::auth::now_ts(),
    )
    .map_err(owner_error)?
    {
        let user_id = manager
            .status()
            .user_id
            .ok_or_else(|| owner_error(crate::auth::OwnerEnrollmentDenied.into()))?;
        drop(manager);
        state
            .session_registry
            .get_session_mut(&session.token, |session| session.set_owner(user_id.clone()))
            .await;
        return Ok(Json(user_id_response(user_id, grant)));
    }
    require_existing_registration_authority(&manager, &session)?;
    require_guest_registration_policy(&state.data_dir)?;
    let response = response
        .as_ref()
        .ok_or_else(|| owner_error(crate::auth::OwnerEnrollmentDenied.into()))?;
    match manager.complete_registration(&session.token, response, &rp.id, &rp.origin) {
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
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if crate::auth::guest_registration_enabled(data_dir)
        .map_err(|err| error_response(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()))?
        && crate::auth::active_admin_passkey_principal_count(data_dir)
            .map_err(|err| error_response(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()))?
            > 0
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
    crate::auth::grant_passkey_session(
        &state.data_dir,
        crate::auth::PasskeySessionRequest {
            credential: &outcome.credential,
            origin: &outcome.origin,
            user_verified: outcome.user_verified,
            display_name: None,
            reason: "passkey registration verified and session granted",
            profile_display_name: None,
            purpose: crate::auth::PasskeySessionPurpose::GuestRegistration,
        },
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
    crate::auth::grant_passkey_session(
        &state.data_dir,
        crate::auth::PasskeySessionRequest {
            credential,
            origin,
            user_verified,
            display_name: None,
            reason,
            profile_display_name: None,
            purpose: crate::auth::PasskeySessionPurpose::SignIn,
        },
    )
}

fn owner_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let status = if error.is::<crate::auth::OwnerEnrollmentDenied>() {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    error_response(status, &error.to_string())
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

        let (status, Json(body)) = require_guest_registration_policy(data_dir.path()).unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.error, "guest passkey registration is disabled");

        crate::auth::set_guest_registration_enabled(data_dir.path(), true, 10).unwrap();
        assert_eq!(
            require_guest_registration_policy(data_dir.path())
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: "admin-credential".into(),
                public_key: "admin-public-key".into(),
                sign_count: 0,
                user_verified: true,
                origin: "https://home.example".into(),
                rp_id: "home.example".into(),
                created_at: 10,
                last_used_at: 10,
                revoked_at: None,
            },
        );
        crate::auth::upsert_principal_for_binding_as_role(
            data_dir.path(),
            binding,
            crate::auth::passkey_credential_principal_id("home.example", "admin-credential")
                .unwrap(),
            crate::auth::RuntimePrincipalRole::Admin,
            10,
        )
        .unwrap();
        assert!(require_guest_registration_policy(data_dir.path()).is_ok());
    }
}
