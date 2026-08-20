//! Browser-host adapter for runtime proof-bound authentication.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode};
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
use elastos_wallet_contract::{
    Erc1271ProofEvidenceV1, ManagedRecoveryKeyEntryV1, ManagedRecoverySetV1, PublicNetwork,
    WalletProviderOperationV2, WalletResultV2,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::gateway::{
    consume_passkey_step_up_token, home_session_cookie_header_for_token,
    is_wallet_connector_capsule_id, issue_home_launch_token_for_auth_grant,
    require_home_launch_token_binding, require_runtime_wallet_authority, runtime_wallet_authority,
    GatewayState, RuntimeWalletAdapter, RuntimeWalletAuthority, HOME_CAPSULE_ID,
};

const AUTH_SESSION_TTL_SECS: u64 = 12 * 60 * 60;
const RECOVERY_DESCRIPTOR_SCHEMA: &str = "elastos.principal.root-descriptor/v1";
const FULL_RECOVERY_BUNDLE_SCHEMA: &str = "elastos.full-recovery-bundle/v1";
const FULL_RECOVERY_PEOPLE_IDENTITY_SCHEMA: &str = "elastos.people.recovery-identity/v1";
const FULL_RECOVERY_BUNDLE_EXPORT_REQUEST_SCHEMA: &str =
    "elastos.full-recovery-bundle.export.request/v1";
const FULL_RECOVERY_BUNDLE_IMPORT_REQUEST_SCHEMA: &str =
    "elastos.full-recovery-bundle.import.request/v1";
const FULL_RECOVERY_BUNDLE_PACKAGE_SCHEMA: &str = "elastos.full-recovery-bundle.package/v1";
const FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA: &str =
    "elastos.full-recovery-bundle.import.response/v2";
const FULL_RECOVERY_BUNDLE_AAD_DOMAIN: &str = "elastos.full-recovery-bundle.package.v1";
const FULL_RECOVERY_BUNDLE_KDF_PARAMS: &str = "m=19456,t=2,p=1,len=32";
const FULL_RECOVERY_BUNDLE_SEMANTIC_DIGEST_DOMAIN: &[u8] =
    b"elastos.full-recovery-bundle.semantic.v1";
const WALLET_RESTORE_COMPLETE: &str = "complete";
const WALLET_RESTORE_INCOMPLETE: &str = "incomplete";
const WALLET_RESTORE_REASON_NONE: &str = "none";
const WALLET_RESTORE_REASON_PROVIDER_UNAVAILABLE: &str = "wallet_provider_unavailable";
const WALLET_RESTORE_REASON_PROVIDER_INVALID_RESPONSE: &str = "wallet_provider_invalid_response";
const WALLET_RESTORE_REASON_PROVIDER_REJECTED: &str = "wallet_provider_rejected";
const WALLET_RESTORE_REASON_AUTHORITY_INVALID: &str = "wallet_authority_invalid";
const RUNTIME_AUDIT_COMPLETE: &str = "complete";
const RUNTIME_AUDIT_INCOMPLETE: &str = "incomplete";
const RUNTIME_AUDIT_REASON_NONE: &str = "none";
const RUNTIME_AUDIT_REASON_UNAVAILABLE: &str = "runtime_audit_unavailable";
const RECOVERY_TERMINAL_RETRY_HEADER: &str = "x-elastos-recovery-terminal";
const MAX_RECOVERY_TERMINAL_RETRY_BYTES: usize = 16 * 1024;

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
    pub step_up_token: String,
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
struct FullRecoveryWalletRestoreOutcomeV2 {
    status: &'static str,
    expected_count: usize,
    imported_count: usize,
    reason_code: &'static str,
}

#[derive(Debug, Serialize)]
struct FullRecoveryRuntimeAuditOutcomeV2 {
    status: &'static str,
    reason_code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct FullRecoveryBundleImportResponseV2 {
    schema: &'static str,
    principal_id: String,
    localhost_root: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_principal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_localhost_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    home_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_token: Option<String>,
    wallet_restore: FullRecoveryWalletRestoreOutcomeV2,
    #[serde(skip_serializing_if = "Option::is_none")]
    people_identity_restore: Option<FullRecoveryPeopleIdentityOutcomeV1>,
    runtime_audit: FullRecoveryRuntimeAuditOutcomeV2,
}

/// What happened to the People identity carried by a Full Recovery Bundle.
/// `restored`: the Profile signing seed (and any contact store) is back and
/// the current device is authorized by the recovered chain — accepted
/// contacts survive. `absent`: the bundle predates identity recovery or the
/// account never saved a Profile. `incomplete`: the root recovered but the
/// identity did not; the response says so instead of claiming a complete
/// restore.
#[derive(Debug, Clone, Serialize)]
struct FullRecoveryPeopleIdentityOutcomeV1 {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_did: Option<String>,
    rebound_device: bool,
    contact_store_restored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
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

#[derive(Debug, Serialize)]
pub struct PasskeyVerifyResponse {
    pub schema: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub session_id: String,
    pub expires_at: u64,
    pub home_token: String,
    pub system_token: String,
    pub profile_readiness: super::gateway::ProfileReadinessSummary,
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
            Some(PasskeyLoginAccount {
                principal_id: principal.principal_id,
                display_name,
                role: role.to_string(),
                credential_id: passkey.credential_id.clone(),
                last_used_at: passkey.last_used_at,
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
        Err(err) => principal_root_migration_required_response(&err)
            .unwrap_or_else(|| auth_error_response(err)),
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
        Err(err) => principal_root_migration_required_response(&err)
            .unwrap_or_else(|| auth_error_response(err)),
    }
}

fn principal_root_migration_required_response(err: &anyhow::Error) -> Option<Response> {
    err.downcast_ref::<crate::auth::PrincipalRootMigrationRequiredV1>()
        .map(|outcome| (StatusCode::CONFLICT, Json(outcome.clone())).into_response())
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
) -> Response {
    match passkey_authenticate_begin_inner(&state, &headers).await {
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
    principal_root_recovery_status_for_context(state, &context)
}

pub(in crate::api) fn principal_root_recovery_status_for_context(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
) -> anyhow::Result<PrincipalRootRecoveryStatusV1> {
    let principal = require_active_passkey_principal_for_context(state, context)?;
    principal_root_recovery_status_for_principal(state, &principal)
}

fn principal_root_recovery_status_for_principal(
    state: &GatewayState,
    principal: &crate::auth::PrincipalRecord,
) -> anyhow::Result<PrincipalRootRecoveryStatusV1> {
    let protected_object_inventory =
        principal_root_protected_object_inventory(&state.data_dir, &principal.localhost_root);
    let inspection = crate::auth::inspect_declarative_principal_root_protection(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
        &protected_object_inventory,
    )?;
    let Some(protection) = inspection.protection else {
        let mut status = PrincipalRootRecoveryStatusV1::unprotected(
            principal.principal_id.clone(),
            principal.localhost_root.clone(),
        );
        if inspection.plaintext_object_count > 0 {
            status
                .required_actions
                .insert(0, "migrate_declared_plaintext_objects".to_string());
        }
        return Ok(status);
    };
    let root_encrypted = inspection.plaintext_object_count == 0
        && inspection.encrypted_object_count == inspection.declared_object_count;
    let protection_configured = !protection.protectors.is_empty();
    let recovery_configured = protection
        .protectors
        .iter()
        .any(|protector| protector.verified_at.is_some());
    let recovery_download_available = recovery_archive_from_protection(&protection).is_some();
    let mut required_actions = Vec::new();
    if !root_encrypted {
        required_actions.push("migrate_declared_plaintext_objects".to_string());
    }
    if !recovery_configured {
        required_actions.push("verify_recovery_before_public_guest_hosting".to_string());
    }
    Ok(PrincipalRootRecoveryStatusV1 {
        schema: elastos_runtime::auth::PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA.to_string(),
        principal_id: principal.principal_id.clone(),
        localhost_root: principal.localhost_root.clone(),
        root_encrypted,
        recovery_configured,
        recovery_download_available,
        protection_configured,
        required_actions,
        crypto: protection.crypto,
    })
}

pub fn verify_configured_principal_roots_ready(data_dir: &std::path::Path) -> anyhow::Result<()> {
    let declarations = configured_principal_root_upgrade_declarations(data_dir)?;
    crate::auth::verify_declared_principal_roots_ready(data_dir, &declarations)
}

pub fn migrate_configured_principal_roots_offline(
    data_dir: &std::path::Path,
    backup_dir: &std::path::Path,
) -> anyhow::Result<crate::auth::PrincipalRootUpgradeReceiptV1> {
    crate::auth::migrate_declared_principal_roots_offline(data_dir, backup_dir, || {
        configured_principal_root_upgrade_declarations(data_dir)
    })
}

fn configured_principal_root_upgrade_declarations(
    data_dir: &std::path::Path,
) -> anyhow::Result<Vec<crate::auth::PrincipalRootUpgradeDeclarationV1>> {
    let mut protections = crate::auth::load_auth_state(data_dir)?.principal_root_protections;
    protections.sort_by(|left, right| {
        (&left.principal_id, &left.localhost_root)
            .cmp(&(&right.principal_id, &right.localhost_root))
    });
    Ok(protections
        .into_iter()
        .map(
            |protection| crate::auth::PrincipalRootUpgradeDeclarationV1 {
                inventory: principal_root_protected_object_inventory(
                    data_dir,
                    &protection.localhost_root,
                ),
                principal_id: protection.principal_id,
                localhost_root: protection.localhost_root,
            },
        )
        .collect())
}

pub(crate) fn principal_root_protected_object_inventory(
    data_dir: &std::path::Path,
    localhost_root: &str,
) -> Vec<crate::auth::PrincipalRootProtectedObjectDeclarationV1> {
    let mut inventory = crate::documents::principal_root_protected_object_inventory(localhost_root);
    inventory.extend(crate::library::principal_root_protected_object_inventory(
        localhost_root,
    ));
    inventory.extend(super::gateway::principal_root_protected_object_inventory(
        localhost_root,
    ));
    inventory.extend(
        super::viewer_gateway::principal_root_protected_object_inventory(data_dir, localhost_root),
    );
    // BrowserProfiles are VM lifecycle artifacts, and provider logs are
    // provider-internal state. Neither is a principal-root protected object.
    inventory.sort();
    inventory.dedup();
    inventory
}

/// Synchronous so the activation guard (a mutex guard) never enters the
/// export handler's async state machine. Consumes the step-up, creates or
/// loads the kit, and migrates any first-run plaintext under the same guard.
fn full_recovery_bundle_establish_kit(
    state: &GatewayState,
    launch: &super::gateway::RequiredHomeLaunchToken,
    context: &super::gateway::HomeLaunchTokenContext,
    principal: &crate::auth::PrincipalRecord,
    input: &FullRecoveryBundleExportRequest,
    now: u64,
) -> anyhow::Result<RecoveryKitV1> {
    let protected_object_inventory =
        principal_root_protected_object_inventory(&state.data_dir, &principal.localhost_root);
    let protection_activation =
        crate::auth::begin_declarative_principal_root_protection_activation_migrating_plaintext(
            &state.data_dir,
            &principal.principal_id,
            &principal.localhost_root,
            &protected_object_inventory,
        )?;
    // Same refusal, ahead of the same first write: a Recovery Kit export
    // must not be the thing that leaves a root protected with plaintext
    // beside it. Checked before the step-up is consumed so a refusal costs
    // the person nothing but a message.
    crate::auth::ensure_online_plaintext_migration_is_possible(
        &protection_activation.plaintext_objects,
    )?;
    consume_passkey_step_up_token(
        &state.data_dir,
        &input.step_up_token,
        launch,
        180,
        "auth.full-recovery-bundle.export",
        &serde_json::json!({
            "principal_id": input.principal_id,
            "localhost_root": input.localhost_root,
            "label": input.label,
            "download_password": input.download_password,
        }),
    )?;
    let kit = recovery_kit_get_or_create_for_principal(
        state,
        context,
        principal,
        input.label.as_deref(),
        RecoveryKitDelivery::RetainedUnseen,
        now,
    )?;
    if !protection_activation.plaintext_objects.is_empty() {
        let migrated = crate::auth::migrate_principal_root_plaintext_objects_under_activation(
            &protection_activation.guard,
            &state.data_dir,
            &principal.principal_id,
            &principal.localhost_root,
            protection_activation.plaintext_objects.clone(),
        )?;
        let _ = crate::auth::append_audit_event(
            &state.data_dir,
            audit_event(AuditEventInput {
                event_type: "auth.principal_root.plaintext_migrated",
                principal_id: Some(principal.principal_id.clone()),
                proof_binding_id: Some(principal.proof_binding_id.clone()),
                session_id: Some(context.session_id.clone()),
                result: "ok",
                reason: "declared plaintext objects migrated during recovery-kit activation",
                occurred_at: now,
                ..AuditEventInput::default()
            }),
        );
        tracing::info!(
            principal_id = %principal.principal_id,
            object_count = migrated.object_count,
            "migrated declared plaintext principal-root objects during recovery activation"
        );
    }
    Ok(kit)
}

async fn full_recovery_bundle_export_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: FullRecoveryBundleExportRequest,
) -> anyhow::Result<Value> {
    if input.schema != FULL_RECOVERY_BUNDLE_EXPORT_REQUEST_SCHEMA {
        anyhow::bail!("unsupported full recovery bundle export request schema");
    }
    let launch = require_home_launch_token_binding(
        &state.data_dir,
        headers,
        &[HOME_CAPSULE_ID, super::gateway::SYSTEM_CAPSULE_ID],
    )?;
    if launch.context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    let wallet_authority = runtime_wallet_authority(&launch)?;
    let context = launch.context.clone();
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
    let kit =
        full_recovery_bundle_establish_kit(state, &launch, &context, &principal, &input, now)?;
    let wallet_recovery_set = export_managed_recovery_set(state, &wallet_authority).await?;
    let wallet_recovery_keys = full_bundle_wallet_recovery_keys(wallet_recovery_set)?;
    let wallet_recovery_key_count = wallet_recovery_keys.len();
    let people_identity = people_identity_for_full_bundle(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )?;
    let people_identity_included = people_identity.is_some();
    let mut bundle = json!({
        "schema": FULL_RECOVERY_BUNDLE_SCHEMA,
        "bundle_id": format!("bundle:{}", random_hex(16)),
        "principal_id": principal.principal_id.clone(),
        "localhost_root": principal.localhost_root.clone(),
        "data_kit": kit,
        "wallet_recovery_keys": wallet_recovery_keys,
        "included": {
            "data_kit": true,
            "wallet_recovery_key_count": wallet_recovery_key_count,
            "people_identity": people_identity_included
        },
        "created_at": now,
        "instructions": [
            "Keep this Full Recovery Bundle offline. Anyone with it can recover this ElastOS user root, included built-in Wallet accounts, and the signed People identity with its contacts.",
            "Import it only through ElastOS System recovery on a runtime you control."
        ]
    });
    if let Some(people_identity) = people_identity {
        bundle
            .as_object_mut()
            .expect("full recovery bundle is an object")
            .insert("people_identity".to_string(), people_identity);
    }
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
    mark_recovery_kit_handed_to_person(state, &principal, &kit, now)?;
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
    let launch = require_home_launch_token_binding(
        &state.data_dir,
        headers,
        &[HOME_CAPSULE_ID, super::gateway::SYSTEM_CAPSULE_ID],
    )?;
    if launch.context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    let context = launch.context.clone();
    let verified_actor = launch.launch_context.executable_actor.clone();
    let pre_recovery_wallet_authority = runtime_wallet_authority(&launch)?;
    let bundle = full_recovery_bundle_from_import_request(&input)?;
    validate_full_recovery_bundle(&bundle)?;
    let recovery_set = managed_recovery_set_from_full_bundle(&bundle)?;
    let data_kit: RecoveryKitV1 = serde_json::from_value(
        bundle
            .get("data_kit")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing data_kit"))?,
    )?;
    crate::auth::verify_recovery_kit_material(&data_kit)?;
    let bundle_sha256 = full_recovery_bundle_semantic_digest(&bundle)?;
    let bundle_id = bundle
        .get("bundle_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("full recovery bundle missing bundle_id"))?;
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
    let expected_wallet_count = recovery_set.keys.len();
    let completed_audit_id = full_recovery_outcome_audit_id(
        bundle_id,
        bundle_principal,
        WALLET_RESTORE_COMPLETE,
        WALLET_RESTORE_REASON_NONE,
        expected_wallet_count,
        expected_wallet_count,
        &bundle_sha256,
    );
    if let Some(terminal_event) = recovery_terminal_retry_event(&state.data_dir, headers)? {
        if input.reassign_to_current_principal {
            anyhow::bail!("Recovery terminal retry cannot reassign a principal root");
        }
        validate_recovery_terminal_retry_event(
            &terminal_event,
            &completed_audit_id,
            bundle_principal,
            context.proof_binding_id.as_deref(),
            &context.session_id,
            expected_wallet_count,
            &bundle_sha256,
        )?;
        let audit_result = crate::auth::append_signed_full_recovery_outcome_audit_event(
            &state.data_dir,
            terminal_event.clone(),
        );
        let runtime_audit = match audit_result {
            Ok(()) => FullRecoveryRuntimeAuditOutcomeV2 {
                status: RUNTIME_AUDIT_COMPLETE,
                reason_code: RUNTIME_AUDIT_REASON_NONE,
                retry_token: None,
            },
            Err(_) => FullRecoveryRuntimeAuditOutcomeV2 {
                status: RUNTIME_AUDIT_INCOMPLETE,
                reason_code: RUNTIME_AUDIT_REASON_UNAVAILABLE,
                retry_token: Some(encode_recovery_terminal_retry_event(&terminal_event)?),
            },
        };
        return Ok(serde_json::to_value(FullRecoveryBundleImportResponseV2 {
            schema: FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA,
            principal_id: bundle_principal.to_string(),
            localhost_root: bundle_root.to_string(),
            status: "imported".to_string(),
            previous_principal_id: None,
            previous_localhost_root: None,
            home_token: None,
            system_token: None,
            wallet_restore: FullRecoveryWalletRestoreOutcomeV2 {
                status: WALLET_RESTORE_COMPLETE,
                expected_count: expected_wallet_count,
                imported_count: expected_wallet_count,
                reason_code: WALLET_RESTORE_REASON_NONE,
            },
            people_identity_restore: None,
            runtime_audit,
        })?);
    }
    if !input.reassign_to_current_principal {
        if let Some(completed_event) = crate::auth::load_signed_full_recovery_outcome_audit_event(
            &state.data_dir,
            &completed_audit_id,
        )? {
            validate_recovery_terminal_retry_event(
                &completed_event,
                &completed_audit_id,
                bundle_principal,
                context.proof_binding_id.as_deref(),
                &context.session_id,
                expected_wallet_count,
                &bundle_sha256,
            )?;
            return Ok(serde_json::to_value(FullRecoveryBundleImportResponseV2 {
                schema: FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA,
                principal_id: bundle_principal.to_string(),
                localhost_root: bundle_root.to_string(),
                status: "imported".to_string(),
                previous_principal_id: None,
                previous_localhost_root: None,
                home_token: None,
                system_token: None,
                wallet_restore: FullRecoveryWalletRestoreOutcomeV2 {
                    status: WALLET_RESTORE_COMPLETE,
                    expected_count: expected_wallet_count,
                    imported_count: expected_wallet_count,
                    reason_code: WALLET_RESTORE_REASON_NONE,
                },
                people_identity_restore: None,
                runtime_audit: FullRecoveryRuntimeAuditOutcomeV2 {
                    status: RUNTIME_AUDIT_COMPLETE,
                    reason_code: RUNTIME_AUDIT_REASON_NONE,
                    retry_token: None,
                },
            })?);
        }
    }
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
    let restored_proof_binding_id = context
        .proof_binding_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let people_identity_restore = restore_people_identity_from_full_bundle(
        &state.data_dir,
        &bundle,
        &recovery_response.principal_id,
        &recovery_response.localhost_root,
        &restored_proof_binding_id,
    );
    let wallet_authority = if recovery_response.status == "reassigned" {
        replacement_wallet_authority(
            &state.data_dir,
            headers,
            &verified_actor,
            context.proof_binding_id.as_deref(),
            &recovery_response,
        )
    } else {
        Ok(pre_recovery_wallet_authority)
    };
    let (wallet_restore, audit_proof_binding_id, audit_session_id) = match wallet_authority {
        Ok(authority) => {
            let audit_proof_binding_id = authority
                .verified_context()
                .proof_binding_id()
                .map(ToString::to_string);
            let audit_session_id = authority.verified_context().session_id().to_string();
            let wallet_restore =
                restore_managed_recovery_set(state, &authority, recovery_set).await;
            (wallet_restore, audit_proof_binding_id, audit_session_id)
        }
        Err(_) => (
            incomplete_wallet_restore(
                expected_wallet_count,
                WALLET_RESTORE_REASON_AUTHORITY_INVALID,
            ),
            context.proof_binding_id.clone(),
            context.session_id.clone(),
        ),
    };
    let (event_type, result) = if wallet_restore.status == WALLET_RESTORE_COMPLETE
        && people_identity_restore.status != "incomplete"
    {
        ("auth.full_recovery_bundle.imported", "ok")
    } else {
        ("auth.full_recovery_bundle.import_incomplete", "incomplete")
    };
    let audit_reason = full_recovery_outcome_audit_reason(
        &recovery_response.status,
        wallet_restore.status,
        wallet_restore.reason_code,
        wallet_restore.expected_count,
        wallet_restore.imported_count,
        &bundle_sha256,
    );
    let mut outcome_event = audit_event(AuditEventInput {
        event_type,
        principal_id: Some(recovery_response.principal_id.clone()),
        proof_binding_id: audit_proof_binding_id,
        session_id: Some(audit_session_id),
        result,
        reason: &audit_reason,
        occurred_at: crate::auth::now_ts(),
        ..AuditEventInput::default()
    });
    outcome_event.event_id = full_recovery_outcome_audit_id(
        bundle_id,
        &recovery_response.principal_id,
        wallet_restore.status,
        wallet_restore.reason_code,
        wallet_restore.expected_count,
        wallet_restore.imported_count,
        &bundle_sha256,
    );
    let signed_outcome_event = crate::auth::sign_audit_event(&state.data_dir, outcome_event)?;
    #[cfg(test)]
    let audit_result = crate::auth::consume_recovery_reassignment_test_fault(
        &state.data_dir,
        crate::auth::RecoveryReassignmentTestFault::PostCommitOutcomeAudit,
    )
    .and_then(|()| {
        crate::auth::append_signed_full_recovery_outcome_audit_event(
            &state.data_dir,
            signed_outcome_event.clone(),
        )
    });
    #[cfg(not(test))]
    let audit_result = crate::auth::append_signed_full_recovery_outcome_audit_event(
        &state.data_dir,
        signed_outcome_event.clone(),
    );
    let runtime_audit = match audit_result {
        Ok(()) => FullRecoveryRuntimeAuditOutcomeV2 {
            status: RUNTIME_AUDIT_COMPLETE,
            reason_code: RUNTIME_AUDIT_REASON_NONE,
            retry_token: None,
        },
        Err(_) => FullRecoveryRuntimeAuditOutcomeV2 {
            status: RUNTIME_AUDIT_INCOMPLETE,
            reason_code: RUNTIME_AUDIT_REASON_UNAVAILABLE,
            retry_token: (wallet_restore.status == WALLET_RESTORE_COMPLETE)
                .then(|| encode_recovery_terminal_retry_event(&signed_outcome_event))
                .transpose()?,
        },
    };
    Ok(serde_json::to_value(FullRecoveryBundleImportResponseV2 {
        schema: FULL_RECOVERY_BUNDLE_IMPORT_RESPONSE_SCHEMA,
        principal_id: recovery_response.principal_id,
        localhost_root: recovery_response.localhost_root,
        status: recovery_response.status,
        previous_principal_id: recovery_response.previous_principal_id,
        previous_localhost_root: recovery_response.previous_localhost_root,
        home_token: recovery_response.home_token,
        system_token: recovery_response.system_token,
        wallet_restore,
        people_identity_restore: Some(people_identity_restore),
        runtime_audit,
    })?)
}

fn full_recovery_bundle_from_import_request(
    input: &FullRecoveryBundleImportRequest,
) -> anyhow::Result<Value> {
    if input.bundle.is_some() == input.package.is_some() {
        anyhow::bail!("import exactly one full recovery bundle or package");
    }
    if let Some(bundle) = input.bundle.as_ref() {
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

/// Did this Recovery Kit reach the person, or was it only minted so their
/// root could be protected? A protector records `verified_at` to mean
/// someone holds the phrase off this machine. Protecting a root on their
/// behalf is worth doing — it is what makes a Profile possible — but it
/// proves nothing about what they hold, and recording it as verified would
/// tell them, and the guest-hosting gate, that recovery is handled when
/// nobody has ever seen the phrase.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryKitDelivery {
    HandedToPerson,
    RetainedUnseen,
}

fn recovery_kit_get_or_create_for_principal(
    state: &GatewayState,
    context: &super::gateway::HomeLaunchTokenContext,
    principal: &crate::auth::PrincipalRecord,
    label: Option<&str>,
    delivery: RecoveryKitDelivery,
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
    let protection = protection_from_recovery_kit(&kit, label, delivery, now, Some(archive))?;
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

fn mark_recovery_kit_handed_to_person(
    state: &GatewayState,
    principal: &crate::auth::PrincipalRecord,
    kit: &RecoveryKitV1,
    now: u64,
) -> anyhow::Result<()> {
    crate::auth::verify_recovery_kit_material(kit)?;
    if kit.principal_id != principal.principal_id || kit.localhost_root != principal.localhost_root
    {
        anyhow::bail!("recovery kit principal binding mismatch");
    }
    let mut protection = crate::auth::load_principal_root_protection(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )?
    .ok_or_else(|| anyhow::anyhow!("principal root protection is missing"))?;
    if protection.data_key_id != kit.data_key_id || protection.crypto != kit.crypto {
        anyhow::bail!("recovery kit protection binding mismatch");
    }
    let protector = protection
        .protectors
        .iter_mut()
        .find(|protector| protector.protector_id == kit.protector_id)
        .ok_or_else(|| anyhow::anyhow!("recovery kit protector is missing"))?;
    if protector.kind != PrincipalRootProtectorKind::RecoveryKit {
        anyhow::bail!("recovery kit protector kind mismatch");
    }
    let archived_kit = crate::auth::recovery_kit_from_archive(
        &state.data_dir,
        protector
            .archive
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("recovery kit archive is missing"))?,
    )?;
    crate::auth::verify_recovery_kit_material(&archived_kit)?;
    if archived_kit != *kit {
        anyhow::bail!("recovery kit archive binding mismatch");
    }
    if protector.verified_at.is_some() {
        return Ok(());
    }
    protector.verified_at = Some(now);
    protection.updated_at = now.max(protection.updated_at);
    crate::auth::store_principal_root_protection(&state.data_dir, protection)
}

/// The People identity a Full Recovery Bundle carries: the decrypted profile
/// authority bundle (signing seed, retained revision ring, signed head) plus
/// the signed contact-store state when one exists. `None` when this account
/// has no saved Profile — a contact store without its Profile would be
/// unusable, so it never travels alone.
fn people_identity_for_full_bundle(
    data_dir: &std::path::Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<Value>> {
    let Some(profile_authority_bundle) =
        crate::collaboration_profile_authority::export_profile_authority_bundle_for_recovery(
            data_dir,
            principal_id,
            localhost_root,
        )?
    else {
        return Ok(None);
    };
    let mut people_identity = json!({
        "schema": FULL_RECOVERY_PEOPLE_IDENTITY_SCHEMA,
        "profile_authority_bundle": profile_authority_bundle,
    });
    if let Some(contact_store_state) =
        crate::collaboration_contact_store::export_contact_store_state_for_recovery(
            data_dir,
            principal_id,
            localhost_root,
        )?
    {
        people_identity
            .as_object_mut()
            .expect("people identity is an object")
            .insert("contact_store_state".to_string(), contact_store_state);
    }
    Ok(Some(people_identity))
}

/// Restores the People identity after the root itself recovered: writes the
/// profile authority bundle and contact store back under the restored
/// protected root, then — when the recovered head does not authorize this
/// machine's device — signs the next revision through the normal Profile
/// authority path so the existing update-delivery chain announces the new
/// device to every accepted contact. Failure never claims completeness: the
/// outcome says exactly what happened.
fn restore_people_identity_from_full_bundle(
    data_dir: &std::path::Path,
    bundle: &Value,
    principal_id: &str,
    localhost_root: &str,
    proof_binding_id: &str,
) -> FullRecoveryPeopleIdentityOutcomeV1 {
    let Some(people_identity) = bundle.get("people_identity") else {
        return FullRecoveryPeopleIdentityOutcomeV1 {
            status: "absent",
            profile_did: None,
            rebound_device: false,
            contact_store_restored: false,
            reason: None,
        };
    };
    match restore_people_identity_inner(
        data_dir,
        people_identity,
        principal_id,
        localhost_root,
        proof_binding_id,
    ) {
        Ok(outcome) => outcome,
        Err(err) => FullRecoveryPeopleIdentityOutcomeV1 {
            status: "incomplete",
            profile_did: None,
            rebound_device: false,
            contact_store_restored: false,
            reason: Some(format!("{err:#}")),
        },
    }
}

fn restore_people_identity_inner(
    data_dir: &std::path::Path,
    people_identity: &Value,
    principal_id: &str,
    localhost_root: &str,
    proof_binding_id: &str,
) -> anyhow::Result<FullRecoveryPeopleIdentityOutcomeV1> {
    if people_identity.get("schema").and_then(Value::as_str)
        != Some(FULL_RECOVERY_PEOPLE_IDENTITY_SCHEMA)
    {
        anyhow::bail!("unsupported People identity schema in full recovery bundle");
    }
    let profile_bundle = people_identity
        .get("profile_authority_bundle")
        .ok_or_else(|| anyhow::anyhow!("People identity missing profile authority bundle"))?;
    let restored =
        crate::collaboration_profile_authority::restore_profile_authority_bundle_for_recovery(
            data_dir,
            principal_id,
            localhost_root,
            profile_bundle,
        )?;
    let profile_did = restored.document().profile_did.clone();
    let contact_store_restored = match people_identity.get("contact_store_state") {
        Some(state) => {
            crate::collaboration_contact_store::restore_contact_store_state_for_recovery(
                data_dir,
                principal_id,
                localhost_root,
                state,
                &profile_did,
            )?;
            true
        }
        None => false,
    };
    let (_, device_did) = elastos_identity::load_or_create_did(data_dir)?;
    let rebound_device = if restored.authorizes_endpoint(&device_did) {
        false
    } else {
        let head = restored.document();
        crate::collaboration_profile_authority::update_profile_authority(
            data_dir,
            principal_id,
            localhost_root,
            proof_binding_id,
            &head.display_name,
            head.handle.as_deref(),
            crate::auth::now_ts(),
        )?;
        true
    };
    Ok(FullRecoveryPeopleIdentityOutcomeV1 {
        status: "restored",
        profile_did: Some(profile_did),
        rebound_device,
        contact_store_restored,
        reason: None,
    })
}

async fn export_managed_recovery_set(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
) -> anyhow::Result<ManagedRecoverySetV1> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("wallet provider unavailable"))?;
    let response = RuntimeWalletAdapter::new(registry, authority)
        .invoke(WalletProviderOperationV2::ExportManagedRecoverySet {})
        .await?;
    let data = match response.result {
        WalletResultV2::Ok { data } => data,
        WalletResultV2::Error { message, .. } => anyhow::bail!(message),
    };
    let recovery_set: ManagedRecoverySetV1 = serde_json::from_value(data)
        .map_err(|err| anyhow::anyhow!("invalid managed recovery set response: {err}"))?;
    recovery_set
        .validate()
        .map_err(|err| anyhow::anyhow!("invalid managed recovery set response: {err}"))?;
    Ok(recovery_set)
}

fn full_bundle_wallet_recovery_keys(
    recovery_set: ManagedRecoverySetV1,
) -> anyhow::Result<Vec<Value>> {
    recovery_set
        .keys
        .into_iter()
        .map(|entry| {
            let mut recovery_key = entry.recovery_key;
            let object = recovery_key
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("managed recovery key must be an object"))?;
            object.insert("account_id".to_string(), json!(entry.account_id));
            if let Some(label) = entry.label {
                object.insert("label".to_string(), json!(label));
            }
            Ok(recovery_key)
        })
        .collect()
}

fn managed_recovery_set_from_full_bundle(bundle: &Value) -> anyhow::Result<ManagedRecoverySetV1> {
    let keys = bundle
        .get("wallet_recovery_keys")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("full recovery bundle wallet_recovery_keys must be an array")
        })?
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or_else(|| {
                anyhow::anyhow!("full recovery bundle Wallet recovery key must be an object")
            })?;
            let account_id = object
                .get("account_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!("full recovery bundle Wallet recovery key missing account_id")
                })?
                .to_string();
            let label = match object.get("label") {
                None | Some(Value::Null) => None,
                Some(Value::String(label)) => Some(label.clone()),
                Some(_) => {
                    anyhow::bail!("full recovery bundle Wallet recovery key label must be text")
                }
            };
            let mut recovery_key = value.clone();
            recovery_key
                .as_object_mut()
                .expect("Wallet recovery key object checked above")
                .remove("label");
            Ok(ManagedRecoveryKeyEntryV1 {
                account_id,
                recovery_key,
                label,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ManagedRecoverySetV1::new(keys).map_err(|err| {
        anyhow::anyhow!("invalid managed recovery set in full recovery bundle: {err}")
    })
}

fn replacement_wallet_authority(
    data_dir: &std::path::Path,
    request_headers: &HeaderMap,
    verified_actor: &str,
    expected_proof_binding_id: Option<&str>,
    recovery_response: &RecoveryKitImportResponse,
) -> anyhow::Result<RuntimeWalletAuthority> {
    let token = match verified_actor {
        HOME_CAPSULE_ID => recovery_response.home_token.as_deref(),
        super::gateway::SYSTEM_CAPSULE_ID => recovery_response.system_token.as_deref(),
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("recovery response missing replacement shell token"))?;
    let mut headers = request_headers.clone();
    headers.insert("x-elastos-home-token", HeaderValue::from_str(token)?);
    let authority = require_runtime_wallet_authority(data_dir, &headers, &[verified_actor])?;
    let verified = authority.verified_context();
    if verified.actor() != verified_actor
        || verified.principal_id() != recovery_response.principal_id
        || verified.proof_binding_id() != expected_proof_binding_id
    {
        anyhow::bail!("replacement shell token Wallet authority binding mismatch");
    }
    Ok(authority)
}

async fn restore_managed_recovery_set(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    recovery_set: ManagedRecoverySetV1,
) -> FullRecoveryWalletRestoreOutcomeV2 {
    let expected_count = recovery_set.keys.len();
    let Some(registry) = state.provider_registry.as_ref() else {
        return incomplete_wallet_restore(
            expected_count,
            WALLET_RESTORE_REASON_PROVIDER_UNAVAILABLE,
        );
    };
    let response = match RuntimeWalletAdapter::new(registry, authority)
        .invoke(WalletProviderOperationV2::ImportManagedRecoverySet { recovery_set })
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let reason_code = if wallet_adapter_error_is_invalid_response(&err) {
                WALLET_RESTORE_REASON_PROVIDER_INVALID_RESPONSE
            } else {
                WALLET_RESTORE_REASON_PROVIDER_UNAVAILABLE
            };
            return incomplete_wallet_restore(expected_count, reason_code);
        }
    };
    let data = match response.result {
        WalletResultV2::Ok { data } => data,
        WalletResultV2::Error { .. } => {
            return incomplete_wallet_restore(
                expected_count,
                WALLET_RESTORE_REASON_PROVIDER_REJECTED,
            )
        }
    };
    let response_is_complete = data.get("imported").and_then(Value::as_bool) == Some(true)
        && data.get("account_count").and_then(Value::as_u64) == u64::try_from(expected_count).ok()
        && data
            .get("accounts")
            .and_then(Value::as_array)
            .is_some_and(|accounts| accounts.len() == expected_count);
    if !response_is_complete {
        return incomplete_wallet_restore(
            expected_count,
            WALLET_RESTORE_REASON_PROVIDER_INVALID_RESPONSE,
        );
    }
    FullRecoveryWalletRestoreOutcomeV2 {
        status: WALLET_RESTORE_COMPLETE,
        expected_count,
        imported_count: expected_count,
        reason_code: WALLET_RESTORE_REASON_NONE,
    }
}

fn incomplete_wallet_restore(
    expected_count: usize,
    reason_code: &'static str,
) -> FullRecoveryWalletRestoreOutcomeV2 {
    FullRecoveryWalletRestoreOutcomeV2 {
        status: WALLET_RESTORE_INCOMPLETE,
        expected_count,
        imported_count: 0,
        reason_code,
    }
}

fn wallet_adapter_error_is_invalid_response(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("response is missing")
        || message.contains("invalid Wallet provider v2 response")
}

fn full_recovery_outcome_audit_id(
    bundle_id: &str,
    principal_id: &str,
    wallet_status: &str,
    reason_code: &str,
    expected_count: usize,
    imported_count: usize,
    bundle_sha256: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"elastos.full-recovery-bundle.outcome-audit.v1");
    let expected_count = expected_count.to_string();
    let imported_count = imported_count.to_string();
    for value in [
        bundle_id.as_bytes(),
        principal_id.as_bytes(),
        wallet_status.as_bytes(),
        reason_code.as_bytes(),
        expected_count.as_bytes(),
        imported_count.as_bytes(),
        bundle_sha256.as_bytes(),
    ] {
        digest.update([0]);
        digest.update(value);
    }
    format!("audit:full-recovery:{}", hex::encode(digest.finalize()))
}

fn full_recovery_bundle_semantic_digest(bundle: &Value) -> anyhow::Result<String> {
    let canonical = canonical_recovery_json(bundle);
    let mut digest = Sha256::new();
    digest.update(FULL_RECOVERY_BUNDLE_SEMANTIC_DIGEST_DOMAIN);
    digest.update([0]);
    digest.update(serde_json::to_vec(&canonical)?);
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn canonical_recovery_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_recovery_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_recovery_json).collect()),
        _ => value.clone(),
    }
}

fn full_recovery_outcome_audit_reason(
    root_status: &str,
    wallet_status: &str,
    reason_code: &str,
    expected_count: usize,
    imported_count: usize,
    bundle_sha256: &str,
) -> String {
    format!(
        "full recovery bundle root {root_status} and Wallet restore {wallet_status}; reason_code={reason_code}; expected_count={expected_count}; imported_count={imported_count}; bundle_sha256={bundle_sha256}"
    )
}

fn encode_recovery_terminal_retry_event(event: &RuntimeAuditEventV1) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(event)?;
    if bytes.len() > MAX_RECOVERY_TERMINAL_RETRY_BYTES {
        anyhow::bail!("Recovery terminal retry evidence is too large");
    }
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn recovery_terminal_retry_event(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<Option<RuntimeAuditEventV1>> {
    let Some(value) = headers.get(RECOVERY_TERMINAL_RETRY_HEADER) else {
        return Ok(None);
    };
    let encoded = value
        .to_str()
        .map_err(|_| anyhow::anyhow!("invalid Recovery terminal retry header"))?;
    if encoded.is_empty() || encoded.len() > MAX_RECOVERY_TERMINAL_RETRY_BYTES * 2 {
        anyhow::bail!("invalid Recovery terminal retry header size");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("invalid Recovery terminal retry encoding"))?;
    if bytes.len() > MAX_RECOVERY_TERMINAL_RETRY_BYTES {
        anyhow::bail!("Recovery terminal retry evidence is too large");
    }
    let event: RuntimeAuditEventV1 = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid Recovery terminal retry evidence"))?;
    crate::auth::verify_signed_full_recovery_outcome_audit_event(data_dir, &event)?;
    Ok(Some(event))
}

fn validate_recovery_terminal_retry_event(
    event: &RuntimeAuditEventV1,
    expected_event_id: &str,
    principal_id: &str,
    proof_binding_id: Option<&str>,
    session_id: &str,
    expected_wallet_count: usize,
    bundle_sha256: &str,
) -> anyhow::Result<()> {
    let reason_matches = ["imported", "reassigned"].iter().any(|root_status| {
        event.reason
            == full_recovery_outcome_audit_reason(
                root_status,
                WALLET_RESTORE_COMPLETE,
                WALLET_RESTORE_REASON_NONE,
                expected_wallet_count,
                expected_wallet_count,
                bundle_sha256,
            )
    });
    if event.event_id != expected_event_id
        || event.schema != RuntimeAuditEventV1::SCHEMA
        || event.event_type != "auth.full_recovery_bundle.imported"
        || event.principal_id.as_deref() != Some(principal_id)
        || event.proof_binding_id.as_deref() != proof_binding_id
        || event.session_id.as_deref() != Some(session_id)
        || event.challenge_id.is_some()
        || event.capsule_id.is_some()
        || event.result != "ok"
        || !reason_matches
    {
        anyhow::bail!("Recovery terminal retry evidence binding mismatch");
    }
    Ok(())
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
    let candidate_data_key = crate::auth::recovery_kit_data_key(&kit)?;
    let mut protection = protection_from_recovery_kit(
        &kit,
        Some("Imported Recovery Kit"),
        RecoveryKitDelivery::HandedToPerson,
        now,
        None,
    )?;
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
    let protected_object_inventory =
        principal_root_protected_object_inventory(&state.data_dir, &kit.localhost_root);
    let _protection_activation =
        crate::auth::begin_declarative_principal_root_protection_activation_with_candidate(
            &state.data_dir,
            &protection,
            &candidate_data_key,
            &protected_object_inventory,
        )?;
    let archive = crate::auth::recovery_archive_from_kit(&state.data_dir, &kit)?;
    let recovery_protector = protection
        .protectors
        .iter_mut()
        .find(|protector| {
            protector.kind == elastos_runtime::auth::PrincipalRootProtectorKind::RecoveryKit
        })
        .ok_or_else(|| anyhow::anyhow!("candidate protection has no Recovery Kit protector"))?;
    recovery_protector.archive = Some(archive);
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
        let proof_binding_id = principal.proof_binding_id.clone();
        let grant = AuthSessionGrantV1 {
            schema: AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: format!("grant:{}", random_hex(16)),
            session_id: format!("auth:{}", random_hex(16)),
            principal_id: kit.principal_id.clone(),
            proof_binding_id: proof_binding_id.clone(),
            issued_at: now,
            expires_at: now.saturating_add(AUTH_SESSION_TTL_SECS),
            apps: vec![
                HOME_CAPSULE_ID.to_string(),
                super::gateway::SYSTEM_CAPSULE_ID.to_string(),
            ],
        };
        #[cfg(test)]
        crate::auth::consume_recovery_reassignment_test_fault(
            &state.data_dir,
            crate::auth::RecoveryReassignmentTestFault::TokenPreparation,
        )?;
        let home_token =
            issue_home_launch_token_for_auth_grant(&state.data_dir, HOME_CAPSULE_ID, &grant)?;
        let system_token = issue_home_launch_token_for_auth_grant(
            &state.data_dir,
            super::gateway::SYSTEM_CAPSULE_ID,
            &grant,
        )?;
        let signed_audit_event = crate::auth::sign_audit_event(
            &state.data_dir,
            audit_event(AuditEventInput {
                event_type: "auth.recovery_kit.reassigned",
                principal_id: Some(kit.principal_id.clone()),
                proof_binding_id: Some(proof_binding_id.clone()),
                session_id: Some(grant.session_id.clone()),
                result: "ok",
                reason: "principal root reassigned from verified Recovery Kit and session reissued",
                occurred_at: now,
                ..AuditEventInput::default()
            }),
        )?;
        let principal = match crate::auth::commit_recovered_root_reassignment(
            &state.data_dir,
            crate::auth::RecoveredRootReassignment {
                proof_binding_id,
                recovered_principal_id: kit.principal_id.clone(),
                recovered_localhost_root: kit.localhost_root.clone(),
                protection,
                replacement_grant: grant,
                signed_audit_event,
                updated_at: now,
            },
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
        return Ok(RecoveryKitImportResponse {
            schema: "elastos.recovery-kit.import.response/v1".to_string(),
            principal_id: principal.principal_id,
            localhost_root: principal.localhost_root,
            status: "reassigned".to_string(),
            previous_principal_id: Some(previous_principal_id),
            previous_localhost_root: Some(previous_localhost_root),
            home_token: Some(home_token),
            system_token: Some(system_token),
        });
    }

    crate::auth::store_principal_root_protection(&state.data_dir, protection)?;
    crate::auth::append_audit_event(
        &state.data_dir,
        audit_event(AuditEventInput {
            event_type: "auth.recovery_kit.imported",
            principal_id: Some(principal.principal_id.clone()),
            proof_binding_id: Some(principal.proof_binding_id.clone()),
            session_id: Some(context.session_id.clone()),
            result: "ok",
            reason: "principal recovery kit imported and verified",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(RecoveryKitImportResponse {
        schema: "elastos.recovery-kit.import.response/v1".to_string(),
        principal_id: principal.principal_id,
        localhost_root: principal.localhost_root,
        status: "imported".to_string(),
        previous_principal_id: None,
        previous_localhost_root: None,
        home_token: None,
        system_token: None,
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
    delivery: RecoveryKitDelivery,
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
            // Verified means someone holds this off the machine. Only the
            // export path can say that.
            verified_at: match delivery {
                RecoveryKitDelivery::HandedToPerson => Some(now),
                RecoveryKitDelivery::RetainedUnseen => None,
            },
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

pub(in crate::api) struct WalletLinkContext {
    app: String,
    context: super::gateway::HomeLaunchTokenContext,
    authority: RuntimeWalletAuthority,
}

fn require_wallet_link_context(
    state: &GatewayState,
    headers: &HeaderMap,
) -> anyhow::Result<WalletLinkContext> {
    let authority = require_runtime_wallet_authority(
        &state.data_dir,
        headers,
        &[super::gateway::WALLET_WALLETCONNECT_CAPSULE_ID],
    )?;
    let verified = authority.verified_context();
    let app = verified.actor().to_string();
    verified_wallet_link_context(state, &app, authority)
}

pub(in crate::api) fn verified_wallet_link_context(
    state: &GatewayState,
    app: &str,
    authority: RuntimeWalletAuthority,
) -> anyhow::Result<WalletLinkContext> {
    if !is_wallet_connector_capsule_id(app) {
        anyhow::bail!("wallet linking requires a dedicated wallet connector capsule");
    }
    let verified = authority.verified_context();
    if verified.actor() != app {
        anyhow::bail!("wallet connector launch actor mismatch");
    }
    let context = super::gateway::HomeLaunchTokenContext {
        principal_id: verified.principal_id().to_string(),
        session_id: verified.session_id().to_string(),
        proof_binding_id: verified.proof_binding_id().map(ToString::to_string),
        grant_id: verified.grant_id().to_string(),
    };
    super::gateway::ensure_wallet_connector_configured(&state.data_dir, app)?;
    if context.proof_binding_id.is_none() {
        anyhow::bail!("missing proof-bound auth session");
    }
    Ok(WalletLinkContext {
        app: app.to_string(),
        context,
        authority,
    })
}

async fn wallet_link_provider_data(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    operation: WalletProviderOperationV2,
) -> anyhow::Result<Value> {
    match wallet_link_provider_result(state, authority, operation).await? {
        WalletResultV2::Ok { data } => Ok(data),
        WalletResultV2::Error { message, .. } => anyhow::bail!(message),
    }
}

async fn wallet_link_provider_result(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    operation: WalletProviderOperationV2,
) -> anyhow::Result<WalletResultV2> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("wallet provider unavailable"))?;
    Ok(RuntimeWalletAdapter::new(registry, authority)
        .invoke(operation)
        .await?
        .result)
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
) -> anyhow::Result<PasskeyBeginResponse<RequestOptions>> {
    let ceremony_id = format!("passkey:authenticate:{}", random_hex(16));
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let options = manager.begin_authentication(&ceremony_id, &rp.id)?;
    Ok(PasskeyBeginResponse {
        schema: "elastos.auth.passkey.authenticate.begin/v1".to_string(),
        ceremony_id,
        options,
    })
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
    let link = require_wallet_link_context(state, headers)?;
    evm_challenge_for_wallet_link(state, headers, input, link).await
}

pub(in crate::api) async fn evm_challenge_for_wallet_link(
    state: &GatewayState,
    headers: &HeaderMap,
    input: EvmChallengeRequest,
    link: WalletLinkContext,
) -> anyhow::Result<EvmChallengeResponse> {
    let WalletLinkContext {
        context, authority, ..
    } = link;
    validate_evm_address(&input.address).map_err(anyhow::Error::msg)?;
    if input.chain_id == 0 {
        anyhow::bail!("chain_id must be non-zero");
    }

    let now = crate::auth::now_ts();
    let request_authority = request_domain(headers)?;
    let scheme = request_scheme(&request_authority);
    let origin = format!("{scheme}://{request_authority}");
    let uri = format!("{origin}/apps/home/");
    let resources = vec![
        "elastos://wallet/account/link".to_string(),
        format!("elastos://principal/{}", context.principal_id),
    ];
    let data = wallet_link_provider_data(
        state,
        &authority,
        WalletProviderOperationV2::Challenge {
            domain: origin,
            uri,
            address: input.address,
            chain_id: input.chain_id,
            resources,
        },
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
    let link = require_wallet_link_context(state, headers)?;
    evm_verify_for_wallet_link(state, input, link).await
}

pub(in crate::api) async fn evm_verify_for_wallet_link(
    state: &GatewayState,
    input: EvmVerifyRequest,
    link: WalletLinkContext,
) -> anyhow::Result<EvmVerifyResponse> {
    let WalletLinkContext {
        app,
        context,
        authority,
    } = link;
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
    let data = match wallet_link_provider_result(
        state,
        &authority,
        WalletProviderOperationV2::VerifyProof {
            message: input.message.clone(),
            signature: input.signature.clone(),
        },
    )
    .await?
    {
        WalletResultV2::Ok { data } => data,
        WalletResultV2::Error { code, message } if code == "invalid_proof" => {
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
                    "Wallet EOA proof failed ({message}); ERC-1271 verification failed ({chain_err})"
                )
            })?;
            let evidence = erc1271_wallet_evidence(network, erc1271_proof)?;
            wallet_link_provider_data(
                state,
                &authority,
                WalletProviderOperationV2::VerifyContractProof {
                    message: input.message.clone(),
                    signature: input.signature.clone(),
                    evidence,
                },
            )
            .await?
        }
        WalletResultV2::Error { code, message } => {
            anyhow::bail!("Wallet proof rejected ({code}): {message}")
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
    let _ = wallet_connector_id_for_wallet_link(&app)?;
    wallet_link_provider_data(
        state,
        &authority,
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: principal.proof_binding_id.clone(),
            chain_namespace,
            address,
            proof_type: proof_type.clone(),
            label: None,
        },
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

    let app_token = super::gateway::issue_home_projection_launch_token_with_context(
        &state.data_dir,
        &app,
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
    let link = require_wallet_link_context(state, headers)?;
    btc_challenge_for_wallet_link(state, headers, input, link).await
}

pub(in crate::api) async fn btc_challenge_for_wallet_link(
    state: &GatewayState,
    headers: &HeaderMap,
    input: BtcChallengeRequest,
    link: WalletLinkContext,
) -> anyhow::Result<BtcChallengeResponse> {
    let WalletLinkContext {
        context, authority, ..
    } = link;
    let now = crate::auth::now_ts();
    let domain = request_domain(headers)?;
    let scheme = request_scheme(&domain);
    let uri = format!("{scheme}://{domain}/apps/home/");
    let resources = vec![
        "elastos://wallet/account/link".to_string(),
        format!("elastos://principal/{}", context.principal_id),
    ];
    let data = wallet_link_provider_data(
        state,
        &authority,
        WalletProviderOperationV2::BitcoinChallenge {
            domain,
            uri,
            address: input.address,
            network: PublicNetwork::new(input.network)?,
            resources,
        },
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
    let link = require_wallet_link_context(state, headers)?;
    btc_verify_for_wallet_link(state, input, link).await
}

pub(in crate::api) async fn btc_verify_for_wallet_link(
    state: &GatewayState,
    input: BtcVerifyRequest,
    link: WalletLinkContext,
) -> anyhow::Result<BtcVerifyResponse> {
    let WalletLinkContext {
        app,
        context,
        authority,
    } = link;
    let session_proof_binding_id = context
        .proof_binding_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing proof-bound auth session"))?;
    let challenge_id = bitcoin_challenge_id_from_message(&input.message)?;
    let now = crate::auth::now_ts();
    let data = wallet_link_provider_data(
        state,
        &authority,
        WalletProviderOperationV2::VerifyBip322Proof {
            message: input.message.clone(),
            signature: input.signature,
            signature_type: input
                .signature_type
                .unwrap_or_else(|| "bip322_simple".to_string()),
            public_key: input.public_key,
        },
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
    let _ = wallet_connector_id_for_wallet_link(&app)?;
    wallet_link_provider_data(
        state,
        &authority,
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: principal.proof_binding_id.clone(),
            chain_namespace,
            address,
            proof_type,
            label: None,
        },
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

    let app_token = super::gateway::issue_home_projection_launch_token_with_context(
        &state.data_dir,
        &app,
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

fn erc1271_wallet_evidence(
    expected_network: &str,
    mut data: Value,
) -> anyhow::Result<Erc1271ProofEvidenceV1> {
    if data
        .get("network")
        .and_then(|network| network.get("id"))
        .and_then(Value::as_str)
        != Some(expected_network)
    {
        anyhow::bail!("ERC-1271 proof network mismatch");
    }
    data["network"] = Value::String(expected_network.to_string());
    serde_json::from_value(data)
        .map_err(|err| anyhow::anyhow!("invalid ERC-1271 proof evidence: {err}"))
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
    let profile_readiness = super::gateway::profile_readiness_for_principal(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )
    .readiness;
    Ok(PasskeyVerifyResponse {
        schema: "elastos.auth.passkey.verify/v2".to_string(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: grant.session_id,
        expires_at: grant.expires_at,
        home_token,
        system_token,
        profile_readiness,
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

pub(in crate::api) fn auth_error_response(err: anyhow::Error) -> Response {
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
        || text.contains("conflicting")
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

    #[test]
    fn full_recovery_semantic_digest_canonicalizes_objects_and_binds_nested_content() {
        let first: Value = serde_json::from_str(
            r#"{
                "schema":"elastos.full-recovery-bundle/v1",
                "bundle_id":"bundle:test",
                "data_kit":{"kit_id":"kit:test","crypto":{"cipher":"aes-256-gcm"}},
                "wallet_recovery_keys":[{"account_id":"wallet:test","private_key_hex":"11"}]
            }"#,
        )
        .unwrap();
        let reordered: Value = serde_json::from_str(
            r#"{
                "wallet_recovery_keys":[{"private_key_hex":"11","account_id":"wallet:test"}],
                "data_kit":{"crypto":{"cipher":"aes-256-gcm"},"kit_id":"kit:test"},
                "bundle_id":"bundle:test",
                "schema":"elastos.full-recovery-bundle/v1"
            }"#,
        )
        .unwrap();
        assert_eq!(
            full_recovery_bundle_semantic_digest(&first).unwrap(),
            full_recovery_bundle_semantic_digest(&reordered).unwrap()
        );

        let mut kit_substitution = reordered.clone();
        kit_substitution["data_kit"]["kit_id"] = json!("kit:substituted");
        assert_ne!(
            full_recovery_bundle_semantic_digest(&first).unwrap(),
            full_recovery_bundle_semantic_digest(&kit_substitution).unwrap()
        );

        let mut wallet_substitution = reordered;
        wallet_substitution["wallet_recovery_keys"][0]["private_key_hex"] = json!("22");
        assert_ne!(
            full_recovery_bundle_semantic_digest(&first).unwrap(),
            full_recovery_bundle_semantic_digest(&wallet_substitution).unwrap()
        );
    }

    fn test_gateway_state(data_dir: &std::path::Path) -> GatewayState {
        GatewayState {
            provider_registry: None,
            collaboration_chat_product_port: None,
            collaboration_presence_product_port: None,
            collaboration_discovery_service: None,
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
            collaboration_chat_product_port: None,
            collaboration_presence_product_port: None,
            collaboration_discovery_service: None,
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
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        let payload: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token)
                .expect("decode test launch token"),
        )
        .expect("parse test launch token");
        let actor = payload["payload"]["launch_context"]["executable_actor"]
            .as_str()
            .expect("test launch token actor");
        headers.insert(
            "origin",
            if actor == HOME_CAPSULE_ID {
                HeaderValue::from_static("http://localhost:61180")
            } else {
                HeaderValue::from_static("null")
            },
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
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        headers.insert("origin", HeaderValue::from_static("http://localhost:61180"));
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

        assert_eq!(response.schema, "elastos.auth.passkey.verify/v2");
        assert_eq!(
            serde_json::to_value(&response.profile_readiness).unwrap(),
            serde_json::json!({
                "schema": "elastos.profile.readiness/v1",
                "status": "setup_required",
            })
        );
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
        let encoded = serde_json::to_value(&accounts).unwrap();
        assert!(encoded[0].get("localhost_root").is_none());
        assert!(encoded[0].get("proof_binding_id").is_none());
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
    async fn recovery_status_distinguishes_configured_protection_from_plaintext_objects() {
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
        crate::auth::store_test_principal_root_protection(temp.path(), &principal.principal_id);
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            principal.localhost_root
        );
        let object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(&object_path, b"legacy plaintext").unwrap();

        let status = recovery_status_inner(&state, &home_token_headers(&grant.home_token))
            .await
            .unwrap();

        assert!(status.protection_configured);
        assert!(!status.root_encrypted);
        assert!(status
            .required_actions
            .contains(&"migrate_declared_plaintext_objects".to_string()));
        assert!(!serde_json::to_string(&status)
            .unwrap()
            .contains(object_uri.as_str()));
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
    async fn recovery_kit_import_accepts_exact_kit_envelope_binding_without_prior_protection() {
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
        let archive = crate::auth::recovery_archive_from_kit(temp.path(), &kit).unwrap();
        let protection = protection_from_recovery_kit(
            &kit,
            Some("Exact imported binding"),
            RecoveryKitDelivery::HandedToPerson,
            1_800_000_000,
            Some(archive),
        )
        .unwrap();
        crate::auth::store_principal_root_protection(temp.path(), protection).unwrap();
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            principal.localhost_root
        );
        let object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        crate::auth::write_principal_root_object(
            temp.path(),
            &grant.principal_id,
            &principal.localhost_root,
            &object_uri,
            &object_path,
            b"exact imported key",
        )
        .unwrap();
        let mut auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        auth_state.principal_root_protections.clear();
        crate::auth::save_auth_state(temp.path(), &auth_state).unwrap();
        let request = RecoveryKitMaterialImport {
            principal_id: grant.principal_id.clone(),
            localhost_root: principal.localhost_root.clone(),
            reassign_to_current_principal: false,
            kit: kit.clone(),
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
        assert_eq!(
            crate::auth::read_principal_root_object(
                temp.path(),
                &kit.principal_id,
                &kit.localhost_root,
                &object_uri,
                &object_path,
            )
            .unwrap(),
            b"exact imported key"
        );
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.imported");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn recovery_kit_import_rejects_wrong_key_binding_without_mutation() {
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
        let exact_kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("exact"),
            1_800_000_000,
        )
        .unwrap();
        let archive = crate::auth::recovery_archive_from_kit(temp.path(), &exact_kit).unwrap();
        let protection = protection_from_recovery_kit(
            &exact_kit,
            Some("Exact retained binding"),
            RecoveryKitDelivery::HandedToPerson,
            1_800_000_000,
            Some(archive),
        )
        .unwrap();
        crate::auth::store_principal_root_protection(temp.path(), protection).unwrap();
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            principal.localhost_root
        );
        let object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        crate::auth::write_principal_root_object(
            temp.path(),
            &grant.principal_id,
            &principal.localhost_root,
            &object_uri,
            &object_path,
            b"exact retained key",
        )
        .unwrap();
        let wrong_kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("wrong"),
            1_800_000_001,
        )
        .unwrap();
        assert_ne!(wrong_kit.data_key_id, exact_kit.data_key_id);
        let auth_state_path = crate::auth::auth_state_path(temp.path()).unwrap();
        let auth_state_before = std::fs::read(&auth_state_path).unwrap();
        let archive_key_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), "ElastOS/System/Auth")
                .unwrap()
                .join("recovery-archive.key");
        let archive_key_before = std::fs::read(&archive_key_path).unwrap();
        let object_before = std::fs::read(&object_path).unwrap();

        let err = recovery_kit_import_inner(
            &state,
            &home_token_headers(&grant.home_token),
            RecoveryKitMaterialImport {
                principal_id: grant.principal_id,
                localhost_root: principal.localhost_root,
                reassign_to_current_principal: false,
                kit: wrong_kit,
                did_recovery_proof: None,
            },
        )
        .await
        .expect_err("a valid but wrong Recovery Kit must not activate the root");

        assert!(err.to_string().contains("envelope binding is invalid"));
        assert_eq!(std::fs::read(auth_state_path).unwrap(), auth_state_before);
        assert_eq!(std::fs::read(archive_key_path).unwrap(), archive_key_before);
        assert_eq!(std::fs::read(object_path).unwrap(), object_before);
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &grant.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }

    #[tokio::test]
    async fn recovery_kit_import_requires_plaintext_migration_before_any_commit() {
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
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            principal.localhost_root
        );
        let object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(&object_path, b"existing uCity save").unwrap();
        let kit = create_recovery_kit_for_principal(
            &grant.principal_id,
            &principal.localhost_root,
            Some("test"),
            1_800_000_000,
        )
        .unwrap();
        let before = crate::auth::load_auth_state(temp.path()).unwrap();
        let archive_key_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), "ElastOS/System/Auth")
                .unwrap()
                .join("recovery-archive.key");

        let err = recovery_kit_import_inner(
            &state,
            &home_token_headers(&grant.home_token),
            RecoveryKitMaterialImport {
                principal_id: grant.principal_id,
                localhost_root: principal.localhost_root.clone(),
                reassign_to_current_principal: false,
                kit,
                did_recovery_proof: None,
            },
        )
        .await
        .expect_err("plaintext migration must precede Recovery Kit import");
        let outcome = err
            .downcast_ref::<crate::auth::PrincipalRootMigrationRequiredV1>()
            .expect("typed migration-required outcome");
        let after = crate::auth::load_auth_state(temp.path()).unwrap();

        assert_eq!(outcome.plaintext_object_count, 1);
        assert_eq!(after.audit.len(), before.audit.len());
        assert_eq!(after.sessions.len(), before.sessions.len());
        assert_eq!(
            after.principal_root_protections,
            before.principal_root_protections
        );
        assert!(!archive_key_path.exists());
        assert_eq!(std::fs::read(object_path).unwrap(), b"existing uCity save");
    }

    #[test]
    fn protected_object_inventory_includes_gba_but_excludes_vm_and_provider_state() {
        let temp = tempfile::tempdir().unwrap();
        let localhost_root = crate::auth::principal_localhost_root("person:local:inventory");
        let inventory = principal_root_protected_object_inventory(temp.path(), &localhost_root);
        let uris = inventory
            .iter()
            .map(crate::auth::PrincipalRootProtectedObjectDeclarationV1::uri)
            .collect::<Vec<_>>();

        assert!(uris
            .iter()
            .any(|uri| uri.ends_with("/.AppData/LocalHost/GBA")));
        assert!(uris.iter().all(|uri| !uri.contains("/BrowserProfiles")));
        assert!(uris
            .iter()
            .all(|uri| { !uri.contains("/ProviderLogs") && !uri.contains("/.Runtime/Providers") }));
    }

    #[test]
    fn configured_principal_root_readiness_rejects_declared_plaintext() {
        let temp = tempfile::tempdir().unwrap();
        let principal_id = "person:local:startup-readiness";
        let protection =
            crate::auth::store_test_principal_root_protection(temp.path(), principal_id);
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/legacy.sav",
            protection.localhost_root
        );
        let object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(&object_path, b"legacy save").unwrap();

        let error = verify_configured_principal_roots_ready(temp.path())
            .expect_err("Home readiness must reject declared plaintext");
        let outcome = error
            .downcast_ref::<crate::auth::PrincipalRootMigrationRequiredV1>()
            .expect("typed migration-required readiness result");

        assert_eq!(outcome.principal_id, principal_id);
        assert_eq!(outcome.plaintext_object_count, 1);
        assert_eq!(std::fs::read(object_path).unwrap(), b"legacy save");
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
        let archive = crate::auth::recovery_archive_from_kit(temp.path(), &kit).unwrap();
        let protection = protection_from_recovery_kit(
            &kit,
            Some("Orphaned exact binding"),
            RecoveryKitDelivery::HandedToPerson,
            1_800_000_000,
            Some(archive),
        )
        .unwrap();
        crate::auth::store_principal_root_protection(temp.path(), protection).unwrap();
        let recovered_object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            old_principal.localhost_root
        );
        let recovered_object_path =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), &recovered_object_uri)
                .unwrap();
        crate::auth::write_principal_root_object(
            temp.path(),
            &old_principal.principal_id,
            &old_principal.localhost_root,
            &recovered_object_uri,
            &recovered_object_path,
            b"reassigned exact key",
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
        assert_eq!(
            crate::auth::read_principal_root_object(
                temp.path(),
                &old_principal.principal_id,
                &old_principal.localhost_root,
                &recovered_object_uri,
                &recovered_object_path,
            )
            .unwrap(),
            b"reassigned exact key"
        );
        let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
        let event = auth_state.audit.last().unwrap();
        assert_eq!(event.event_type, "auth.recovery_kit.reassigned");
        assert_eq!(event.result, "ok");
    }

    #[tokio::test]
    async fn recovery_reassignment_precommit_failures_preserve_original_authority() {
        for fault in [
            crate::auth::RecoveryReassignmentTestFault::TokenPreparation,
            crate::auth::RecoveryReassignmentTestFault::AuditChainRejection,
            crate::auth::RecoveryReassignmentTestFault::AuthStateSave,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = test_gateway_state(temp.path());
            let recovered = issue_passkey_session_grant(
                &state,
                "identity-test",
                &test_credential(),
                "https://elastos.elacitylabs.com",
                true,
                "recovered passkey grant",
            )
            .unwrap();
            let recovered_principal = crate::auth::load_principal_for_proof_binding(
                temp.path(),
                &recovered.proof_binding_id,
            )
            .unwrap();
            let kit = create_recovery_kit_for_principal(
                &recovered_principal.principal_id,
                &recovered_principal.localhost_root,
                Some("fault-injected recovered root"),
                1_800_000_000,
            )
            .unwrap();
            crate::auth::revoke_passkey_binding(
                temp.path(),
                &recovered.proof_binding_id,
                crate::auth::now_ts(),
            )
            .unwrap();
            let current = issue_passkey_session_grant(
                &state,
                "identity-test",
                &test_credential_2(),
                "https://elastos.elacitylabs.com",
                true,
                "pre-reassignment passkey grant",
            )
            .unwrap();
            let current_principal = crate::auth::load_principal_for_proof_binding(
                temp.path(),
                &current.proof_binding_id,
            )
            .unwrap();
            crate::auth::inject_recovery_reassignment_test_fault(temp.path(), fault);

            let error = recovery_kit_import_inner(
                &state,
                &home_token_headers(&current.home_token),
                RecoveryKitMaterialImport {
                    principal_id: current_principal.principal_id.clone(),
                    localhost_root: current_principal.localhost_root.clone(),
                    reassign_to_current_principal: true,
                    kit: kit.clone(),
                    did_recovery_proof: None,
                },
            )
            .await
            .expect_err("fault-injected reassignment must fail");

            assert!(error.to_string().contains("injected recovery reassignment"));
            assert!(crate::auth::is_auth_session_active(
                temp.path(),
                &current.session_id,
                crate::auth::now_ts()
            )
            .unwrap());
            let unchanged = crate::auth::load_principal_for_proof_binding(
                temp.path(),
                &current.proof_binding_id,
            )
            .unwrap();
            assert_eq!(unchanged.principal_id, current_principal.principal_id);
            assert_eq!(unchanged.localhost_root, current_principal.localhost_root);
            recovery_status_inner(&state, &home_token_headers(&current.home_token))
                .await
                .expect("pre-reassignment token must remain usable");

            let auth_state = crate::auth::load_auth_state(temp.path()).unwrap();
            assert!(!auth_state.audit.iter().any(|event| {
                event.event_type == "auth.recovery_kit.reassigned" && event.result == "ok"
            }));
            assert!(!auth_state.sessions.iter().any(|stored| {
                stored.revoked_at.is_none()
                    && stored.grant.proof_binding_id == current.proof_binding_id
                    && stored.grant.principal_id == recovered_principal.principal_id
            }));
            assert!(crate::auth::load_principal_root_protection(
                temp.path(),
                &kit.principal_id,
                &kit.localhost_root,
            )
            .unwrap()
            .is_none());
        }
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
        headers.insert(
            "origin",
            HeaderValue::from_static("https://elastos.elacitylabs.com"),
        );

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
    fn registration_with_a_name_keeps_profile_setup_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();

        let registered = issue_named_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey registration",
            Some("Anders"),
        )
        .unwrap();
        let principal = crate::auth::load_principal_for_proof_binding(
            temp.path(),
            &registered.proof_binding_id,
        )
        .unwrap();
        assert_eq!(principal.display_name, "Anders");
        assert_eq!(
            serde_json::to_value(&registered.profile_readiness).unwrap(),
            serde_json::json!({
                "schema": "elastos.profile.readiness/v1",
                "status": "setup_required",
            })
        );
        assert!(
            crate::collaboration_profile_authority::load_profile_authority(
                temp.path(),
                &principal.principal_id,
                &principal.localhost_root,
            )
            .unwrap()
            .is_none()
        );
        assert!(crate::auth::load_principal_root_protection(
            temp.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .is_none());
        assert!(
            !crate::collaboration_profile_authority::profile_authority_path(
                temp.path(),
                &principal.localhost_root,
            )
            .unwrap()
            .exists()
        );
        let recovery_archive_key =
            elastos_common::localhost::rooted_localhost_fs_path(temp.path(), "ElastOS/System/Auth")
                .unwrap()
                .join("recovery-archive.key");
        assert!(!recovery_archive_key.exists());
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &registered.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());

        let again = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey authentication",
        )
        .unwrap();
        assert_eq!(again.proof_binding_id, registered.proof_binding_id);
        assert_eq!(
            serde_json::to_value(&again.profile_readiness).unwrap()["status"],
            "setup_required"
        );
    }

    #[test]
    fn authentication_survives_an_invalid_profile_without_reporting_ready() {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let first = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "initial passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &first.proof_binding_id)
                .unwrap();
        crate::auth::store_test_principal_root_protection(temp.path(), &principal.principal_id);
        let object_uri = crate::collaboration_profile_authority::profile_authority_object_uri(
            &principal.localhost_root,
        );
        let path = crate::collaboration_profile_authority::profile_authority_path(
            temp.path(),
            &principal.localhost_root,
        )
        .unwrap();
        crate::auth::write_protected_principal_root_object(
            temp.path(),
            &principal.principal_id,
            &principal.localhost_root,
            &object_uri,
            &path,
            b"not a profile bundle",
        )
        .unwrap();

        let authenticated = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "passkey authentication with invalid Profile",
        )
        .unwrap();

        assert_eq!(authenticated.proof_binding_id, first.proof_binding_id);
        assert_eq!(
            serde_json::to_value(&authenticated.profile_readiness).unwrap(),
            serde_json::json!({
                "schema": "elastos.profile.readiness/v1",
                "status": "unavailable",
            })
        );
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &authenticated.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());
    }

    #[test]
    fn authentication_reports_ready_only_after_profile_verification() {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        let first = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "initial passkey grant",
        )
        .unwrap();
        let principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &first.proof_binding_id)
                .unwrap();
        elastos_identity::load_or_create_did(temp.path()).unwrap();
        crate::auth::store_test_principal_root_protection(temp.path(), &principal.principal_id);
        crate::collaboration_profile_authority::update_profile_authority(
            temp.path(),
            &principal.principal_id,
            &principal.localhost_root,
            &principal.proof_binding_id,
            "Verified Profile",
            None,
            crate::auth::now_ts(),
        )
        .unwrap();

        let authenticated = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "passkey authentication with verified Profile",
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(&authenticated.profile_readiness).unwrap(),
            serde_json::json!({
                "schema": "elastos.profile.readiness/v1",
                "status": "ready",
            })
        );
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &authenticated.session_id,
            crate::auth::now_ts(),
        )
        .unwrap());
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

    #[tokio::test]
    async fn sign_out_revokes_only_active_session_without_resetting_principal_or_passkey() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let active = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test passkey grant",
        )
        .unwrap();
        let retained = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "retained passkey grant",
        )
        .unwrap();
        let principal_before =
            crate::auth::load_principal_for_proof_binding(temp.path(), &active.proof_binding_id)
                .unwrap();
        let headers = home_session_cookie_headers(&active.home_token);

        let response = sign_out_session_inner(&state, &headers).unwrap();

        assert_eq!(response.status, "signed_out");
        assert_eq!(response.session_id, active.session_id);
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
            &active.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
        assert!(crate::auth::is_auth_session_active(
            temp.path(),
            &retained.session_id,
            crate::auth::now_ts()
        )
        .unwrap());

        let principal_after =
            crate::auth::load_principal_for_proof_binding(temp.path(), &active.proof_binding_id)
                .unwrap();
        assert_eq!(
            serde_json::to_value(principal_after).unwrap(),
            serde_json::to_value(principal_before).unwrap(),
        );

        let passkeys = passkey_list_inner(&state, &home_token_headers(&retained.home_token))
            .await
            .unwrap();
        assert!(passkeys.passkeys.iter().any(|passkey| {
            passkey.proof_binding_id == active.proof_binding_id
                && passkey.principal_id == active.principal_id
                && passkey.current
        }));
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

    #[test]
    fn sign_out_accepts_stale_header_beside_rotated_session_cookie() {
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

        let refreshed =
            refresh_session_inner(&state, &home_token_headers(&grant.home_token)).unwrap();
        assert_ne!(refreshed.home_token, grant.home_token);

        // The tab keeps its pre-refresh mint in the header while the browser
        // carries the rotated session cookie — one session, two generations.
        let mut headers = home_token_headers(&grant.home_token);
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "{}={}",
                super::super::gateway::HOME_SESSION_COOKIE,
                refreshed.home_token
            ))
            .unwrap(),
        );

        let signed_out = sign_out_session_inner(&state, &headers).unwrap();

        assert_eq!(signed_out.session_id, grant.session_id);
        assert!(!crate::auth::is_auth_session_active(
            temp.path(),
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
        headers.insert(
            "origin",
            HeaderValue::from_static("https://elastos.elacitylabs.com"),
        );

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
