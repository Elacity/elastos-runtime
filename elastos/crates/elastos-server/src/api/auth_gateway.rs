//! Browser-host adapter for runtime proof-bound authentication.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{
    header::{CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE},
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use elastos_identity::{
    AuthenticationResponse, CreationOptions, RegistrationResponse, RequestOptions, StoredCredential,
};
use elastos_runtime::auth::{
    ethereum_signed_message_hash, normalize_evm_address, validate_evm_address, AuthChallengeV1,
    AuthSessionGrantV1, DidRecoveryProofV1, PasskeyWebAuthnBinding, PrincipalRootCryptoProfileV1,
    PrincipalRootProtectionV1, PrincipalRootProtectorEnvelopeV1, PrincipalRootProtectorKind,
    PrincipalRootProtectorV1, PrincipalRootRecoveryArchiveV1, PrincipalRootRecoveryStatusV1,
    ProofBinding, ProofBindingKind, RecoveryKitV1, RuntimeAuditEventV1,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::gateway::{
    consume_fresh_passkey_home_token, home_session_cookie_header_for_token,
    is_wallet_connector_capsule_id, issue_home_launch_token_for_auth_grant,
    login_avatar_bytes_for_credential, login_avatar_cid_for_principal, GatewayState,
    HOME_CAPSULE_ID, WALLET_LINK_CAPSULE_IDS,
};

const AUTH_SESSION_TTL_SECS: u64 = 12 * 60 * 60;
const RECOVERY_DESCRIPTOR_SCHEMA: &str = "elastos.principal.root-descriptor/v1";
const FULL_RECOVERY_BUNDLE_SCHEMA: &str = "elastos.full-recovery-bundle/v1";
const FULL_RECOVERY_BUNDLE_EXPORT_REQUEST_SCHEMA: &str =
    "elastos.full-recovery-bundle.export.request/v1";
const FULL_RECOVERY_BUNDLE_IMPORT_REQUEST_SCHEMA: &str =
    "elastos.full-recovery-bundle.import.request/v1";
const FULL_RECOVERY_BUNDLE_PACKAGE_SCHEMA: &str = "elastos.full-recovery-bundle.package/v1";
const FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA: &str =
    "elastos.full-recovery-bundle.import.response/v1";
const FULL_RECOVERY_BUNDLE_AAD_DOMAIN: &str = "elastos.full-recovery-bundle.package.v1";
const FULL_RECOVERY_BUNDLE_KDF_PARAMS: &str = "m=19456,t=2,p=1,len=32";
const WALLET_RECOVERY_KEY_SCHEMA: &str = "elastos.wallet.recovery-key/v1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvmChallengeRequest {
    pub address: String,
    pub chain_id: u64,
}

#[derive(Debug, Serialize)]
pub struct EvmChallengeResponse {
    pub schema: String,
    pub challenge_id: String,
    pub message: String,
    pub expires_at: u64,
    pub resources: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvmVerifyRequest {
    pub message: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct EvmVerifyResponse {
    pub schema: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub session_id: String,
    pub expires_at: u64,
    pub app_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChallengeRequest {
    pub address: String,
    #[serde(default = "default_btc_network")]
    pub network: String,
}

#[derive(Debug, Serialize)]
pub struct BtcChallengeResponse {
    pub schema: String,
    pub challenge_id: String,
    pub message: String,
    pub expires_at: u64,
    pub network: String,
    pub address: String,
    pub resources: Vec<String>,
    pub proof_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BtcVerifyRequest {
    pub message: String,
    pub signature: String,
    #[serde(default)]
    pub signature_type: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BtcVerifyResponse {
    pub schema: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub session_id: String,
    pub expires_at: u64,
    pub app_token: String,
}

fn default_btc_network() -> String {
    "bitcoin".to_string()
}

#[derive(Debug, Serialize)]
pub struct AuthRevokeResponse {
    pub status: String,
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct PasskeyStatusResponse {
    pub registered: bool,
    pub guest_registration_enabled: bool,
    /// Minimal local account directory for the unsigned Home front door.
    /// Never includes principal roots, grants, or recovery material.
    #[serde(default)]
    pub accounts: Vec<PasskeyLoginAccount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PasskeyLoginAccount {
    pub principal_id: String,
    pub display_name: String,
    pub role: String,
    pub credential_id: String,
    pub last_used_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_cid: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PasskeyAccountAvatarQuery {
    pub credential_id: String,
}

#[derive(Debug, Serialize)]
pub struct PasskeyListResponse {
    pub schema: String,
    pub passkeys: Vec<PasskeyView>,
}

#[derive(Debug, Serialize)]
pub struct PasskeyView {
    pub proof_binding_id: String,
    pub principal_id: String,
    pub display_name: String,
    pub role: String,
    pub localhost_root: String,
    pub rp_id: String,
    pub sign_count: u32,
    pub created_at: u64,
    pub last_used_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
    pub current: bool,
}

#[derive(Debug, Serialize)]
pub struct PasskeyRevokeResponse {
    pub status: String,
    pub proof_binding_id: String,
    pub revoked_at: u64,
}

#[derive(Debug, Serialize)]
pub struct PasskeyPromoteResponse {
    pub status: String,
    pub proof_binding_id: String,
    pub role: String,
    pub promoted_at: u64,
}

#[derive(Debug, Serialize)]
pub struct PasskeyDemoteResponse {
    pub status: String,
    pub proof_binding_id: String,
    pub role: String,
    pub demoted_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryKitImportResponse {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_principal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_localhost_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_token: Option<String>,
}

#[derive(Debug, Clone)]
struct RecoveryKitMaterialImport {
    principal_id: String,
    localhost_root: String,
    kit: RecoveryKitV1,
    did_recovery_proof: Option<DidRecoveryProofV1>,
    reassign_to_current_principal: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullRecoveryBundleExportRequest {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    #[serde(default)]
    pub label: Option<String>,
    pub home_token: String,
    #[serde(default)]
    pub download_password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullRecoveryBundleImportRequest {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    #[serde(default)]
    pub bundle: Option<Value>,
    #[serde(default)]
    pub package: Option<Value>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub reassign_to_current_principal: bool,
    #[serde(default)]
    pub did_recovery_proof: Option<DidRecoveryProofV1>,
}

#[derive(Debug, Serialize)]
pub struct PasskeyBeginResponse<T> {
    pub schema: String,
    pub ceremony_id: String,
    pub options: T,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegisterCompleteRequest {
    pub ceremony_id: String,
    pub response: RegistrationResponse,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasskeyAuthenticateCompleteRequest {
    pub ceremony_id: String,
    pub response: AuthenticationResponse,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PasskeyAuthenticateBeginRequest {
    pub credential_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PasskeyVerifyResponse {
    pub schema: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub session_id: String,
    pub expires_at: u64,
    pub home_token: String,
    pub system_token: String,
}

#[derive(Debug, Serialize)]
pub struct AuthSessionRefreshResponse {
    pub schema: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub session_id: String,
    pub expires_at: u64,
    pub home_token: String,
    pub system_token: String,
}

pub async fn evm_challenge(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<EvmChallengeRequest>,
) -> Response {
    match evm_challenge_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn evm_verify(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<EvmVerifyRequest>,
) -> Response {
    match evm_verify_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn btc_challenge(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BtcChallengeRequest>,
) -> Response {
    match btc_challenge_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn btc_verify(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BtcVerifyRequest>,
) -> Response {
    match btc_verify_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_status(State(state): State<GatewayState>) -> Response {
    let manager = match state.identity_manager() {
        Ok(manager) => manager,
        Err(err) => return auth_error_response(err),
    };
    let manager = manager.lock().await;
    let registered = manager.status().registered;
    drop(manager);
    let accounts = match passkey_login_accounts(&state.data_dir) {
        Ok(accounts) => accounts,
        Err(err) => return auth_error_response(err),
    };
    Json(PasskeyStatusResponse {
        registered,
        guest_registration_enabled: crate::auth::guest_registration_enabled(&state.data_dir)
            .unwrap_or(false),
        accounts,
    })
    .into_response()
}

/// Unsigned front-door avatar: only the CID registered for this enrolled credential.
pub async fn passkey_account_avatar(
    State(state): State<GatewayState>,
    Query(query): Query<PasskeyAccountAvatarQuery>,
) -> Response {
    match login_avatar_bytes_for_credential(&state.data_dir, &query.credential_id) {
        Ok((bytes, content_type)) => {
            let mut response = bytes.into_response();
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
            response.headers_mut().insert(
                CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=300"),
            );
            // Capsule frames use COEP require-corp; without CORP the <img> is
            // blocked (opaque Origin: null) even when the GET itself is 200.
            response.headers_mut().insert(
                "cross-origin-resource-policy",
                HeaderValue::from_static("cross-origin"),
            );
            response.headers_mut().insert(
                axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("null"),
            );
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn passkey_list(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    match passkey_list_inner(&state, &headers).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn recovery_status(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    match recovery_status_inner(&state, &headers).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn full_recovery_bundle_export(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<FullRecoveryBundleExportRequest>,
) -> Response {
    match full_recovery_bundle_export_inner(&state, &headers, input).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn full_recovery_bundle_import(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<FullRecoveryBundleImportRequest>,
) -> Response {
    match full_recovery_bundle_import_inner(&state, &headers, input).await {
        Ok(response) => {
            let home_token = response
                .get("home_token")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut http_response = Json(response).into_response();
            if let Some(home_token) = home_token {
                let secure = super::gateway::request_uses_tls(&headers);
                if let Ok(cookie) = home_session_cookie_header_for_token(&home_token, secure) {
                    http_response.headers_mut().append(SET_COOKIE, cookie);
                }
            }
            http_response
        }
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_revoke(
    State(state): State<GatewayState>,
    Path(proof_binding_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match passkey_revoke_inner(&state, &headers, proof_binding_id).await {
        Ok((response, clear_current_cookie)) => {
            let mut http_response = Json(response).into_response();
            if clear_current_cookie {
                let secure = super::gateway::request_uses_tls(&headers);
                if let Ok(cookie) = super::gateway::home_session_clear_cookie_header(secure) {
                    http_response.headers_mut().append(SET_COOKIE, cookie);
                }
            }
            http_response
        }
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_promote_admin(
    State(state): State<GatewayState>,
    Path(proof_binding_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match passkey_promote_admin_inner(&state, &headers, proof_binding_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_demote_guest(
    State(state): State<GatewayState>,
    Path(proof_binding_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match passkey_demote_guest_inner(&state, &headers, proof_binding_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn refresh_session(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    match refresh_session_inner(&state, &headers) {
        Ok(response) => {
            let secure = super::gateway::request_uses_tls(&headers);
            let cookie = home_session_cookie_header_for_token(&response.home_token, secure);
            let mut http_response = Json(response).into_response();
            if let Ok(cookie) = cookie {
                http_response.headers_mut().append(SET_COOKIE, cookie);
            }
            http_response
        }
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_register_begin(
    State(state): State<GatewayState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let local_first_owner = local_first_owner_registration(&headers, peer.map(|peer| peer.0));
    match passkey_register_begin_inner(&state, &headers, local_first_owner).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_register_complete(
    State(state): State<GatewayState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(input): Json<PasskeyRegisterCompleteRequest>,
) -> Response {
    let local_first_owner = local_first_owner_registration(&headers, peer.map(|peer| peer.0));
    match passkey_register_complete_inner(&state, &headers, input, local_first_owner).await {
        Ok(response) => passkey_verified_response(&headers, response),
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_authenticate_begin(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Option<Json<PasskeyAuthenticateBeginRequest>>,
) -> Response {
    let input = body.map(|Json(value)| value).unwrap_or_default();
    match passkey_authenticate_begin_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_authenticate_complete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<PasskeyAuthenticateCompleteRequest>,
) -> Response {
    match passkey_authenticate_complete_inner(&state, &headers, input).await {
        Ok(response) => passkey_verified_response(&headers, response),
        Err(err) => auth_error_response(err),
    }
}

pub async fn revoke_session(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let context = match require_auth_home_or_system_context(&state, &headers) {
        Ok(context) => context,
        Err(err) => return auth_error_response(err),
    };
    let now = crate::auth::now_ts();
    let auth_data_dir = super::gateway::home_launch_auth_data_dir(&state.data_dir);
    let target = match crate::auth::load_active_session_grant(&auth_data_dir, &session_id, now) {
        Ok(target) => target,
        Err(err) => return auth_error_response(err),
    };
    let actor = match require_active_principal_for_context(&state, &context) {
        Ok(actor) => actor,
        Err(err) => return auth_error_response(err),
    };
    if target.session_id != context.session_id && !crate::auth::is_admin(&actor) {
        return auth_error_response(anyhow::anyhow!(
            "admin authority required to revoke another auth session"
        ));
    }

    match crate::auth::revoke_session_grant(&auth_data_dir, &session_id, now) {
        Ok(()) => {
            if auth_data_dir != state.data_dir {
                let _ = crate::auth::revoke_session_grant(&state.data_dir, &session_id, now);
            }
            let _ = crate::auth::append_audit_event(
                &auth_data_dir,
                audit_event(AuditEventInput {
                    event_type: "auth.session.revoked",
                    principal_id: Some(actor.principal_id),
                    proof_binding_id: context.proof_binding_id,
                    session_id: Some(session_id.clone()),
                    result: "ok",
                    reason: "session revoked",
                    occurred_at: now,
                    ..AuditEventInput::default()
                }),
            );
            Json(AuthRevokeResponse {
                status: "revoked".to_string(),
                session_id,
            })
            .into_response()
        }
        Err(err) => auth_error_response(err),
    }
}

pub async fn sign_out_session(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let secure = super::gateway::request_uses_tls(&headers);
    let mut http_response = match sign_out_session_inner(&state, &headers) {
        Ok(response) => Json(response).into_response(),
        Err(err) => auth_error_response(err),
    };
    if let Ok(cookie) = super::gateway::home_session_clear_cookie_header(secure) {
        http_response.headers_mut().append(SET_COOKIE, cookie);
    }
    http_response
}

fn sign_out_session_inner(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<AuthRevokeResponse> {
    let context = super::gateway::require_home_token_context(&state.data_dir, headers)?;
    let now = crate::auth::now_ts();
    let auth_data_dir = super::gateway::home_launch_auth_data_dir(&state.data_dir);
    crate::auth::revoke_session_grant(&auth_data_dir, &context.session_id, now)?;
    if auth_data_dir != state.data_dir {
        let _ = crate::auth::revoke_session_grant(&state.data_dir, &context.session_id, now);
    }
    let _ = crate::auth::append_audit_event(
        &auth_data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.session.signed_out",
            principal_id: Some(context.principal_id),
            proof_binding_id: context.proof_binding_id,
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "home browser session signed out",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    );
    Ok(AuthRevokeResponse {
        status: "signed_out".to_string(),
        session_id: context.session_id,
    })
}

async fn passkey_list_inner(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<PasskeyListResponse> {
    let context = require_auth_home_or_system_context(state, headers)?;
    let current_proof_binding_id = context.proof_binding_id.as_deref();
    let actor = require_active_principal_for_context(state, &context)?;
    let actor_is_admin = crate::auth::is_admin(&actor);

    let manager = state.identity_manager()?;
    let manager = manager.lock().await;
    let credentials = manager
        .credentials()
        .into_iter()
        .filter(|credential| {
            actor_is_admin
                || current_proof_binding_id == Some(passkey_proof_binding_id(credential).as_str())
        })
        .collect::<Vec<_>>();
    drop(manager);

    let principals = crate::auth::list_passkey_principals(&state.data_dir)?;
    let principals_by_proof: BTreeMap<_, _> = principals
        .iter()
        .map(|record| (record.proof_binding_id.as_str(), record))
        .collect();
    let mut passkeys = Vec::with_capacity(credentials.len());
    for credential in credentials {
        let proof_binding_id = passkey_proof_binding_id(&credential);
        let principal = principals_by_proof
            .get(proof_binding_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("passkey credential missing runtime proof binding"))?;
        let passkey = principal
            .proof_binding
            .passkey
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("proof binding is not a passkey"))?;
        passkeys.push(PasskeyView {
            proof_binding_id: proof_binding_id.clone(),
            principal_id: principal.principal_id.clone(),
            display_name: principal.display_name.clone(),
            role: principal_role_label(principal.role).to_string(),
            localhost_root: principal.localhost_root.clone(),
            rp_id: credential.rp_id,
            sign_count: credential.sign_count,
            created_at: passkey.created_at,
            last_used_at: passkey.last_used_at,
            revoked_at: passkey.revoked_at,
            current: current_proof_binding_id == Some(proof_binding_id.as_str()),
        });
    }
    Ok(PasskeyListResponse {
        schema: "elastos.auth.passkeys/v1".to_string(),
        passkeys,
    })
}

async fn passkey_revoke_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    proof_binding_id: String,
) -> anyhow::Result<(PasskeyRevokeResponse, bool)> {
    let context = require_auth_home_or_system_context(state, headers)?;
    validate_passkey_proof_binding_id(&proof_binding_id)?;
    let actor = require_active_principal_for_context(state, &context)?;
    let target = crate::auth::load_principal_for_proof_binding(&state.data_dir, &proof_binding_id)?;
    let revoking_self = actor.proof_binding_id == proof_binding_id;
    if !revoking_self && !crate::auth::is_admin(&actor) {
        anyhow::bail!("admin passkey required to remove another passkey");
    }
    if crate::auth::is_admin(&target)
        && crate::auth::active_admin_passkey_principal_count(&state.data_dir)? <= 1
        && crate::auth::active_passkey_principal_count(&state.data_dir)? > 1
    {
        anyhow::bail!("last admin passkey cannot be removed while guest passkeys remain");
    }

    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let credential_id = manager
        .credentials()
        .into_iter()
        .find(|credential| passkey_proof_binding_id(credential) == proof_binding_id)
        .map(|credential| credential.credential_id)
        .ok_or_else(|| anyhow::anyhow!("passkey credential not found"))?;
    manager.revoke_credential(&credential_id)?;
    drop(manager);

    let now = crate::auth::now_ts();
    crate::auth::revoke_passkey_binding(&state.data_dir, &proof_binding_id, now)?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.passkey.revoked",
            principal_id: Some(actor.principal_id),
            proof_binding_id: Some(proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "passkey credential revoked",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    let clear_current_cookie =
        context.proof_binding_id.as_deref() == Some(proof_binding_id.as_str());
    Ok((
        PasskeyRevokeResponse {
            status: "revoked".to_string(),
            proof_binding_id,
            revoked_at: now,
        },
        clear_current_cookie,
    ))
}

async fn passkey_promote_admin_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    proof_binding_id: String,
) -> anyhow::Result<PasskeyPromoteResponse> {
    let context = require_auth_home_or_system_context(state, headers)?;
    validate_passkey_proof_binding_id(&proof_binding_id)?;
    let actor = require_active_principal_for_context(state, &context)?;
    if !crate::auth::is_admin(&actor) {
        anyhow::bail!("admin passkey required to promote a guest passkey");
    }
    let target = crate::auth::load_principal_for_proof_binding(&state.data_dir, &proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&target)?;
    if target.proof_binding.passkey.is_none() {
        anyhow::bail!("proof binding is not a passkey");
    }
    if crate::auth::is_admin(&target) {
        anyhow::bail!("passkey is already admin");
    }

    let now = crate::auth::now_ts();
    let promoted = crate::auth::promote_passkey_to_admin(&state.data_dir, &proof_binding_id, now)?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.passkey.promoted",
            principal_id: Some(actor.principal_id),
            proof_binding_id: Some(promoted.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "guest passkey promoted to admin",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    Ok(PasskeyPromoteResponse {
        status: "promoted".to_string(),
        proof_binding_id,
        role: principal_role_label(promoted.role).to_string(),
        promoted_at: now,
    })
}

async fn passkey_demote_guest_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    proof_binding_id: String,
) -> anyhow::Result<PasskeyDemoteResponse> {
    let context = require_auth_home_or_system_context(state, headers)?;
    validate_passkey_proof_binding_id(&proof_binding_id)?;
    let actor = require_active_principal_for_context(state, &context)?;
    if !crate::auth::is_admin(&actor) {
        anyhow::bail!("admin passkey required to demote another admin passkey");
    }
    if actor.proof_binding_id == proof_binding_id {
        anyhow::bail!("admin passkey cannot demote itself");
    }
    let target = crate::auth::load_principal_for_proof_binding(&state.data_dir, &proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&target)?;
    if target.proof_binding.passkey.is_none() {
        anyhow::bail!("proof binding is not a passkey");
    }
    if !crate::auth::is_admin(&target) {
        anyhow::bail!("passkey is already guest");
    }
    if crate::auth::active_admin_passkey_principal_count(&state.data_dir)? <= 1 {
        anyhow::bail!("last admin passkey cannot be demoted");
    }

    let now = crate::auth::now_ts();
    let demoted = crate::auth::demote_passkey_to_guest(&state.data_dir, &proof_binding_id, now)?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.passkey.demoted",
            principal_id: Some(actor.principal_id),
            proof_binding_id: Some(demoted.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "admin passkey demoted to guest",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    Ok(PasskeyDemoteResponse {
        status: "demoted".to_string(),
        proof_binding_id,
        role: principal_role_label(demoted.role).to_string(),
        demoted_at: now,
    })
}

async fn recovery_status_inner(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<PrincipalRootRecoveryStatusV1> {
    let context = require_auth_home_or_system_context(state, headers)?;
    let principal = require_active_passkey_principal_for_context(state, &context)?;
    let Some(protection) = crate::auth::load_principal_root_protection(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )?
    else {
        return Ok(PrincipalRootRecoveryStatusV1::unprotected(
            principal.principal_id,
            principal.localhost_root,
        ));
    };
    let protection_configured = !protection.protectors.is_empty();
    let recovery_configured = protection
        .protectors
        .iter()
        .any(|protector| protector.verified_at.is_some());
    let recovery_download_available = recovery_archive_from_protection(&protection).is_some();
    let required_actions = if recovery_configured {
        Vec::new()
    } else {
        vec!["verify_recovery_before_public_guest_hosting".to_string()]
    };
    Ok(PrincipalRootRecoveryStatusV1 {
        schema: elastos_runtime::auth::PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA.to_string(),
        principal_id: principal.principal_id,
        localhost_root: principal.localhost_root,
        root_encrypted: true,
        recovery_configured,
        recovery_download_available,
        protection_configured,
        required_actions,
        crypto: protection.crypto,
    })
}

async fn full_recovery_bundle_export_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: FullRecoveryBundleExportRequest,
) -> anyhow::Result<Value> {
    if input.schema != FULL_RECOVERY_BUNDLE_EXPORT_REQUEST_SCHEMA {
        anyhow::bail!("unsupported full recovery bundle export request schema");
    }
    let (app, context) = require_auth_home_or_system_app_context(state, headers)?;
    consume_fresh_passkey_home_token(
        &state.data_dir,
        &input.home_token,
        &context,
        &app,
        180,
        "auth.full-recovery-bundle.export",
        &serde_json::json!({
            "principal_id": input.principal_id,
            "localhost_root": input.localhost_root,
            "label": input.label,
        }),
    )?;
    let principal = require_active_passkey_principal_for_context(state, &context)?;
    if input.principal_id != principal.principal_id
        || input.localhost_root != principal.localhost_root
    {
        return fail_recovery_kit_request(
            state,
            &context,
            &principal,
            "auth.full_recovery_bundle.export.rejected",
            "full recovery bundle principal binding mismatch",
        );
    }

    let now = crate::auth::now_ts();
    let kit = recovery_kit_get_or_create_for_principal(
        state,
        &context,
        &principal,
        input.label.as_deref(),
        now,
    )?;
    let wallet_recovery_keys =
        wallet_recovery_keys_for_principal(state, &principal.principal_id).await?;
    let wallet_recovery_key_count = wallet_recovery_keys.len();
    let bundle = json!({
        "schema": FULL_RECOVERY_BUNDLE_SCHEMA,
        "bundle_id": format!("bundle:{}", random_hex(16)),
        "principal_id": principal.principal_id.clone(),
        "localhost_root": principal.localhost_root.clone(),
        "data_kit": kit,
        "wallet_recovery_keys": wallet_recovery_keys,
        "included": {
            "data_kit": true,
            "wallet_recovery_key_count": wallet_recovery_key_count
        },
        "created_at": now,
        "instructions": [
            "Keep this Full Recovery Bundle offline. Anyone with it can recover this ElastOS user root and included built-in Wallet accounts.",
            "Import it only through ElastOS System recovery on a runtime you control."
        ]
    });
    let value = match input
        .download_password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(password) => password_protected_full_recovery_bundle(&bundle, password)?,
        None => bundle,
    };
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.full_recovery_bundle.exported",
            principal_id: Some(context.principal_id.clone()),
            proof_binding_id: context.proof_binding_id.clone(),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "full recovery bundle downloaded after fresh passkey verification",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(value)
}

async fn full_recovery_bundle_import_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: FullRecoveryBundleImportRequest,
) -> anyhow::Result<Value> {
    if input.schema != FULL_RECOVERY_BUNDLE_IMPORT_REQUEST_SCHEMA {
        anyhow::bail!("unsupported full recovery bundle import request schema");
    }
    let context = require_auth_home_or_system_context(state, headers)?;
    let bundle = full_recovery_bundle_from_import_request(&input)?;
    let bundle_principal = bundle
        .get("principal_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing principal_id"))?;
    let bundle_root = bundle
        .get("localhost_root")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing localhost_root"))?;
    if !input.reassign_to_current_principal
        && (input.principal_id != bundle_principal || input.localhost_root != bundle_root)
    {
        anyhow::bail!(
            "full recovery bundle belongs to another account; use account recovery to attach it"
        );
    }
    let data_kit: RecoveryKitV1 = serde_json::from_value(
        bundle
            .get("data_kit")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing data_kit"))?,
    )?;
    let recovery_response = recovery_kit_import_inner(
        state,
        headers,
        RecoveryKitMaterialImport {
            principal_id: input.principal_id,
            localhost_root: input.localhost_root,
            kit: data_kit,
            did_recovery_proof: input.did_recovery_proof,
            reassign_to_current_principal: input.reassign_to_current_principal,
        },
    )
    .await?;
    let wallet_keys = bundle
        .get("wallet_recovery_keys")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut imported_wallet_keys = Vec::new();
    for key in wallet_keys {
        if key.get("schema").and_then(Value::as_str) != Some(WALLET_RECOVERY_KEY_SCHEMA) {
            continue;
        }
        let label = key.get("label").and_then(Value::as_str).map(str::to_string);
        wallet_provider_data(
            state,
            json!({
                "op": "import_managed_secret",
                "principal_id": recovery_response.principal_id,
                "recovery_key": key,
                "label": label,
            }),
        )
        .await?;
        imported_wallet_keys.push(json!({ "schema": WALLET_RECOVERY_KEY_SCHEMA }));
    }
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.full_recovery_bundle.imported",
            principal_id: Some(recovery_response.principal_id.clone()),
            proof_binding_id: context.proof_binding_id.clone(),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "full recovery bundle imported and included wallet keys restored",
            occurred_at: crate::auth::now_ts(),
            ..AuditEventInput::default()
        }),
    )?;
    Ok(json!({
        "schema": FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA,
        "principal_id": recovery_response.principal_id,
        "localhost_root": recovery_response.localhost_root,
        "status": recovery_response.status,
        "previous_principal_id": recovery_response.previous_principal_id,
        "previous_localhost_root": recovery_response.previous_localhost_root,
        "home_token": recovery_response.home_token,
        "system_token": recovery_response.system_token,
        "wallet_recovery_key_count": imported_wallet_keys.len(),
    }))
}

fn full_recovery_bundle_from_import_request(
    input: &FullRecoveryBundleImportRequest,
) -> anyhow::Result<Value> {
    if input.bundle.is_some() == input.package.is_some() {
        anyhow::bail!("import exactly one full recovery bundle or package");
    }
    if let Some(bundle) = input.bundle.as_ref() {
        validate_full_recovery_bundle(bundle)?;
        return Ok(bundle.clone());
    }
    let package = input
        .package
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing full recovery bundle package"))?;
    let password = input
        .password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle password is required"))?;
    full_recovery_bundle_from_password_package(package, password)
}

fn recovery_kit_get_or_create_for_principal(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
    principal: &crate::auth::PrincipalRecord,
    label: Option<&str>,
    now: u64,
) -> anyhow::Result<RecoveryKitV1> {
    if let Some(protection) = crate::auth::load_principal_root_protection(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )? {
        if let Some(archive) = recovery_archive_from_protection(&protection) {
            let kit = crate::auth::recovery_kit_from_archive(&state.data_dir, archive)?;
            crate::auth::verify_recovery_kit_material(&kit)?;
            if kit.principal_id != principal.principal_id
                || kit.localhost_root != principal.localhost_root
            {
                anyhow::bail!("recovery kit archive principal binding mismatch");
            }
            return Ok(kit);
        }
    }
    let kit = create_recovery_kit_for_principal(
        &principal.principal_id,
        &principal.localhost_root,
        label,
        now,
    )?;
    let archive = crate::auth::recovery_archive_from_kit(&state.data_dir, &kit)?;
    let protection = protection_from_recovery_kit(&kit, label, now, Some(archive))?;
    crate::auth::store_principal_root_protection(&state.data_dir, protection)?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.recovery_kit.created",
            principal_id: Some(principal.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "principal recovery kit created for full recovery bundle",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(kit)
}

async fn wallet_recovery_keys_for_principal(
    state: &GatewayState,
    principal_id: &str,
) -> anyhow::Result<Vec<Value>> {
    let accounts = wallet_provider_data(
        state,
        json!({
            "op": "accounts",
            "principal_id": principal_id,
            "include_revoked": false,
        }),
    )
    .await?;
    let mut recovery_keys = Vec::new();
    for account in accounts
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let proof_type = account
            .get("proof_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if proof_type != "managed_evm" && proof_type != "managed_btc_p2wpkh" {
            continue;
        }
        let Some(account_id) = account.get("account_id").and_then(Value::as_str) else {
            continue;
        };
        let mut key = wallet_provider_data(
            state,
            json!({
                "op": "export_managed_secret",
                "principal_id": principal_id,
                "account_id": account_id,
            }),
        )
        .await?;
        if let Some(label) = account.get("label").and_then(Value::as_str) {
            key["label"] = json!(label);
        }
        recovery_keys.push(key);
    }
    Ok(recovery_keys)
}

fn password_protected_full_recovery_bundle(
    bundle: &Value,
    password: &str,
) -> anyhow::Result<Value> {
    validate_full_recovery_bundle(bundle)?;
    let mut salt = [0u8; 32];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let principal_id = full_bundle_str(bundle, "principal_id")?;
    let localhost_root = full_bundle_str(bundle, "localhost_root")?;
    let bundle_id = full_bundle_str(bundle, "bundle_id")?;
    let created_at = bundle
        .get("created_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing created_at"))?;
    let key =
        derive_full_recovery_bundle_key(password, &salt, principal_id, localhost_root, bundle_id)?;
    let bytes = serde_json::to_vec(bundle)?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    let encrypted_bundle = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &bytes,
                aad: full_recovery_bundle_aad(principal_id, localhost_root, bundle_id).as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("full recovery bundle encryption failed"))?;
    Ok(json!({
        "schema": FULL_RECOVERY_BUNDLE_PACKAGE_SCHEMA,
        "principal_id": principal_id,
        "localhost_root": localhost_root,
        "bundle_id": bundle_id,
        "created_at": created_at,
        "protection": {
            "cipher": "aes-256-gcm",
            "kdf": "argon2id",
            "kdf_params": FULL_RECOVERY_BUNDLE_KDF_PARAMS,
            "salt": URL_SAFE_NO_PAD.encode(salt),
            "nonce": URL_SAFE_NO_PAD.encode(nonce),
            "encrypted_full_recovery_bundle": URL_SAFE_NO_PAD.encode(encrypted_bundle)
        }
    }))
}

fn full_recovery_bundle_from_password_package(
    package: &Value,
    password: &str,
) -> anyhow::Result<Value> {
    if package.get("schema").and_then(Value::as_str) != Some(FULL_RECOVERY_BUNDLE_PACKAGE_SCHEMA) {
        anyhow::bail!("unsupported full recovery bundle package schema");
    }
    let principal_id = full_bundle_str(package, "principal_id")?;
    let localhost_root = full_bundle_str(package, "localhost_root")?;
    let bundle_id = full_bundle_str(package, "bundle_id")?;
    let protection = package
        .get("protection")
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle package missing protection"))?;
    if protection.get("cipher").and_then(Value::as_str) != Some("aes-256-gcm") {
        anyhow::bail!("unsupported full recovery bundle package cipher");
    }
    if protection.get("kdf").and_then(Value::as_str) != Some("argon2id") {
        anyhow::bail!("unsupported full recovery bundle package kdf");
    }
    let salt = b64_decode_field(protection, "salt")?;
    let nonce = b64_decode_field(protection, "nonce")?;
    let ciphertext = b64_decode_field(protection, "encrypted_full_recovery_bundle")?;
    if salt.len() != 32 {
        anyhow::bail!("full recovery bundle package salt must be 32 bytes");
    }
    if nonce.len() != 12 {
        anyhow::bail!("full recovery bundle package nonce must be 12 bytes");
    }
    let key =
        derive_full_recovery_bundle_key(password, &salt, principal_id, localhost_root, bundle_id)?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: full_recovery_bundle_aad(principal_id, localhost_root, bundle_id).as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("invalid full recovery bundle password or ciphertext"))?;
    let bundle: Value = serde_json::from_slice(&plaintext)?;
    validate_full_recovery_bundle(&bundle)?;
    if full_bundle_str(&bundle, "principal_id")? != principal_id
        || full_bundle_str(&bundle, "localhost_root")? != localhost_root
        || full_bundle_str(&bundle, "bundle_id")? != bundle_id
    {
        anyhow::bail!("full recovery bundle package binding mismatch");
    }
    Ok(bundle)
}

fn validate_full_recovery_bundle(bundle: &Value) -> anyhow::Result<()> {
    if bundle.get("schema").and_then(Value::as_str) != Some(FULL_RECOVERY_BUNDLE_SCHEMA) {
        anyhow::bail!("unsupported full recovery bundle schema");
    }
    let principal_id = full_bundle_str(bundle, "principal_id")?;
    let localhost_root = full_bundle_str(bundle, "localhost_root")?;
    let bundle_id = full_bundle_str(bundle, "bundle_id")?;
    if !bundle_id.starts_with("bundle:") {
        anyhow::bail!("full recovery bundle id must start with bundle:");
    }
    let kit = bundle
        .get("data_kit")
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing data_kit"))?;
    if kit.get("schema").and_then(Value::as_str) != Some(elastos_runtime::auth::RECOVERY_KIT_SCHEMA)
    {
        anyhow::bail!("full recovery bundle data_kit must be a Recovery Kit");
    }
    if kit.get("principal_id").and_then(Value::as_str) != Some(principal_id)
        || kit.get("localhost_root").and_then(Value::as_str) != Some(localhost_root)
    {
        anyhow::bail!("full recovery bundle data_kit binding mismatch");
    }
    if !bundle
        .get("wallet_recovery_keys")
        .map(Value::is_array)
        .unwrap_or(false)
    {
        anyhow::bail!("full recovery bundle wallet_recovery_keys must be an array");
    }
    Ok(())
}

fn derive_full_recovery_bundle_key(
    password: &str,
    salt: &[u8],
    principal_id: &str,
    localhost_root: &str,
    bundle_id: &str,
) -> anyhow::Result<[u8; 32]> {
    let params = Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|err| anyhow::anyhow!("invalid full recovery bundle KDF params: {err}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    let input = format!("{principal_id}:{localhost_root}:{bundle_id}:{password}");
    argon2
        .hash_password_into(input.as_bytes(), salt, &mut key)
        .map_err(|err| anyhow::anyhow!("full recovery bundle key derivation failed: {err}"))?;
    Ok(key)
}

fn full_recovery_bundle_aad(principal_id: &str, localhost_root: &str, bundle_id: &str) -> String {
    format!("{FULL_RECOVERY_BUNDLE_AAD_DOMAIN}\n{principal_id}\n{localhost_root}\n{bundle_id}")
}

fn full_bundle_str<'a>(value: &'a Value, field: &str) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing {field}"))
}

fn b64_decode_field(value: &Value, field: &str) -> anyhow::Result<Vec<u8>> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle package missing {field}"))?;
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| anyhow::anyhow!("full recovery bundle package invalid {field}"))
}

async fn recovery_kit_import_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: RecoveryKitMaterialImport,
) -> anyhow::Result<RecoveryKitImportResponse> {
    let context = require_auth_home_or_system_context(state, headers)?;
    let principal = require_active_passkey_principal_for_context(state, &context)?;
    if input.principal_id != principal.principal_id
        || input.localhost_root != principal.localhost_root
    {
        return fail_recovery_kit_request(
            state,
            &context,
            &principal,
            "auth.recovery_kit.import.rejected",
            "recovery request principal binding mismatch",
        );
    }
    let kit = input.kit;
    if !input.reassign_to_current_principal
        && (kit.principal_id != input.principal_id || kit.localhost_root != input.localhost_root)
    {
        return fail_recovery_kit_request(
            state,
            &context,
            &principal,
            "auth.recovery_kit.import.rejected",
            "recovery kit principal binding mismatch",
        );
    }
    if let Err(err) = crate::auth::verify_recovery_kit_material(&kit) {
        return fail_recovery_kit_request(
            state,
            &context,
            &principal,
            "auth.recovery_kit.import.rejected",
            format!("invalid recovery kit: {err}"),
        );
    }
    if input.reassign_to_current_principal
        && kit.localhost_root != crate::auth::principal_localhost_root(&kit.principal_id)
    {
        return fail_recovery_kit_request(
            state,
            &context,
            &principal,
            "auth.recovery_kit.import.rejected",
            "recovered principal root is not canonical for the recovered principal",
        );
    }
    let now = crate::auth::now_ts();
    let previous_principal_id = principal.principal_id.clone();
    let previous_localhost_root = principal.localhost_root.clone();
    let verified_did_recovery_protector = match input.did_recovery_proof.as_ref() {
        Some(proof) => match verify_did_recovery_import_proof(state, &kit, proof, now).await {
            Ok(protector) => Some(protector),
            Err(err) => {
                return fail_recovery_kit_request(
                    state,
                    &context,
                    &principal,
                    "auth.recovery_kit.import.rejected",
                    format!("DID recovery proof verification failed: {err}"),
                );
            }
        },
        None => None,
    };
    let archive = crate::auth::recovery_archive_from_kit(&state.data_dir, &kit)?;
    let mut protection =
        protection_from_recovery_kit(&kit, Some("Imported Recovery Kit"), now, Some(archive))?;
    if let Some(protector) = verified_did_recovery_protector {
        protection.protectors.push(protector);
    }
    if input.reassign_to_current_principal {
        if let Err(err) = crate::auth::ensure_recovered_root_reassignable(
            &state.data_dir,
            &principal.proof_binding_id,
            &kit.principal_id,
            &kit.localhost_root,
        ) {
            return fail_recovery_kit_request(
                state,
                &context,
                &principal,
                "auth.recovery_kit.import.rejected",
                format!("recovery root reassignment failed: {err}"),
            );
        }
    }
    crate::auth::store_principal_root_protection(&state.data_dir, protection)?;
    let (principal, home_token, system_token, audit_session_id, status, event_type, reason) =
        if input.reassign_to_current_principal {
            let proof_binding_id = principal.proof_binding_id.clone();
            let principal = match crate::auth::reassign_passkey_binding_to_recovered_root(
                &state.data_dir,
                &proof_binding_id,
                &kit.principal_id,
                &kit.localhost_root,
                now,
            ) {
                Ok(principal) => principal,
                Err(err) => {
                    return fail_recovery_kit_request(
                        state,
                        &context,
                        &principal,
                        "auth.recovery_kit.import.rejected",
                        format!("recovery root reassignment failed: {err}"),
                    );
                }
            };
            let grant = AuthSessionGrantV1 {
                schema: AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: format!("grant:{}", random_hex(16)),
                session_id: format!("auth:{}", random_hex(16)),
                principal_id: principal.principal_id.clone(),
                proof_binding_id: principal.proof_binding_id.clone(),
                issued_at: now,
                expires_at: now.saturating_add(AUTH_SESSION_TTL_SECS),
                apps: vec![
                    HOME_CAPSULE_ID.to_string(),
                    super::gateway::SYSTEM_CAPSULE_ID.to_string(),
                ],
            };
            crate::auth::store_session_grant(&state.data_dir, grant.clone())?;
            let home_token =
                issue_home_launch_token_for_auth_grant(&state.data_dir, HOME_CAPSULE_ID, &grant)?;
            let system_token = issue_home_launch_token_for_auth_grant(
                &state.data_dir,
                super::gateway::SYSTEM_CAPSULE_ID,
                &grant,
            )?;
            (
                principal,
                Some(home_token),
                Some(system_token),
                grant.session_id,
                "reassigned",
                "auth.recovery_kit.reassigned",
                "principal root reassigned from verified Recovery Kit and session reissued",
            )
        } else {
            (
                principal,
                None,
                None,
                context.session_id.clone(),
                "imported",
                "auth.recovery_kit.imported",
                "principal recovery kit imported and verified",
            )
        };
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type,
            principal_id: Some(principal.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id),
            session_id: Some(audit_session_id),
            result: "ok",
            reason,
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(RecoveryKitImportResponse {
        schema: "elastos.recovery-kit.import.response/v1".to_string(),
        principal_id: principal.principal_id,
        localhost_root: principal.localhost_root,
        status: status.to_string(),
        previous_principal_id: input
            .reassign_to_current_principal
            .then_some(previous_principal_id),
        previous_localhost_root: input
            .reassign_to_current_principal
            .then_some(previous_localhost_root),
        home_token,
        system_token,
    })
}

fn create_recovery_kit_for_principal(
    principal_id: &str,
    localhost_root: &str,
    label: Option<&str>,
    created_at: u64,
) -> anyhow::Result<RecoveryKitV1> {
    let mut data_key = [0u8; 32];
    let mut salt = [0u8; 32];
    let mut wrap_nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut data_key);
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut wrap_nonce);
    let recovery_phrase = random_recovery_phrase();
    let crypto = PrincipalRootCryptoProfileV1 {
        recovery_kdf: "hkdf-sha256".to_string(),
        ..PrincipalRootCryptoProfileV1::default()
    };
    let wrapping_key = crate::auth::derive_recovery_wrapping_key(
        &recovery_phrase,
        &salt,
        principal_id,
        localhost_root,
    )?;
    let wrapped_data_key =
        crate::auth::encrypt_aes256_gcm_bytes(&wrapping_key, &wrap_nonce, &data_key)?;
    let data_key_id = crate::auth::principal_data_key_id(&data_key);
    let descriptor = json!({
        "schema": RECOVERY_DESCRIPTOR_SCHEMA,
        "principal_id": principal_id,
        "localhost_root": localhost_root,
        "data_key_id": data_key_id,
        "created_at": created_at,
    });
    let mut descriptor_nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut descriptor_nonce);
    let descriptor_bytes = serde_json::to_vec(&descriptor)?;
    let descriptor_ciphertext =
        crate::auth::encrypt_aes256_gcm_bytes(&data_key, &descriptor_nonce, &descriptor_bytes)?;
    let encrypted_root_descriptor = format!(
        "aes-256-gcm:v1:{}:{}",
        crate::auth::b64_url(&descriptor_nonce),
        descriptor_ciphertext
    );
    let kit_id = format!(
        "kit:{}",
        hex::encode(
            &Sha256::digest(
                format!(
                    "{principal_id}:{localhost_root}:{created_at}:{}",
                    crate::auth::b64_url(&salt)
                )
                .as_bytes()
            )[..16]
        )
    );
    let protector_id = format!(
        "protector:recovery:{}",
        hex::encode(&Sha256::digest(format!("{kit_id}:{data_key_id}").as_bytes())[..16])
    );
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Recovery Kit");
    Ok(RecoveryKitV1 {
        schema: elastos_runtime::auth::RECOVERY_KIT_SCHEMA.to_string(),
        kit_id,
        protector_id,
        principal_id: principal_id.to_string(),
        localhost_root: localhost_root.to_string(),
        data_key_id,
        recovery_phrase,
        salt: crate::auth::b64_url(&salt),
        nonce: crate::auth::b64_url(&wrap_nonce),
        wrapped_data_key,
        encrypted_root_descriptor,
        crypto,
        created_at,
        instructions: vec![
            format!(
                "Keep this {label} offline. Anyone with it can recover this ElastOS user root."
            ),
            "Import it only through ElastOS System recovery on a runtime you control.".to_string(),
        ],
    })
}

fn protection_from_recovery_kit(
    kit: &RecoveryKitV1,
    label: Option<&str>,
    now: u64,
    archive: Option<PrincipalRootRecoveryArchiveV1>,
) -> anyhow::Result<PrincipalRootProtectionV1> {
    crate::auth::verify_recovery_kit_material(kit)?;
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Recovery Kit")
        .to_string();
    Ok(PrincipalRootProtectionV1 {
        schema: elastos_runtime::auth::PRINCIPAL_ROOT_PROTECTION_SCHEMA.to_string(),
        principal_id: kit.principal_id.clone(),
        localhost_root: kit.localhost_root.clone(),
        data_key_id: kit.data_key_id.clone(),
        crypto: kit.crypto.clone(),
        protectors: vec![PrincipalRootProtectorV1 {
            protector_id: kit.protector_id.clone(),
            kind: PrincipalRootProtectorKind::RecoveryKit,
            label,
            subject: None,
            created_at: kit.created_at,
            verified_at: Some(now),
            envelope: Some(PrincipalRootProtectorEnvelopeV1 {
                cipher: kit.crypto.cipher.clone(),
                kdf: kit.crypto.recovery_kdf.clone(),
                salt: kit.salt.clone(),
                nonce: kit.nonce.clone(),
                wrapped_data_key: kit.wrapped_data_key.clone(),
            }),
            archive,
        }],
        created_at: kit.created_at,
        updated_at: now,
    })
}

async fn verify_did_recovery_import_proof(
    state: &GatewayState,
    kit: &RecoveryKitV1,
    proof: &DidRecoveryProofV1,
    now: u64,
) -> anyhow::Result<PrincipalRootProtectorV1> {
    elastos_runtime::auth::validate_did_recovery_proof(proof).map_err(anyhow::Error::msg)?;
    if proof.principal_id != kit.principal_id
        || proof.localhost_root != kit.localhost_root
        || proof.data_key_id != kit.data_key_id
    {
        anyhow::bail!("proof binding does not match the recovered root");
    }

    let existing = crate::auth::load_principal_root_protection(
        &state.data_dir,
        &kit.principal_id,
        &kit.localhost_root,
    )?
    .ok_or_else(|| anyhow::anyhow!("no existing DID recovery protector for recovered root"))?;
    if existing.data_key_id != kit.data_key_id {
        anyhow::bail!("existing root protection uses a different data key");
    }
    let Some(mut protector) = existing
        .protectors
        .iter()
        .find(|protector| {
            protector.kind == PrincipalRootProtectorKind::DidRecovery
                && protector.protector_id == proof.protector_id
                && protector.subject.as_deref() == Some(proof.did.as_str())
        })
        .cloned()
    else {
        anyhow::bail!("DID recovery proof does not match a configured protector");
    };
    if protector.envelope.is_none() {
        anyhow::bail!("DID recovery protector has no encrypted data-key envelope");
    }

    let data = provider_data(
        state,
        "did",
        json!({
            "op": "verify_did_recovery",
            "did": proof.did.as_str(),
            "principal_id": proof.principal_id.as_str(),
            "localhost_root": proof.localhost_root.as_str(),
            "protector_id": proof.protector_id.as_str(),
            "data_key_id": proof.data_key_id.as_str(),
            "nonce": proof.nonce.as_str(),
            "issued_at": proof.issued_at,
            "expires_at": proof.expires_at,
            "signature": proof.signature.as_str(),
        }),
    )
    .await?;
    if data.get("schema").and_then(|value| value.as_str()) != Some("elastos.did.recovery-proof/v1")
    {
        anyhow::bail!("DID provider returned an unsupported recovery proof schema");
    }
    if data.get("valid").and_then(|value| value.as_bool()) != Some(true) {
        anyhow::bail!("DID provider rejected the recovery proof");
    }
    for (field, expected) in [
        ("did", proof.did.as_str()),
        ("principal_id", proof.principal_id.as_str()),
        ("localhost_root", proof.localhost_root.as_str()),
        ("protector_id", proof.protector_id.as_str()),
        ("data_key_id", proof.data_key_id.as_str()),
    ] {
        if data.get(field).and_then(|value| value.as_str()) != Some(expected) {
            anyhow::bail!("DID provider response changed the {field} binding");
        }
    }

    protector.verified_at = Some(now);
    protector.archive = None;
    Ok(protector)
}

fn recovery_archive_from_protection(
    protection: &PrincipalRootProtectionV1,
) -> Option<&PrincipalRootRecoveryArchiveV1> {
    protection
        .protectors
        .iter()
        .find(|protector| protector.kind == PrincipalRootProtectorKind::RecoveryKit)
        .and_then(|protector| protector.archive.as_ref())
}

fn random_recovery_phrase() -> String {
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("-")
}

fn fail_recovery_kit_request<T>(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
    principal: &crate::auth::PrincipalRecord,
    event_type: &str,
    reason: impl Into<String>,
) -> anyhow::Result<T> {
    let reason = reason.into();
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type,
            principal_id: Some(principal.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "denied",
            reason: &reason,
            occurred_at: crate::auth::now_ts(),
            ..AuditEventInput::default()
        }),
    )?;
    anyhow::bail!("{reason}")
}

fn require_active_principal_for_context(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
) -> anyhow::Result<crate::auth::PrincipalRecord> {
    let proof_binding_id = context
        .proof_binding_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let principal =
        crate::auth::load_principal_for_proof_binding(&state.data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if principal.principal_id != context.principal_id {
        anyhow::bail!("auth session principal mismatch");
    }
    Ok(principal)
}

fn require_active_passkey_principal_for_context(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
) -> anyhow::Result<crate::auth::PrincipalRecord> {
    let principal = require_active_principal_for_context(state, context)?;
    if principal.proof_binding.passkey.is_none() {
        anyhow::bail!("passkey authority required for recovery kit operations");
    }
    Ok(principal)
}

fn refresh_session_inner(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<AuthSessionRefreshResponse> {
    let context = super::gateway::require_home_token_context(&state.data_dir, headers)?;
    let proof_binding_id = context
        .proof_binding_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let now = crate::auth::now_ts();
    let auth_data_dir = super::gateway::home_launch_auth_data_dir(&state.data_dir);
    let previous =
        crate::auth::load_active_session_grant(&auth_data_dir, &context.session_id, now)?;
    let principal =
        crate::auth::load_principal_for_proof_binding(&auth_data_dir, &proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if previous.principal_id != context.principal_id
        || previous.proof_binding_id != proof_binding_id
        || previous.grant_id != context.grant_id
    {
        anyhow::bail!("home launch token authority context mismatch");
    }
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: previous.grant_id.clone(),
        session_id: previous.session_id.clone(),
        principal_id: previous.principal_id.clone(),
        proof_binding_id: previous.proof_binding_id.clone(),
        issued_at: previous.issued_at,
        expires_at: now.saturating_add(AUTH_SESSION_TTL_SECS),
        apps: previous.apps,
    };
    crate::auth::renew_session_grant(&auth_data_dir, grant.clone())?;
    if auth_data_dir != state.data_dir {
        let _ = crate::auth::renew_session_grant(&state.data_dir, grant.clone());
    }
    crate::auth::append_audit_event(
        &auth_data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.session.refreshed",
            principal_id: Some(grant.principal_id.clone()),
            proof_binding_id: Some(grant.proof_binding_id.clone()),
            session_id: Some(grant.session_id.clone()),
            result: "ok",
            reason: "proof-bound session refreshed",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    let home_token =
        issue_home_launch_token_for_auth_grant(&state.data_dir, HOME_CAPSULE_ID, &grant)?;
    let system_token = issue_home_launch_token_for_auth_grant(
        &state.data_dir,
        super::gateway::SYSTEM_CAPSULE_ID,
        &grant,
    )?;
    Ok(AuthSessionRefreshResponse {
        schema: "elastos.auth.session.refresh/v1".to_string(),
        principal_id: grant.principal_id,
        proof_binding_id: grant.proof_binding_id,
        session_id: grant.session_id,
        expires_at: grant.expires_at,
        home_token,
        system_token,
    })
}

fn require_auth_home_or_system_context(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<super::gateway::HomeLaunchTokenContext> {
    let context = super::gateway::require_home_launch_token_for_any_context(
        &state.data_dir,
        headers,
        &[HOME_CAPSULE_ID, super::gateway::SYSTEM_CAPSULE_ID],
    )?;
    if context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    Ok(context)
}

fn require_auth_home_or_system_app_context(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<(String, super::gateway::HomeLaunchTokenContext)> {
    let (app, context) = super::gateway::require_home_launch_token_for_any_app_context(
        &state.data_dir,
        headers,
        &[HOME_CAPSULE_ID, super::gateway::SYSTEM_CAPSULE_ID],
    )?;
    if context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    Ok((app, context))
}

struct WalletLinkContext {
    app: String,
    context: super::gateway::HomeLaunchTokenContext,
}

fn require_wallet_link_context(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<WalletLinkContext> {
    let (app, context) = super::gateway::require_home_launch_token_for_any_app_context(
        &state.data_dir,
        headers,
        WALLET_LINK_CAPSULE_IDS,
    )?;
    super::gateway::ensure_wallet_connector_configured(&state.data_dir, &app)?;
    if context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    Ok(WalletLinkContext { app, context })
}

fn validate_passkey_proof_binding_id(proof_binding_id: &str) -> anyhow::Result<()> {
    if !proof_binding_id.starts_with("proof:passkey:")
        || proof_binding_id.len() > 256
        || proof_binding_id
            .chars()
            .any(|ch| ch == '/' || ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid passkey proof binding id");
    }
    Ok(())
}

pub(crate) fn principal_role_label(role: crate::auth::RuntimePrincipalRole) -> &'static str {
    match role {
        crate::auth::RuntimePrincipalRole::Admin => "admin",
        crate::auth::RuntimePrincipalRole::Guest => "guest",
    }
}

async fn passkey_register_begin_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    local_first_owner: bool,
) -> anyhow::Result<PasskeyBeginResponse<CreationOptions>> {
    require_passkey_registration_allowed(state, local_first_owner).await?;
    let ceremony_id = format!("passkey:register:{}", random_hex(16));
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let options = manager.begin_principal_registration(&ceremony_id, &rp.id)?;
    Ok(PasskeyBeginResponse {
        schema: "elastos.auth.passkey.register.begin/v1".to_string(),
        ceremony_id,
        options,
    })
}

async fn passkey_register_complete_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyRegisterCompleteRequest,
    local_first_owner: bool,
) -> anyhow::Result<PasskeyVerifyResponse> {
    require_passkey_registration_allowed(state, local_first_owner).await?;
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let outcome =
        manager.complete_registration(&input.ceremony_id, &input.response, &rp.id, &rp.origin)?;
    let credential = outcome.credential.clone();
    let origin = outcome.origin.clone();
    let user_verified = outcome.user_verified;
    drop(manager);
    issue_named_passkey_session_grant(
        state,
        &outcome.user_id,
        &credential,
        &origin,
        user_verified,
        "passkey registration verified and session granted",
        input.display_name.as_deref(),
    )
}

async fn passkey_authenticate_begin_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyAuthenticateBeginRequest,
) -> anyhow::Result<PasskeyBeginResponse<RequestOptions>> {
    let ceremony_id = format!("passkey:authenticate:{}", random_hex(16));
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let credential_id = input
        .credential_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let options = manager.begin_authentication_for(&ceremony_id, &rp.id, credential_id)?;
    Ok(PasskeyBeginResponse {
        schema: "elastos.auth.passkey.authenticate.begin/v1".to_string(),
        ceremony_id,
        options,
    })
}

fn passkey_login_accounts(data_dir: &std::path::Path) -> anyhow::Result<Vec<PasskeyLoginAccount>> {
    let mut accounts = crate::auth::active_passkey_principals(data_dir)?
        .into_iter()
        .filter_map(|principal| {
            let passkey = principal.proof_binding.passkey.as_ref()?;
            let display_name = if principal.display_name.trim().is_empty() {
                "Account".to_string()
            } else {
                principal.display_name.clone()
            };
            let role = match principal.role {
                crate::auth::RuntimePrincipalRole::Admin => "admin",
                crate::auth::RuntimePrincipalRole::Guest => "guest",
            };
            let avatar_cid = login_avatar_cid_for_principal(
                data_dir,
                &principal.principal_id,
                &principal.localhost_root,
            );
            Some(PasskeyLoginAccount {
                principal_id: principal.principal_id,
                display_name,
                role: role.to_string(),
                credential_id: passkey.credential_id.clone(),
                last_used_at: passkey.last_used_at,
                avatar_cid,
            })
        })
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| {
        right
            .last_used_at
            .cmp(&left.last_used_at)
            .then_with(|| {
                let left_admin = left.role == "admin";
                let right_admin = right.role == "admin";
                right_admin.cmp(&left_admin)
            })
            .then_with(|| {
                left.display_name
                    .to_lowercase()
                    .cmp(&right.display_name.to_lowercase())
            })
    });
    Ok(accounts)
}

async fn passkey_authenticate_complete_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyAuthenticateCompleteRequest,
) -> anyhow::Result<PasskeyVerifyResponse> {
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let outcome =
        manager.complete_authentication(&input.ceremony_id, &input.response, &rp.id, &rp.origin)?;
    let credential = outcome.credential.clone();
    let origin = outcome.origin.clone();
    let user_verified = outcome.user_verified;
    drop(manager);
    issue_named_passkey_session_grant(
        state,
        &outcome.user_id,
        &credential,
        &origin,
        user_verified,
        "passkey authentication verified and session granted",
        None,
    )
}

async fn require_passkey_registration_allowed(
    state: &GatewayState,
    local_first_owner: bool,
) -> anyhow::Result<()> {
    let manager = state.identity_manager()?;
    let manager = manager.lock().await;
    let registered = manager.status().registered;
    drop(manager);
    if !registered && crate::auth::active_passkey_principal_count(&state.data_dir)? == 0 {
        if local_first_owner {
            return Ok(());
        }
        anyhow::bail!("first owner passkey registration requires local Runtime access");
    }
    if crate::auth::guest_registration_enabled(&state.data_dir)? {
        return Ok(());
    }
    anyhow::bail!("guest passkey registration is disabled")
}

fn local_first_owner_registration(headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    let Some(peer) = peer else {
        return false;
    };
    if !peer.ip().is_loopback() || forwarded_client_is_remote(headers) {
        return false;
    }
    super::handlers::identity::derive_rp(headers)
        .map(|rp| loopback_host(&rp.id))
        .unwrap_or(false)
}

fn forwarded_client_is_remote(headers: &HeaderMap) -> bool {
    if let Some(value) = headers.get("x-forwarded-for") {
        let Ok(value) = value.to_str() else {
            return true;
        };
        return value
            .split(',')
            .map(str::trim)
            .any(|value| parse_forwarded_ip(value).is_none_or(|ip| !ip.is_loopback()));
    }
    let Some(value) = headers.get("forwarded") else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return true;
    };
    let mut found = false;
    for field in value.split(',').flat_map(|entry| entry.split(';')) {
        let Some(value) = field.trim().strip_prefix("for=") else {
            continue;
        };
        found = true;
        if parse_forwarded_ip(value).is_none_or(|ip| !ip.is_loopback()) {
            return true;
        }
    }
    !found
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim_matches('"');
    value
        .parse::<IpAddr>()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

fn loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

async fn evm_challenge_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: EvmChallengeRequest,
) -> anyhow::Result<EvmChallengeResponse> {
    let launch_context = require_wallet_link_context(state, headers)?;
    let context = launch_context.context;
    validate_evm_address(&input.address).map_err(anyhow::Error::msg)?;
    if input.chain_id == 0 {
        anyhow::bail!("chain_id must be non-zero");
    }

    let now = crate::auth::now_ts();
    let domain = request_domain(headers)?;
    let scheme = request_scheme(&domain);
    let uri = format!("{scheme}://{domain}/apps/home/");
    let resources = vec![
        "elastos://wallet/account/link".to_string(),
        format!("elastos://principal/{}", context.principal_id),
    ];
    let data = wallet_provider_data(
        state,
        json!({
            "op": "challenge",
            "domain": domain,
            "uri": uri,
            "address": input.address,
            "chain_id": input.chain_id,
            "resources": resources,
        }),
    )
    .await?;
    let challenge_id = required_string(&data, "challenge_id")?;
    let message = required_string(&data, "message")?;
    let expires_at = required_u64(&data, "expires_at")?;
    let resources = required_string_array(&data, "resources")?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.challenge.created",
            principal_id: Some(context.principal_id),
            session_id: Some(context.session_id),
            challenge_id: Some(challenge_id.clone()),
            result: "ok",
            reason: "EVM wallet-link challenge created",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(EvmChallengeResponse {
        schema: AuthChallengeV1::SCHEMA.to_string(),
        challenge_id,
        message,
        expires_at,
        resources,
    })
}

async fn evm_verify_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: EvmVerifyRequest,
) -> anyhow::Result<EvmVerifyResponse> {
    let launch_context = require_wallet_link_context(state, headers)?;
    let app = launch_context.app;
    let context = launch_context.context;
    let session_proof_binding_id = context
        .proof_binding_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let parsed =
        elastos_runtime::auth::parse_siwe_message(&input.message).map_err(anyhow::Error::msg)?;
    let challenge_id = parsed
        .resources
        .iter()
        .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
        .ok_or_else(|| anyhow::anyhow!("SIWE proof missing challenge resource"))?
        .to_string();
    let now = crate::auth::now_ts();
    let data = match wallet_provider_data(
        state,
        json!({
            "op": "verify_proof",
            "message": &input.message,
            "signature": &input.signature,
        }),
    )
    .await
    {
        Ok(data) => data,
        Err(ecdsa_err) => {
            let network = network_id_for_eip155_chain_id(parsed.chain_id)
                .ok_or_else(|| anyhow::anyhow!("ERC-1271 verification requires a configured chain-provider network for eip155:{}", parsed.chain_id))?;
            let message_hash = format!(
                "0x{}",
                hex::encode(ethereum_signed_message_hash(input.message.as_bytes()))
            );
            let erc1271_proof = chain_provider_data(
                state,
                json!({
                    "op": "erc1271_is_valid_signature",
                    "network": network,
                    "contract": &parsed.address,
                    "message_hash": message_hash,
                    "signature": &input.signature,
                }),
            )
            .await
            .map_err(|chain_err| {
                anyhow::anyhow!(
                    "wallet ECDSA proof failed ({ecdsa_err}); ERC-1271 verification failed ({chain_err})"
                )
            })?;
            wallet_provider_data(
                state,
                json!({
                    "op": "verify_contract_proof",
                    "message": &input.message,
                    "signature": &input.signature,
                    "erc1271_proof": erc1271_proof,
                }),
            )
            .await?
        }
    };
    let proof_binding_id = required_string(&data, "proof_binding_id")?;
    let chain_namespace = required_string(&data, "chain_namespace")?;
    let address = required_string(&data, "address")?;
    let proof_type = required_string(&data, "proof_type")?;
    if proof_type != "siwe" && proof_type != "siwe_erc1271" {
        anyhow::bail!("unsupported wallet proof type");
    }
    let chain_id = chain_namespace
        .strip_prefix("eip155:")
        .ok_or_else(|| anyhow::anyhow!("unsupported wallet proof namespace"))?
        .parse::<u64>()?;
    if chain_id != parsed.chain_id || normalize_evm_address(&address) != parsed.address {
        anyhow::bail!("wallet proof response does not match SIWE message");
    }
    let binding = ProofBinding::evm_account(chain_id, &address, now);
    if binding.id() != proof_binding_id {
        anyhow::bail!("wallet proof binding mismatch");
    }
    if !parsed
        .resources
        .iter()
        .any(|resource| resource == &format!("elastos://principal/{}", context.principal_id))
    {
        anyhow::bail!("wallet proof is not bound to this runtime principal");
    }

    let session =
        crate::auth::load_active_session_grant(&state.data_dir, &context.session_id, now)?;
    if session.principal_id != context.principal_id
        || session.proof_binding_id != session_proof_binding_id
        || session.grant_id != context.grant_id
    {
        anyhow::bail!("home launch token authority context mismatch");
    }
    let session_principal =
        crate::auth::load_principal_for_proof_binding(&state.data_dir, &session_proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&session_principal)?;
    let principal = crate::auth::upsert_principal_for_binding_as_role(
        &state.data_dir,
        binding,
        context.principal_id.clone(),
        session_principal.role,
        now,
    )?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    wallet_provider_data(
        state,
        json!({
            "op": "link_account",
            "principal_id": principal.principal_id.clone(),
            "proof_binding_id": principal.proof_binding_id.clone(),
            "chain_namespace": chain_namespace,
            "address": address,
            "proof_type": proof_type,
            "connector_id": wallet_connector_id_for_wallet_link(&app)?,
        }),
    )
    .await?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.wallet.linked",
            principal_id: Some(context.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            challenge_id: Some(challenge_id),
            result: "ok",
            reason: if proof_type == "siwe_erc1271" {
                "EVM SIWE ERC-1271 proof verified and wallet linked"
            } else {
                "EVM SIWE proof verified and wallet linked"
            },
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    let app_token = super::gateway::issue_home_launch_token_with_context(
        &state.data_dir,
        &app,
        &super::gateway::HomeLaunchTokenContext {
            principal_id: session.principal_id.clone(),
            session_id: session.session_id.clone(),
            proof_binding_id: Some(session.proof_binding_id.clone()),
            grant_id: session.grant_id.clone(),
        },
    )?;
    Ok(EvmVerifyResponse {
        schema: "elastos.auth.evm.verify/v1".to_string(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: session.session_id,
        expires_at: session.expires_at,
        app_token,
    })
}

async fn btc_challenge_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: BtcChallengeRequest,
) -> anyhow::Result<BtcChallengeResponse> {
    let launch_context = require_wallet_link_context(state, headers)?;
    let context = launch_context.context;
    let now = crate::auth::now_ts();
    let domain = request_domain(headers)?;
    let scheme = request_scheme(&domain);
    let uri = format!("{scheme}://{domain}/apps/home/");
    let resources = vec![
        "elastos://wallet/account/link".to_string(),
        format!("elastos://principal/{}", context.principal_id),
    ];
    let data = wallet_provider_data(
        state,
        json!({
            "op": "bitcoin_challenge",
            "domain": domain,
            "uri": uri,
            "address": input.address,
            "network": input.network,
            "resources": resources,
        }),
    )
    .await?;
    let challenge_id = required_string(&data, "challenge_id")?;
    let message = required_string(&data, "message")?;
    let expires_at = required_u64(&data, "expires_at")?;
    let network = required_string(&data, "network")?;
    let address = required_string(&data, "address")?;
    let resources = required_string_array(&data, "resources")?;
    let proof_type = required_string(&data, "proof_type")?;
    if proof_type != "bip322_simple" && proof_type != "bitcoin_signed_message" {
        anyhow::bail!("unsupported Bitcoin wallet proof type");
    }
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.challenge.created",
            principal_id: Some(context.principal_id),
            session_id: Some(context.session_id),
            challenge_id: Some(challenge_id.clone()),
            result: "ok",
            reason: "Bitcoin BIP-322 wallet-link challenge created",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(BtcChallengeResponse {
        schema: "elastos.wallet.bitcoin_challenge/v1".to_string(),
        challenge_id,
        message,
        expires_at,
        network,
        address,
        resources,
        proof_type,
    })
}

async fn btc_verify_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: BtcVerifyRequest,
) -> anyhow::Result<BtcVerifyResponse> {
    let launch_context = require_wallet_link_context(state, headers)?;
    let app = launch_context.app;
    let context = launch_context.context;
    let session_proof_binding_id = context
        .proof_binding_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let challenge_id = bitcoin_challenge_id_from_message(&input.message)?;
    let now = crate::auth::now_ts();
    let data = wallet_provider_data(
        state,
        json!({
            "op": "verify_bip322_proof",
            "message": &input.message,
            "signature": &input.signature,
            "signature_type": input.signature_type.as_deref().unwrap_or("bip322_simple"),
            "public_key": input.public_key,
        }),
    )
    .await?;
    let proof_binding_id = required_string(&data, "proof_binding_id")?;
    let chain_namespace = required_string(&data, "chain_namespace")?;
    let address = required_string(&data, "address")?;
    let proof_type = required_string(&data, "proof_type")?;
    if proof_type != "bip322_simple" && proof_type != "bitcoin_signed_message" {
        anyhow::bail!("unsupported Bitcoin wallet proof type");
    }
    if !chain_namespace.starts_with("bip122:") {
        anyhow::bail!("unsupported Bitcoin wallet proof namespace");
    }
    let subject = format!(
        "{}:{}",
        chain_namespace.trim_start_matches("bip122:"),
        address
    );
    let binding = ProofBinding {
        kind: ProofBindingKind::BtcAddress,
        subject,
        chain_id: None,
        verified_at: now,
        passkey: None,
    };
    if binding.id() != proof_binding_id {
        anyhow::bail!("Bitcoin wallet proof binding mismatch");
    }
    if !bitcoin_message_has_resource(
        &input.message,
        &format!("elastos://principal/{}", context.principal_id),
    ) {
        anyhow::bail!("Bitcoin wallet proof is not bound to this runtime principal");
    }

    let session =
        crate::auth::load_active_session_grant(&state.data_dir, &context.session_id, now)?;
    if session.principal_id != context.principal_id
        || session.proof_binding_id != session_proof_binding_id
        || session.grant_id != context.grant_id
    {
        anyhow::bail!("home launch token authority context mismatch");
    }
    let session_principal =
        crate::auth::load_principal_for_proof_binding(&state.data_dir, &session_proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&session_principal)?;
    let principal = crate::auth::upsert_principal_for_binding_as_role(
        &state.data_dir,
        binding,
        context.principal_id.clone(),
        session_principal.role,
        now,
    )?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    wallet_provider_data(
        state,
        json!({
            "op": "link_account",
            "principal_id": principal.principal_id.clone(),
            "proof_binding_id": principal.proof_binding_id.clone(),
            "chain_namespace": chain_namespace,
            "address": address,
            "proof_type": proof_type,
            "connector_id": wallet_connector_id_for_wallet_link(&app)?,
        }),
    )
    .await?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.wallet.linked",
            principal_id: Some(context.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            challenge_id: Some(challenge_id),
            result: "ok",
            reason: "Bitcoin wallet proof verified and wallet linked",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    let app_token = super::gateway::issue_home_launch_token_with_context(
        &state.data_dir,
        &app,
        &super::gateway::HomeLaunchTokenContext {
            principal_id: session.principal_id.clone(),
            session_id: session.session_id.clone(),
            proof_binding_id: Some(session.proof_binding_id.clone()),
            grant_id: session.grant_id.clone(),
        },
    )?;
    Ok(BtcVerifyResponse {
        schema: "elastos.auth.btc.verify/v1".to_string(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: session.session_id,
        expires_at: session.expires_at,
        app_token,
    })
}

fn bitcoin_challenge_id_from_message(message: &str) -> anyhow::Result<String> {
    message
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("- elastos://auth/bitcoin-challenge/")
        })
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("BIP-322 proof missing challenge resource"))
}

fn bitcoin_message_has_resource(message: &str, resource: &str) -> bool {
    let expected = format!("- {resource}");
    message.lines().any(|line| line.trim() == expected)
}

fn wallet_connector_id_for_wallet_link(app: &str) -> anyhow::Result<&str> {
    if is_wallet_connector_capsule_id(app) {
        return Ok(app);
    }
    anyhow::bail!("wallet linking requires a dedicated wallet connector capsule")
}

pub(crate) async fn wallet_provider_data(
    state: &GatewayState,
    request: Value,
) -> anyhow::Result<Value> {
    provider_data(state, "wallet", request).await
}

async fn chain_provider_data(state: &GatewayState, request: Value) -> anyhow::Result<Value> {
    provider_data(state, "chain", request).await
}

async fn provider_data(
    state: &GatewayState,
    scheme: &str,
    request: Value,
) -> anyhow::Result<Value> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{scheme} provider unavailable"))?;
    let response = registry.send_raw(scheme, &request).await?;
    match response.get("status").and_then(|value| value.as_str()) {
        Some("ok") => Ok(response.get("data").cloned().unwrap_or(Value::Null)),
        Some("error") => {
            let message = response
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("provider returned an error");
            anyhow::bail!("{message}");
        }
        _ => anyhow::bail!("{scheme} provider returned malformed response"),
    }
}

fn network_id_for_eip155_chain_id(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        20 => Some("esc-mainnet"),
        8453 => Some("base-mainnet"),
        _ => None,
    }
}

fn required_string(data: &Value, field: &str) -> anyhow::Result<String> {
    data.get(field)
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("wallet provider response missing {field}"))
}

fn required_u64(data: &Value, field: &str) -> anyhow::Result<u64> {
    data.get(field)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow::anyhow!("wallet provider response missing {field}"))
}

fn required_string_array(data: &Value, field: &str) -> anyhow::Result<Vec<String>> {
    let values = data
        .get(field)
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("wallet provider response missing {field}"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| anyhow::anyhow!("wallet provider response has invalid {field}"))
        })
        .collect()
}

#[cfg(test)]
fn issue_passkey_session_grant(
    state: &GatewayState,
    user_id: &str,
    credential: &StoredCredential,
    origin: &str,
    user_verified: bool,
    reason: &str,
) -> anyhow::Result<PasskeyVerifyResponse> {
    issue_named_passkey_session_grant(
        state,
        user_id,
        credential,
        origin,
        user_verified,
        reason,
        None,
    )
}

fn issue_named_passkey_session_grant(
    state: &GatewayState,
    _user_id: &str,
    credential: &StoredCredential,
    origin: &str,
    user_verified: bool,
    reason: &str,
    display_name: Option<&str>,
) -> anyhow::Result<PasskeyVerifyResponse> {
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
    let principal = crate::auth::upsert_principal_for_binding_as_role_named(
        &state.data_dir,
        binding,
        principal_id,
        role,
        display_name,
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
        expires_at: now.saturating_add(AUTH_SESSION_TTL_SECS),
        apps: vec![
            HOME_CAPSULE_ID.to_string(),
            super::gateway::SYSTEM_CAPSULE_ID.to_string(),
        ],
    };
    crate::auth::store_session_grant(&state.data_dir, grant.clone())?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.session.granted",
            principal_id: Some(grant.principal_id.clone()),
            proof_binding_id: Some(grant.proof_binding_id.clone()),
            session_id: Some(grant.session_id.clone()),
            result: "ok",
            reason,
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;

    let home_token =
        issue_home_launch_token_for_auth_grant(&state.data_dir, HOME_CAPSULE_ID, &grant)?;
    let system_token = issue_home_launch_token_for_auth_grant(
        &state.data_dir,
        super::gateway::SYSTEM_CAPSULE_ID,
        &grant,
    )?;
    Ok(PasskeyVerifyResponse {
        schema: "elastos.auth.passkey.verify/v1".to_string(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: grant.session_id,
        expires_at: grant.expires_at,
        home_token,
        system_token,
    })
}

fn passkey_proof_binding_id(credential: &StoredCredential) -> String {
    ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
        credential_id: credential.credential_id.clone(),
        public_key: credential.public_key.clone(),
        sign_count: credential.sign_count,
        user_verified: true,
        origin: String::new(),
        rp_id: credential.rp_id.clone(),
        created_at: 0,
        last_used_at: 0,
        revoked_at: None,
    })
    .id()
}

fn passkey_verified_response(headers: &HeaderMap, response: PasskeyVerifyResponse) -> Response {
    let secure = super::gateway::request_uses_tls(headers);
    let cookie = home_session_cookie_header_for_token(&response.home_token, secure);
    let mut http_response = Json(response).into_response();
    if let Ok(cookie) = cookie {
        http_response.headers_mut().append(SET_COOKIE, cookie);
    }
    http_response
}

fn auth_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("missing")
        || text.contains("invalid")
        || text.contains("expired")
        || text.contains("mismatch")
        || text.contains("does not match")
        || text.contains("not authorized")
        || text.contains("unsupported")
        || text.contains("unavailable")
        || text.contains("not configured")
        || text.contains("consumed")
        || text.contains("disabled")
        || text.contains("not found")
        || text.contains("not active")
        || text.contains("not a passkey")
        || text.contains("not bound")
        || text.contains("required")
    {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

#[derive(Debug, Default)]
struct AuditEventInput<'a> {
    event_type: &'a str,
    principal_id: Option<String>,
    proof_binding_id: Option<String>,
    session_id: Option<String>,
    challenge_id: Option<String>,
    capsule_id: Option<String>,
    result: &'a str,
    reason: &'a str,
    occurred_at: u64,
}

fn audit_event(input: AuditEventInput<'_>) -> RuntimeAuditEventV1 {
    RuntimeAuditEventV1 {
        schema: RuntimeAuditEventV1::SCHEMA.to_string(),
        event_id: format!("audit:{}", random_hex(16)),
        event_type: input.event_type.to_string(),
        principal_id: input.principal_id,
        proof_binding_id: input.proof_binding_id,
        session_id: input.session_id,
        challenge_id: input.challenge_id,
        capsule_id: input.capsule_id,
        result: input.result.to_string(),
        reason: input.reason.to_string(),
        occurred_at: input.occurred_at,
        signer_did: None,
        signature: None,
    }
}

fn request_domain(headers: &HeaderMap) -> anyhow::Result<String> {
    let value = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(clean_host_header)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    clean_domain(value)
}

fn clean_host_header(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

fn clean_domain(value: String) -> anyhow::Result<String> {
    let value = clean_host_header(&value);
    if value.is_empty()
        || value.contains('/')
        || value.contains('@')
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid SIWE domain");
    }
    Ok(value)
}

fn request_scheme(domain: &str) -> &'static str {
    if is_local_authority(domain) {
        "http"
    } else {
        "https"
    }
}

fn is_local_authority(domain: &str) -> bool {
    let host = domain
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| domain.split(':').next().unwrap_or(domain))
        .to_ascii_lowercase();
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn random_hex(bytes_len: usize) -> String {
    let mut bytes = vec![0u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
    use std::sync::Arc;

    fn test_gateway_state(data_dir: &std::path::Path) -> GatewayState {
        GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: data_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
        }
    }

    async fn did_recovery_test_gateway_state(
        data_dir: &std::path::Path,
        did_provider_valid: bool,
    ) -> GatewayState {
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        registry
            .register_sub_provider(
                "did",
                Arc::new(MockDidRecoveryProvider {
                    valid: did_provider_valid,
                }),
            )
            .await
            .unwrap();
        GatewayState {
            provider_registry: Some(registry),
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: data_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
        }
    }

    struct MockDidRecoveryProvider {
        valid: bool,
    }

    #[async_trait::async_trait]
    impl Provider for MockDidRecoveryProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock DID provider only supports raw requests".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["elastos"]
        }

        fn name(&self) -> &'static str {
            "mock-did-recovery-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            if request.get("op").and_then(|value| value.as_str()) != Some("verify_did_recovery") {
                return Ok(json!({
                    "status": "error",
                    "message": "unsupported DID provider operation"
                }));
            }
            Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.did.recovery-proof/v1",
                    "valid": self.valid,
                    "did": request.get("did").and_then(|value| value.as_str()).unwrap_or_default(),
                    "principal_id": request.get("principal_id").and_then(|value| value.as_str()).unwrap_or_default(),
                    "localhost_root": request.get("localhost_root").and_then(|value| value.as_str()).unwrap_or_default(),
                    "protector_id": request.get("protector_id").and_then(|value| value.as_str()).unwrap_or_default(),
                    "data_key_id": request.get("data_key_id").and_then(|value| value.as_str()).unwrap_or_default(),
                    "verified_at": 1_800_000_010u64,
                }
            }))
        }
    }

    fn test_credential() -> StoredCredential {
        StoredCredential {
            credential_id: "credential-1".to_string(),
            public_key: "public-key".to_string(),
            sign_count: 7,
            rp_id: "elastos.elacitylabs.com".to_string(),
        }
    }

    fn test_credential_2() -> StoredCredential {
        StoredCredential {
            credential_id: "credential-2".to_string(),
            public_key: "public-key-2".to_string(),
            sign_count: 11,
            rp_id: "elastos.elacitylabs.com".to_string(),
        }
    }

    fn store_test_credential(data_dir: &std::path::Path, credential: StoredCredential) {
        let mut store = elastos_identity::IdentityStore::new(data_dir).unwrap();
        store.load().unwrap();
        store.add_credential(credential);
        store.save().unwrap();
    }

    fn copy_test_auth_root(source: &std::path::Path, destination: &std::path::Path) {
        let state = crate::auth::load_auth_state(source).unwrap();
        std::fs::create_dir_all(destination.join("identity")).unwrap();
        std::fs::copy(
            source.join("identity/device.key"),
            destination.join("identity/device.key"),
        )
        .unwrap();
        crate::auth::save_auth_state(destination, &state).unwrap();
    }

    fn home_token_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-elastos-home-token",
            HeaderValue::from_str(token).unwrap(),
        );
        headers
    }

    fn home_session_cookie_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "{}={token}",
                super::super::gateway::HOME_SESSION_COOKIE
            ))
            .unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn first_owner_registration_rejects_public_origin() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:8090"));
        headers.insert(
            "origin",
            HeaderValue::from_static("https://elastos.elacitylabs.com"),
        );

        let response = passkey_register_begin_inner(&state, &headers, false)
            .await
            .unwrap_err();

        assert!(response
            .to_string()
            .contains("first owner passkey registration requires local Runtime access"));
    }

    #[tokio::test]
    async fn first_owner_registration_accepts_local_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        headers.insert("origin", HeaderValue::from_static("http://localhost:61180"));

        let response = passkey_register_begin_inner(&state, &headers, true)
            .await
            .unwrap();

        assert_eq!(response.schema, "elastos.auth.passkey.register.begin/v1");
        assert_eq!(response.options.public_key.rp.id, "localhost");
    }

    #[test]
    fn first_owner_registration_distinguishes_local_and_proxied_clients() {
        let peer = "127.0.0.1:61180".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        headers.insert("origin", HeaderValue::from_static("http://localhost:61180"));
        assert!(local_first_owner_registration(&headers, Some(peer)));

        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.8"));
        assert!(!local_first_owner_registration(&headers, Some(peer)));
        assert!(!local_first_owner_registration(&headers, None));
    }

    #[test]
    fn passkey_session_grant_is_runtime_bound_and_active() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();

        let response = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();

        assert_eq!(response.schema, "elastos.auth.passkey.verify/v1");
        assert!(response
            .proof_binding_id
            .starts_with("proof:passkey:elastos.elacitylabs.com:"));
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &response.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &response.proof_binding_id)
                .unwrap();
        assert_eq!(principal.role, crate::auth::RuntimePrincipalRole::Admin);
        assert!(principal.localhost_root.starts_with("localhost://Users/"));
        assert!(!response.home_token.is_empty());
        assert!(!response.system_token.is_empty());
    }

    #[test]
    fn each_passkey_gets_its_own_principal_root_and_role() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());

        let first = issue_passkey_session_grant(
            &state,
            "same-identity-store-user",
            &test_credential(),
            "https://elastos.elacitylabs.com",
            true,
            "first passkey",
        )
        .unwrap();
        let second = issue_passkey_session_grant(
            &state,
            "same-identity-store-user",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "second passkey",
        )
        .unwrap();

        let first_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &first.proof_binding_id)
                .unwrap();
        let second_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &second.proof_binding_id)
                .unwrap();
        assert_eq!(
            first_principal.role,
            crate::auth::RuntimePrincipalRole::Admin
        );
        assert_eq!(
            second_principal.role,
            crate::auth::RuntimePrincipalRole::Guest
        );
        assert_ne!(first.principal_id, second.principal_id);
        assert_ne!(
            first_principal.localhost_root,
            second_principal.localhost_root
        );
    }

    #[test]
    fn passkey_login_accounts_are_sorted_and_omit_roots() {
        let empty = tempfile::tempdir().unwrap();
        assert!(passkey_login_accounts(empty.path()).unwrap().is_empty());

        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin = issue_named_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential(),
            "https://elastos.elacitylabs.com",
            true,
            "admin passkey",
            Some("Zed Admin"),
        )
        .unwrap();
        let guest = issue_named_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "guest passkey",
            Some("Ada Guest"),
        )
        .unwrap();

        // Equal last_used_at (second resolution) → admin before guest, then name.
        let accounts = passkey_login_accounts(temp.path()).unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].principal_id, admin.principal_id);
        assert_eq!(accounts[0].display_name, "Zed Admin");
        assert_eq!(accounts[0].role, "admin");
        assert_eq!(accounts[0].credential_id, "credential-1");
        assert_eq!(accounts[1].principal_id, guest.principal_id);
        assert_eq!(accounts[1].display_name, "Ada Guest");
        assert_eq!(accounts[1].role, "guest");
        assert_eq!(accounts[1].credential_id, "credential-2");
        assert!(accounts[0].avatar_cid.is_none());
        let encoded = serde_json::to_value(&accounts).unwrap();
        assert!(encoded[0].get("localhost_root").is_none());
        assert!(encoded[0].get("proof_binding_id").is_none());
        assert!(encoded[0].get("avatar_cid").is_none());
    }

    #[tokio::test]
    async fn passkey_list_returns_runtime_bound_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_named_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
            Some("Work laptop"),
        )
        .unwrap();
        let headers = home_token_headers(&grant.home_token);

        let response = passkey_list_inner(&state, &headers).await.unwrap();

        assert_eq!(response.schema, "elastos.auth.passkeys/v1");
        assert_eq!(response.passkeys.len(), 1);
        assert_eq!(
            response.passkeys[0].proof_binding_id,
            grant.proof_binding_id
        );
        assert_eq!(response.passkeys[0].display_name, "Work laptop");
        assert_eq!(response.passkeys[0].rp_id, "elastos.elacitylabs.com");
        assert!(response.passkeys[0].current);
    }

    #[tokio::test]
    async fn guest_passkey_list_is_scoped_to_current_principal() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let admin_list = passkey_list_inner(&state, &home_token_headers(&admin.home_token))
            .await
            .unwrap();
        let guest_list = passkey_list_inner(&state, &home_token_headers(&guest.home_token))
            .await
            .unwrap();

        assert_eq!(admin_list.passkeys.len(), 2);
        assert_eq!(guest_list.passkeys.len(), 1);
        assert_eq!(
            guest_list.passkeys[0].proof_binding_id,
            guest.proof_binding_id
        );
        assert!(guest_list.passkeys[0].current);
    }

    #[tokio::test]
    async fn recovery_status_is_bound_to_current_principal() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let admin_status = recovery_status_inner(&state, &home_token_headers(&admin.home_token))
            .await
            .unwrap();
        let guest_status = recovery_status_inner(&state, &home_token_headers(&guest.home_token))
            .await
            .unwrap();

        assert_eq!(
            admin_status.schema,
            elastos_runtime::auth::PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA
        );
        assert_eq!(admin_status.principal_id, admin.principal_id);
        assert_eq!(guest_status.principal_id, guest.principal_id);
        assert_ne!(admin_status.localhost_root, guest_status.localhost_root);
        assert!(!guest_status.root_encrypted);
        assert!(!guest_status.recovery_configured);
        assert!(guest_status
            .required_actions
            .contains(&"create_recovery_kit".to_string()));
    }

    #[tokio::test]
    async fn recovery_status_reports_matching_root_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        crate::auth::store_principal_root_protection(
            temp.path(),
            root_protection_for(&principal.principal_id, &principal.localhost_root),
        )
        .unwrap();

        let status = recovery_status_inner(&state, &home_token_headers(&grant.home_token))
            .await
            .unwrap();

        assert_eq!(status.principal_id, principal.principal_id);
        assert_eq!(status.localhost_root, principal.localhost_root);
        assert!(status.root_encrypted);
        assert!(status.protection_configured);
        assert!(status.recovery_configured);
        assert!(status.required_actions.is_empty());
    }

    #[tokio::test]
    async fn recovery_status_requires_verified_protector() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let mut protection =
            root_protection_for(&principal.principal_id, &principal.localhost_root);
        for protector in &mut protection.protectors {
            protector.verified_at = None;
        }
        crate::auth::store_principal_root_protection(temp.path(), protection).unwrap();

        let status = recovery_status_inner(&state, &home_token_headers(&grant.home_token))
            .await
            .unwrap();

        assert_eq!(status.principal_id, principal.principal_id);
        assert_eq!(status.localhost_root, principal.localhost_root);
        assert!(status.root_encrypted);
        assert!(status.protection_configured);
        assert!(!status.recovery_configured);
        assert!(status
            .required_actions
            .contains(&"verify_recovery_before_public_guest_hosting".to_string()));
    }

    #[tokio::test]
    async fn recovery_status_ignores_cross_principal_root_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();
        let admin_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &admin.proof_binding_id)
                .unwrap();
        crate::auth::store_principal_root_protection(
            temp.path(),
            root_protection_for(
                &admin_principal.principal_id,
                &admin_principal.localhost_root,
            ),
        )
        .unwrap();

        let guest_status = recovery_status_inner(&state, &home_token_headers(&guest.home_token))
            .await
            .unwrap();

        assert_eq!(guest_status.principal_id, guest.principal_id);
        assert!(!guest_status.root_encrypted);
        assert!(!guest_status.protection_configured);
        assert!(!guest_status.recovery_configured);
    }

    #[tokio::test]
    async fn recovery_status_fails_closed_for_invalid_matching_root_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let mut protection =
            root_protection_for(&principal.principal_id, &principal.localhost_root);
        protection.protectors.clear();
        let mut auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        auth_state.principal_root_protections.push(protection);
        crate::auth::save_auth_state(temp.path(), &auth_state).unwrap();

        let err = recovery_status_inner(&state, &home_token_headers(&grant.home_token))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("at least one protector"));
    }

    #[tokio::test]
    async fn recovery_status_rejects_proofless_session() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let headers = HeaderMap::new();
        let err = recovery_status_inner(&state, &headers)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("home launch token"));
    }

    fn root_protection_for(
        principal_id: &str,
        localhost_root: &str,
    ) -> elastos_runtime::auth::PrincipalRootProtectionV1 {
        elastos_runtime::auth::PrincipalRootProtectionV1 {
            schema: elastos_runtime::auth::PRINCIPAL_ROOT_PROTECTION_SCHEMA.to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            data_key_id: "pdek:abc123".to_string(),
            crypto: elastos_runtime::auth::PrincipalRootCryptoProfileV1::default(),
            protectors: vec![elastos_runtime::auth::PrincipalRootProtectorV1 {
                protector_id: "protector:recovery:abc123".to_string(),
                kind: elastos_runtime::auth::PrincipalRootProtectorKind::RecoveryKit,
                label: "Recovery Kit".to_string(),
                subject: None,
                created_at: 1_800_000_000,
                verified_at: Some(1_800_000_010),
                envelope: Some(elastos_runtime::auth::PrincipalRootProtectorEnvelopeV1 {
                    cipher: "aes-256-gcm".to_string(),
                    kdf: "hkdf-sha256".to_string(),
                    salt: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                    nonce: "AAAAAAAAAAAAAAAA".to_string(),
                    wrapped_data_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                }),
                archive: None,
            }],
            created_at: 1_800_000_000,
            updated_at: 1_800_000_010,
        }
    }

    fn recovery_kit_for(
        principal_id: &str,
        localhost_root: &str,
    ) -> elastos_runtime::auth::RecoveryKitV1 {
        elastos_runtime::auth::RecoveryKitV1 {
            schema: elastos_runtime::auth::RECOVERY_KIT_SCHEMA.to_string(),
            kit_id: "kit:abc123".to_string(),
            protector_id: "protector:recovery:abc123".to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            data_key_id: "pdek:abc123".to_string(),
            recovery_phrase: "aaaa-bbbb-cccc-dddd-eeee-ffff-1111-2222-3333-4444".to_string(),
            salt: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            nonce: "AAAAAAAAAAAAAAAA".to_string(),
            wrapped_data_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            encrypted_root_descriptor: "enc:v1:metadata-ciphertext".to_string(),
            crypto: elastos_runtime::auth::PrincipalRootCryptoProfileV1 {
                recovery_kdf: "hkdf-sha256".to_string(),
                ..elastos_runtime::auth::PrincipalRootCryptoProfileV1::default()
            },
            created_at: 1_800_000_000,
            instructions: vec!["Import through ElastOS Runtime recovery.".to_string()],
        }
    }

    fn did_recovery_subject() -> &'static str {
        "did:key:z6Mkh11111111111111111111111111111111111111111"
    }

    fn did_recovery_proof_for(
        kit: &elastos_runtime::auth::RecoveryKitV1,
    ) -> elastos_runtime::auth::DidRecoveryProofV1 {
        elastos_runtime::auth::DidRecoveryProofV1 {
            schema: "elastos.did.recovery-proof/v1".to_string(),
            did: did_recovery_subject().to_string(),
            principal_id: kit.principal_id.clone(),
            localhost_root: kit.localhost_root.clone(),
            protector_id: "protector:did:abc123".to_string(),
            data_key_id: kit.data_key_id.clone(),
            nonce: "nonce:did-recovery:abc123".to_string(),
            issued_at: 1_800_000_000,
            expires_at: 1_800_000_300,
            signature: "ab".repeat(64),
        }
    }

    fn did_root_protection_for(
        kit: &elastos_runtime::auth::RecoveryKitV1,
    ) -> elastos_runtime::auth::PrincipalRootProtectionV1 {
        elastos_runtime::auth::PrincipalRootProtectionV1 {
            schema: elastos_runtime::auth::PRINCIPAL_ROOT_PROTECTION_SCHEMA.to_string(),
            principal_id: kit.principal_id.clone(),
            localhost_root: kit.localhost_root.clone(),
            data_key_id: kit.data_key_id.clone(),
            crypto: kit.crypto.clone(),
            protectors: vec![elastos_runtime::auth::PrincipalRootProtectorV1 {
                protector_id: "protector:did:abc123".to_string(),
                kind: elastos_runtime::auth::PrincipalRootProtectorKind::DidRecovery,
                label: "Recovery DID".to_string(),
                subject: Some(did_recovery_subject().to_string()),
                created_at: 1_800_000_000,
                verified_at: None,
                envelope: Some(elastos_runtime::auth::PrincipalRootProtectorEnvelopeV1 {
                    cipher: "aes-256-gcm".to_string(),
                    kdf: "hkdf-sha256".to_string(),
                    salt: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                    nonce: "AAAAAAAAAAAAAAAA".to_string(),
                    wrapped_data_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                }),
                archive: None,
            }],
            created_at: 1_800_000_000,
            updated_at: 1_800_000_010,
        }
    }

    #[tokio::test]
    async fn recovery_kit_import_rejects_invalid_material() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let mut kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("test"),
            1_800_000_000,
        )
        .unwrap();
        kit.encrypted_root_descriptor.clear();
        let request = RecoveryKitMaterialImport {
            principal_id: grant.principal_id.clone(),
            localhost_root: principal.localhost_root,
            reassign_to_current_principal: false,
            kit,
            did_recovery_proof: None,
        };

        let err =
            recovery_kit_import_inner(&state, &home_token_headers(&grant.home_token), request)
                .await
                .expect_err("invalid recovery kit material must be rejected")
                .to_string();

        assert!(err.contains("encrypted_root_descriptor"));
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.import.rejected");
        assert_eq!(event.result, "denied");
        assert!(event.reason.contains("encrypted_root_descriptor"));
    }

    #[tokio::test]
    async fn recovery_kit_import_accepts_verified_material_without_prior_protection() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("test"),
            1_800_000_000,
        )
        .unwrap();
        let request = RecoveryKitMaterialImport {
            principal_id: grant.principal_id.clone(),
            localhost_root: principal.localhost_root.clone(),
            reassign_to_current_principal: false,
            kit,
            did_recovery_proof: None,
        };

        let response =
            recovery_kit_import_inner(&state, &home_token_headers(&grant.home_token), request)
                .await
                .unwrap();

        assert_eq!(response.status, "imported");
        let status = recovery_status_inner(&state, &home_token_headers(&grant.home_token))
            .await
            .unwrap();
        assert!(status.root_encrypted);
        assert!(status.recovery_configured);
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.imported");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn recovery_kit_import_consumes_matching_did_recovery_proof() {
        let temp = tempfile::tempdir().unwrap();
        let state = did_recovery_test_gateway_state(temp.path(), true).await;
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("DID protected"),
            1_800_000_000,
        )
        .unwrap();
        crate::auth::store_principal_root_protection(temp.path(), did_root_protection_for(&kit))
            .unwrap();

        let response = recovery_kit_import_inner(
            &state,
            &home_token_headers(&grant.home_token),
            RecoveryKitMaterialImport {
                principal_id: grant.principal_id.clone(),
                localhost_root: principal.localhost_root,
                reassign_to_current_principal: false,
                kit: kit.clone(),
                did_recovery_proof: Some(did_recovery_proof_for(&kit)),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status, "imported");
        let protection = crate::auth::load_principal_root_protection(
            temp.path(),
            &kit.principal_id,
            &kit.localhost_root,
        )
        .unwrap()
        .unwrap();
        assert!(protection.protectors.iter().any(|protector| {
            protector.kind == elastos_runtime::auth::PrincipalRootProtectorKind::RecoveryKit
        }));
        let did = protection
            .protectors
            .iter()
            .find(|protector| {
                protector.kind == elastos_runtime::auth::PrincipalRootProtectorKind::DidRecovery
            })
            .expect("DID recovery protector should be preserved after import");
        assert_eq!(did.subject.as_deref(), Some(did_recovery_subject()));
        assert!(did.verified_at.is_some());
        assert!(did.archive.is_none());
    }

    #[tokio::test]
    async fn recovery_kit_import_rejects_unverified_did_recovery_proof() {
        let temp = tempfile::tempdir().unwrap();
        let state = did_recovery_test_gateway_state(temp.path(), false).await;
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &grant.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("DID protected"),
            1_800_000_000,
        )
        .unwrap();
        crate::auth::store_principal_root_protection(temp.path(), did_root_protection_for(&kit))
            .unwrap();

        let err = recovery_kit_import_inner(
            &state,
            &home_token_headers(&grant.home_token),
            RecoveryKitMaterialImport {
                principal_id: grant.principal_id,
                localhost_root: principal.localhost_root,
                reassign_to_current_principal: false,
                kit: kit.clone(),
                did_recovery_proof: Some(did_recovery_proof_for(&kit)),
            },
        )
        .await
        .expect_err("unverified DID recovery proof must fail closed")
        .to_string();

        assert!(err.contains("DID provider rejected the recovery proof"));
        let protection = crate::auth::load_principal_root_protection(
            temp.path(),
            &kit.principal_id,
            &kit.localhost_root,
        )
        .unwrap()
        .unwrap();
        assert!(protection
            .protectors
            .iter()
            .all(|protector| protector.verified_at.is_none()));
    }

    #[tokio::test]
    async fn recovery_kit_import_reassigns_orphaned_root_to_current_passkey() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let old = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential(),
            "https://elastos.elacitylabs.com",
            true,
            "old passkey grant",
        )
        .unwrap();
        let old_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &old.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &old_principal.principal_id,
            &old_principal.localhost_root,
            Some("orphaned root"),
            1_800_000_000,
        )
        .unwrap();
        crate::auth::revoke_passkey_binding(
            temp.path(),
            &old.proof_binding_id,
            crate::auth::now_ts(),
        )
        .unwrap();
        let current = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "replacement passkey grant",
        )
        .unwrap();
        let current_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &current.proof_binding_id)
                .unwrap();
        assert_ne!(old_principal.principal_id, current_principal.principal_id);
        assert_ne!(
            old_principal.localhost_root,
            current_principal.localhost_root
        );

        let response = recovery_kit_import_inner(
            &state,
            &home_token_headers(&current.home_token),
            RecoveryKitMaterialImport {
                principal_id: current_principal.principal_id.clone(),
                localhost_root: current_principal.localhost_root.clone(),
                reassign_to_current_principal: true,
                kit,
                did_recovery_proof: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status, "reassigned");
        assert_eq!(response.principal_id, old_principal.principal_id);
        assert_eq!(response.localhost_root, old_principal.localhost_root);
        assert_eq!(
            response.previous_principal_id.as_deref(),
            Some(current_principal.principal_id.as_str())
        );
        assert_eq!(
            response.previous_localhost_root.as_deref(),
            Some(current_principal.localhost_root.as_str())
        );
        assert!(response
            .home_token
            .as_deref()
            .is_some_and(|value| !value.is_empty()));
        assert!(response
            .system_token
            .as_deref()
            .is_some_and(|value| !value.is_empty()));
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &current.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        let rebound =
            crate::auth::load_principal_for_proof_binding(temp.path(), &current.proof_binding_id)
                .unwrap();
        assert_eq!(rebound.principal_id, old_principal.principal_id);
        assert_eq!(rebound.localhost_root, old_principal.localhost_root);
        let status = recovery_status_inner(
            &state,
            &home_token_headers(response.home_token.as_ref().unwrap()),
        )
        .await
        .unwrap();
        assert!(status.root_encrypted);
        assert!(status.recovery_configured);
        assert_eq!(status.principal_id, old_principal.principal_id);
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.reassigned");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn recovery_kit_import_reassignment_response_sets_reissued_home_cookie() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let old = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential(),
            "https://elastos.elacitylabs.com",
            true,
            "old passkey grant",
        )
        .unwrap();
        let old_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &old.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &old_principal.principal_id,
            &old_principal.localhost_root,
            Some("orphaned root"),
            1_800_000_000,
        )
        .unwrap();
        crate::auth::revoke_passkey_binding(
            temp.path(),
            &old.proof_binding_id,
            crate::auth::now_ts(),
        )
        .unwrap();
        let current = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "replacement passkey grant",
        )
        .unwrap();
        let current_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &current.proof_binding_id)
                .unwrap();
        let mut headers = home_token_headers(&current.home_token);
        headers.insert("host", HeaderValue::from_static("elastos.elacitylabs.com"));

        let bundle_principal_id = kit.principal_id.clone();
        let bundle_localhost_root = kit.localhost_root.clone();
        let response = full_recovery_bundle_import(
            State(state),
            headers,
            Json(FullRecoveryBundleImportRequest {
                schema: FULL_RECOVERY_BUNDLE_IMPORT_REQUEST_SCHEMA.to_string(),
                principal_id: current_principal.principal_id,
                localhost_root: current_principal.localhost_root,
                reassign_to_current_principal: true,
                bundle: Some(serde_json::json!({
                    "schema": FULL_RECOVERY_BUNDLE_SCHEMA,
                    "bundle_id": "bundle:cookie-test",
                    "principal_id": bundle_principal_id,
                    "localhost_root": bundle_localhost_root,
                    "data_kit": kit,
                    "wallet_recovery_keys": [],
                })),
                package: None,
                password: None,
                did_recovery_proof: None,
            }),
        )
        .await;
        let cookies: Vec<_> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(cookies.iter().any(|value| {
            value.starts_with("home-session=")
                && !value.starts_with("home-session=;")
                && value.contains("Secure")
                && !value.contains(&current.home_token)
        }));
    }

    #[tokio::test]
    async fn recovery_kit_import_reassignment_replaces_active_root_binding() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let active = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential(),
            "https://elastos.elacitylabs.com",
            true,
            "active passkey grant",
        )
        .unwrap();
        let active_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &active.proof_binding_id)
                .unwrap();
        let kit = create_recovery_kit_for_principal(
            &active_principal.principal_id,
            &active_principal.localhost_root,
            Some("active root"),
            1_800_000_000,
        )
        .unwrap();
        let current = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "current passkey grant",
        )
        .unwrap();
        let current_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &current.proof_binding_id)
                .unwrap();

        let response = recovery_kit_import_inner(
            &state,
            &home_token_headers(&current.home_token),
            RecoveryKitMaterialImport {
                principal_id: current_principal.principal_id,
                localhost_root: current_principal.localhost_root,
                reassign_to_current_principal: true,
                kit,
                did_recovery_proof: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status, "reassigned");
        assert_eq!(response.principal_id, active_principal.principal_id);
        assert_eq!(response.localhost_root, active_principal.localhost_root);
        assert!(crate::auth::load_principal_for_proof_binding(
            temp.path(),
            &active.proof_binding_id
        )
        .is_err());
        let recovered =
            crate::auth::load_principal_for_proof_binding(temp.path(), &current.proof_binding_id)
                .unwrap();
        assert_eq!(recovered.principal_id, active_principal.principal_id);
        assert_eq!(recovered.localhost_root, active_principal.localhost_root);
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &active.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &current.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }

    #[tokio::test]
    async fn recovery_kit_import_rejects_cross_principal_material() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();
        let admin_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &admin.proof_binding_id)
                .unwrap();
        let guest_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &guest.proof_binding_id)
                .unwrap();
        let request = RecoveryKitMaterialImport {
            principal_id: guest.principal_id.clone(),
            localhost_root: guest_principal.localhost_root,
            reassign_to_current_principal: false,
            kit: recovery_kit_for(&admin.principal_id, &admin_principal.localhost_root),
            did_recovery_proof: None,
        };

        let err =
            recovery_kit_import_inner(&state, &home_token_headers(&guest.home_token), request)
                .await
                .expect_err("recovery kit material from another principal must be rejected")
                .to_string();

        assert!(err.contains("principal binding mismatch"));
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.import.rejected");
        assert_eq!(
            event.principal_id.as_deref(),
            Some(guest.principal_id.as_str())
        );
        assert_eq!(
            event.proof_binding_id.as_deref(),
            Some(guest.proof_binding_id.as_str())
        );
    }

    #[tokio::test]
    async fn passkey_management_rejects_missing_grant() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let headers = HeaderMap::new();

        let list_err = passkey_list_inner(&state, &headers)
            .await
            .unwrap_err()
            .to_string();
        let refresh_err = refresh_session_inner(&state, &headers)
            .unwrap_err()
            .to_string();

        assert!(list_err.contains("missing home launch token"));
        assert!(refresh_err.contains("missing home launch token"));
    }

    #[tokio::test]
    async fn guest_passkey_registration_is_policy_gated() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let empty_headers = HeaderMap::new();

        let denied = passkey_register_begin_inner(&state, &empty_headers, false)
            .await
            .unwrap_err()
            .to_string();
        let admin_denied =
            passkey_register_begin_inner(&state, &home_token_headers(&grant.home_token), false)
                .await
                .unwrap_err()
                .to_string();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();
        let guest_denied =
            passkey_register_begin_inner(&state, &home_token_headers(&guest.home_token), false)
                .await
                .unwrap_err()
                .to_string();
        crate::auth::set_guest_registration_enabled(temp.path(), true, crate::auth::now_ts())
            .unwrap();
        let public_allowed = passkey_register_begin_inner(&state, &empty_headers, false)
            .await
            .unwrap();

        assert!(denied.contains("guest passkey registration is disabled"));
        assert!(admin_denied.contains("guest passkey registration is disabled"));
        assert!(guest_denied.contains("guest passkey registration is disabled"));
        assert_eq!(
            public_allowed.schema,
            "elastos.auth.passkey.register.begin/v1"
        );
        assert!(public_allowed
            .options
            .public_key
            .exclude_credentials
            .is_empty());
    }

    #[test]
    fn refresh_session_reissues_proof_bound_home_and_system_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let headers = home_token_headers(&grant.home_token);

        let response = refresh_session_inner(&state, &headers).unwrap();

        assert_eq!(response.schema, "elastos.auth.session.refresh/v1");
        assert_eq!(response.principal_id, grant.principal_id);
        assert_eq!(response.proof_binding_id, grant.proof_binding_id);
        assert_eq!(response.session_id, grant.session_id);
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &response.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        super::super::gateway::require_home_launch_token_context(
            temp.path(),
            &home_token_headers(&grant.system_token),
            super::super::gateway::SYSTEM_CAPSULE_ID,
        )
        .expect("an open child token must survive host session renewal");
        assert!(!response.home_token.is_empty());
        assert!(!response.system_token.is_empty());
    }

    #[test]
    fn refresh_session_accepts_http_only_home_cookie() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let headers = home_session_cookie_headers(&grant.home_token);

        let response = refresh_session_inner(&state, &headers).unwrap();

        assert_eq!(response.schema, "elastos.auth.session.refresh/v1");
        assert_eq!(response.principal_id, grant.principal_id);
        assert_eq!(response.session_id, grant.session_id);
        assert!(!response.home_token.is_empty());
    }

    #[test]
    fn refresh_session_uses_trusted_auth_data_dir_for_refreshed_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        copy_test_auth_root(temp.path(), trusted.path());
        let _auth_data_dir =
            super::super::gateway::set_test_home_launch_auth_data_dir(trusted.path());

        let response =
            refresh_session_inner(&state, &home_token_headers(&grant.home_token)).unwrap();
        let refreshed_headers = home_token_headers(&response.home_token);
        let refreshed_context =
            super::super::gateway::require_home_token_context(temp.path(), &refreshed_headers)
                .unwrap();
        let signed_out = sign_out_session_inner(&state, &refreshed_headers).unwrap();

        assert_eq!(refreshed_context.session_id, response.session_id);
        assert_eq!(signed_out.session_id, response.session_id);
        assert!(!crate::auth::is_auth_session_active(
            trusted.path(),
            &grant.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());
        assert!(!crate::auth::is_auth_session_active(
            trusted.path(),
            &response.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());
    }

    #[test]
    fn sign_out_revokes_http_only_home_cookie_session() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let headers = home_session_cookie_headers(&grant.home_token);

        let response = sign_out_session_inner(&state, &headers).unwrap();

        assert_eq!(response.status, "signed_out");
        assert_eq!(response.session_id, grant.session_id);
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &grant.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }

    #[tokio::test]
    async fn revoke_session_uses_trusted_auth_data_dir_for_target_session() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        copy_test_auth_root(temp.path(), trusted.path());
        let _auth_data_dir =
            super::super::gateway::set_test_home_launch_auth_data_dir(trusted.path());

        let response = revoke_session(
            State(state),
            Path(grant.session_id.clone()),
            home_token_headers(&grant.home_token),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!crate::auth::is_auth_session_active(
            trusted.path(),
            &grant.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());
    }

    #[tokio::test]
    async fn sign_out_response_clears_home_cookie() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let mut headers = home_session_cookie_headers(&grant.home_token);
        headers.insert("host", HeaderValue::from_static("elastos.elacitylabs.com"));

        let response = sign_out_session(State(state), headers).await;
        let cookies: Vec<_> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(cookies.iter().any(|value| {
            value.starts_with("home-session=;")
                && value.contains("Max-Age=0")
                && value.contains("Secure")
        }));
    }

    #[tokio::test]
    async fn passkey_revoke_removes_credential_and_revokes_current_session() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let headers = home_token_headers(&grant.home_token);

        let (response, clear_cookie) =
            passkey_revoke_inner(&state, &headers, grant.proof_binding_id.clone())
                .await
                .unwrap();

        assert_eq!(response.status, "revoked");
        assert_eq!(response.proof_binding_id, grant.proof_binding_id);
        assert!(clear_cookie);
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &grant.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        let manager = state.identity_manager().unwrap();
        let manager = manager.lock().await;
        assert!(manager.credentials().is_empty());
    }

    #[tokio::test]
    async fn guest_passkey_cannot_revoke_admin_passkey() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let err = passkey_revoke_inner(
            &state,
            &home_token_headers(&guest.home_token),
            admin.proof_binding_id.clone(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("admin passkey required"));
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &admin.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }

    #[tokio::test]
    async fn admin_can_revoke_guest_passkey_without_revoking_admin_session() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let (response, clear_cookie) = passkey_revoke_inner(
            &state,
            &home_token_headers(&admin.home_token),
            guest.proof_binding_id.clone(),
        )
        .await
        .unwrap();

        assert_eq!(response.status, "revoked");
        assert_eq!(response.proof_binding_id, guest.proof_binding_id);
        assert!(!clear_cookie);
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &admin.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &guest.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        let manager = state.identity_manager().unwrap();
        let manager = manager.lock().await;
        let credentials = manager.credentials();
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0].credential_id, admin_credential.credential_id);
    }

    #[tokio::test]
    async fn admin_can_promote_guest_passkey_to_admin() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let response = passkey_promote_admin_inner(
            &state,
            &home_token_headers(&admin.home_token),
            guest.proof_binding_id.clone(),
        )
        .await
        .unwrap();

        assert_eq!(response.status, "promoted");
        assert_eq!(response.role, "admin");
        assert_eq!(response.proof_binding_id, guest.proof_binding_id);
        let promoted =
            crate::auth::load_principal_for_proof_binding(temp.path(), &guest.proof_binding_id)
                .unwrap();
        assert!(crate::auth::is_admin(&promoted));
        let guest_admin_list = passkey_list_inner(&state, &home_token_headers(&guest.home_token))
            .await
            .unwrap();
        assert_eq!(guest_admin_list.passkeys.len(), 2);
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.passkey.promoted");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn admin_can_demote_another_admin_passkey_to_guest() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let other_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), other_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let other = issue_passkey_session_grant(
            &state,
            "identity-test",
            &other_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test other passkey grant",
        )
        .unwrap();
        passkey_promote_admin_inner(
            &state,
            &home_token_headers(&admin.home_token),
            other.proof_binding_id.clone(),
        )
        .await
        .unwrap();

        let response = passkey_demote_guest_inner(
            &state,
            &home_token_headers(&admin.home_token),
            other.proof_binding_id.clone(),
        )
        .await
        .unwrap();

        assert_eq!(response.status, "demoted");
        assert_eq!(response.role, "guest");
        assert_eq!(response.proof_binding_id, other.proof_binding_id);
        let demoted =
            crate::auth::load_principal_for_proof_binding(temp.path(), &other.proof_binding_id)
                .unwrap();
        assert!(!crate::auth::is_admin(&demoted));
        assert_eq!(
            crate::auth::active_admin_passkey_principal_count(temp.path()).unwrap(),
            1
        );
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.passkey.demoted");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn admin_cannot_demote_self() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let other_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), other_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let other = issue_passkey_session_grant(
            &state,
            "identity-test",
            &other_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test other passkey grant",
        )
        .unwrap();
        passkey_promote_admin_inner(
            &state,
            &home_token_headers(&admin.home_token),
            other.proof_binding_id,
        )
        .await
        .unwrap();

        let err = passkey_demote_guest_inner(
            &state,
            &home_token_headers(&admin.home_token),
            admin.proof_binding_id.clone(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("admin passkey cannot demote itself"));
        let admin_record =
            crate::auth::load_principal_for_proof_binding(temp.path(), &admin.proof_binding_id)
                .unwrap();
        assert!(crate::auth::is_admin(&admin_record));
    }

    #[tokio::test]
    async fn guest_cannot_promote_passkeys_to_admin() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let err = passkey_promote_admin_inner(
            &state,
            &home_token_headers(&guest.home_token),
            admin.proof_binding_id.clone(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("admin passkey required"));
        let admin_record =
            crate::auth::load_principal_for_proof_binding(temp.path(), &admin.proof_binding_id)
                .unwrap();
        assert!(crate::auth::is_admin(&admin_record));
        let guest_record =
            crate::auth::load_principal_for_proof_binding(temp.path(), &guest.proof_binding_id)
                .unwrap();
        assert!(!crate::auth::is_admin(&guest_record));
    }

    #[tokio::test]
    async fn guest_cannot_demote_admin_passkeys() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let err = passkey_demote_guest_inner(
            &state,
            &home_token_headers(&guest.home_token),
            admin.proof_binding_id.clone(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("admin passkey required"));
        let admin_record =
            crate::auth::load_principal_for_proof_binding(temp.path(), &admin.proof_binding_id)
                .unwrap();
        assert!(crate::auth::is_admin(&admin_record));
    }

    #[tokio::test]
    async fn last_admin_passkey_cannot_be_removed_while_guests_remain() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let admin_credential = test_credential();
        let guest_credential = test_credential_2();
        store_test_credential(temp.path(), admin_credential.clone());
        store_test_credential(temp.path(), guest_credential.clone());
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &admin_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test admin passkey grant",
        )
        .unwrap();
        let _guest = issue_passkey_session_grant(
            &state,
            "identity-test",
            &guest_credential,
            "https://elastos.elacitylabs.com",
            true,
            "test guest passkey grant",
        )
        .unwrap();

        let err = passkey_revoke_inner(
            &state,
            &home_token_headers(&admin.home_token),
            admin.proof_binding_id.clone(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("last admin passkey cannot be removed"));
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &admin.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }

    #[test]
    fn revoked_passkey_cannot_mint_new_session_grant() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let grant = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        crate::auth::revoke_passkey_binding(
            temp.path(),
            &grant.proof_binding_id,
            crate::auth::now_ts(),
        )
        .unwrap();

        let err = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("revoked"));
    }
}
