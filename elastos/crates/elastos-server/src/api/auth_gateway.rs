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
    GatewayState, RuntimeWalletAdapter, RuntimeWalletAuthority, HOME_CAPSULE_ID, HOME_ROUTE,
};

use crate::auth::AUTH_SESSION_TTL_SECS;
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
    pub owner_setup_pending: bool,
    pub guest_registration_enabled: bool,
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
    #[serde(skip)]
    guest_client_cookie: Option<HeaderValue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegisterBeginRequest {
    pub(crate) intent: Option<crate::auth::PasskeyEnrollmentIntent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegisterCompleteRequest {
    pub ceremony_id: String,
    pub response: Option<RegistrationResponse>,
    pub(crate) intent: Option<crate::auth::PasskeyEnrollmentIntent>,
    #[serde(default, deserialize_with = "reject_registration_name_field")]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "reject_registration_name_field")]
    pub profile_display_name: Option<String>,
}

fn reject_registration_name_field<'de, D>(_: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Err(serde::de::Error::custom(
        "Use the enrollment intent for account setup",
    ))
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
    Json(PasskeyStatusResponse {
        registered: manager.status().registered,
        owner_setup_pending: match crate::auth::owner_setup_pending(&state.data_dir) {
            Ok(pending) => pending,
            Err(err) => return auth_error_response(err),
        },
        guest_registration_enabled: crate::auth::guest_registration_enabled(&state.data_dir)
            .unwrap_or(false),
    })
    .into_response()
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
    mut headers: HeaderMap,
    Json(input): Json<PasskeyRegisterBeginRequest>,
) -> Response {
    if input.intent.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enrollment intent is required",
        )
            .into_response();
    }
    if let Some(crate::auth::PasskeyEnrollmentIntent::Create { public_name }) =
        input.intent.as_ref()
    {
        let valid_name = crate::auth::validate_owner_enrollment_intent(input.intent.as_ref())
            .is_ok()
            && crate::collaboration_profile_authority::clean_profile_display_name(public_name)
                .is_ok();
        if !valid_name {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose a public display name",
            )
                .into_response();
        }
    }
    let local_first_owner = local_first_owner_registration(&headers, peer.map(|peer| peer.0));
    if let Err(err) = prepare_owner_claim(&mut headers, local_first_owner) {
        return auth_error_response(err);
    }
    let intent = input.intent;
    match passkey_register_begin_inner(&state, &headers, local_first_owner, intent.as_ref()).await {
        Ok(mut response) => {
            let guest_cookie = response.guest_client_cookie.take();
            let mut response = Json(response).into_response();
            if let Some(cookie) = guest_cookie {
                response.headers_mut().append(SET_COOKIE, cookie);
                response
            } else {
                with_owner_claim_cookie(&headers, response)
            }
        }
        Err(err) => auth_error_response(err),
    }
}

pub async fn passkey_register_complete(
    State(state): State<GatewayState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(input): Json<PasskeyRegisterCompleteRequest>,
) -> Response {
    if input.intent.is_none()
        || input.display_name.is_some()
        || input.profile_display_name.is_some()
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Use the enrollment intent for account setup",
        )
            .into_response();
    }
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
    principal_root_recovery_status_for_verified_principal(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )
}

pub(in crate::api) fn principal_root_recovery_status_for_verified_principal(
    data_dir: &std::path::Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<PrincipalRootRecoveryStatusV1> {
    let protected_object_inventory =
        principal_root_protected_object_inventory(data_dir, localhost_root);
    let inspection = crate::auth::inspect_declarative_principal_root_protection(
        data_dir,
        principal_id,
        localhost_root,
        &protected_object_inventory,
    )?;
    let Some(protection) = inspection.protection else {
        let mut status = PrincipalRootRecoveryStatusV1::unprotected(
            principal_id.to_string(),
            localhost_root.to_string(),
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
    if let Some(profile) = crate::collaboration_profile_authority::load_profile_authority(
        data_dir,
        principal_id,
        localhost_root,
    )? {
        if !protection.protectors.iter().any(|protector| {
            protector
                .profile_coverage
                .as_ref()
                .is_some_and(|coverage| coverage.profile_did == profile.document().profile_did)
        }) {
            required_actions.push("download_recovery_kit_with_profile".to_string());
        }
    }
    Ok(PrincipalRootRecoveryStatusV1 {
        schema: elastos_runtime::auth::PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA.to_string(),
        principal_id: principal_id.to_string(),
        localhost_root: localhost_root.to_string(),
        root_encrypted,
        recovery_configured,
        recovery_download_available,
        protection_configured,
        required_actions,
        crypto: protection.crypto,
    })
}

pub(in crate::api) fn principal_root_recovery_is_ready(
    recovery: &PrincipalRootRecoveryStatusV1,
) -> bool {
    recovery.root_encrypted && recovery.recovery_configured
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

pub(in crate::api) fn initialize_local_profile(
    state: &GatewayState,
    principal: &crate::auth::PrincipalRecord,
    session_id: &str,
    display_name: &str,
) -> anyhow::Result<()> {
    crate::collaboration_profile_authority::require_profile_authority_passkey_binding(
        &state.data_dir,
        &principal.principal_id,
        Some(&principal.proof_binding_id),
    )?;
    if crate::collaboration_profile_authority::load_profile_authority(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
    )?
    .is_some()
    {
        return Ok(());
    }
    crate::collaboration_profile_authority::validate_profile_authority_update(
        &state.data_dir,
        display_name,
        None,
    )?;
    {
        let inventory =
            principal_root_protected_object_inventory(&state.data_dir, &principal.localhost_root);
        let activation = crate::auth::begin_declarative_principal_root_protection_activation_migrating_plaintext(
            &state.data_dir, &principal.principal_id, &principal.localhost_root, &inventory,
        )?;
        crate::auth::ensure_online_plaintext_migration_is_possible(&activation.plaintext_objects)?;
        if crate::auth::load_principal_root_protection(
            &state.data_dir,
            &principal.principal_id,
            &principal.localhost_root,
        )?
        .is_none()
        {
            recovery_kit_get_or_create_for_principal(
                state,
                session_id,
                principal,
                None,
                RecoveryKitDelivery::RetainedUnseen,
                crate::auth::now_ts(),
            )?;
        }
        if !activation.plaintext_objects.is_empty() {
            let migrated = crate::auth::migrate_principal_root_plaintext_objects_under_activation(
                &activation.guard,
                &state.data_dir,
                &principal.principal_id,
                &principal.localhost_root,
                activation.plaintext_objects.clone(),
            )?;
            crate::auth::append_audit_event(
                &state.data_dir,
                audit_event(AuditEventInput {
                    event_type: "auth.principal_root.plaintext_migrated",
                    principal_id: Some(principal.principal_id.clone()),
                    proof_binding_id: Some(principal.proof_binding_id.clone()),
                    session_id: Some(session_id.to_string()),
                    result: "ok",
                    reason: "declared plaintext objects migrated during Profile setup",
                    occurred_at: crate::auth::now_ts(),
                    ..AuditEventInput::default()
                }),
            )?;
            tracing::info!(
                object_count = migrated.object_count,
                "declared objects migrated during Profile setup"
            );
        }
    }
    #[cfg(test)]
    local_profile_setup_test_fault(&state.data_dir, "after-root")?;
    crate::collaboration_profile_authority::ensure_initial_profile_authority(
        &state.data_dir,
        &principal.principal_id,
        &principal.localhost_root,
        &principal.proof_binding_id,
        display_name,
        crate::auth::now_ts(),
    )?;
    #[cfg(test)]
    local_profile_setup_test_fault(&state.data_dir, "after-profile")?;
    Ok(())
}

#[cfg(test)]
thread_local! {
    static LOCAL_PROFILE_SETUP_FAULT: std::cell::RefCell<Option<(std::path::PathBuf, &'static str)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn local_profile_setup_test_fault(data_dir: &std::path::Path, point: &str) -> anyhow::Result<()> {
    LOCAL_PROFILE_SETUP_FAULT.with(|slot| {
        let mut fault = slot.borrow_mut();
        if fault
            .as_ref()
            .is_some_and(|(path, selected)| path == data_dir && *selected == point)
        {
            *fault = None;
            anyhow::bail!("injected local Profile setup failure");
        }
        Ok(())
    })
}

/// Synchronous so the activation guard never enters the export handler's
/// async state machine. Export consumes its own bound step-up.
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
    let intent = serde_json::json!({
        "principal_id": input.principal_id,
        "localhost_root": input.localhost_root,
        "label": input.label,
        "download_password": input.download_password,
    });
    consume_passkey_step_up_token(
        &state.data_dir,
        &input.step_up_token,
        launch,
        180,
        "auth.full-recovery-bundle.export",
        &intent,
    )?;
    let kit = recovery_kit_get_or_create_for_principal(
        state,
        &context.session_id,
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
    // This identity comes from the verified bundle being returned, not a later
    // Profile read or the caller's proposed name.
    let profile_coverage = people_identity
        .as_ref()
        .map(|identity| {
            let profile_did = identity
                .pointer("/profile_authority_bundle/signed_profile/payload/profile_did")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("verified recovery Profile identity is missing"))?;
            Ok::<_, anyhow::Error>(elastos_runtime::auth::RecoveryProfileCoverageV1 {
                profile_did: profile_did.to_string(),
                exported_at: now,
            })
        })
        .transpose()?;
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
    crate::auth::mark_recovery_kit_handed_to_person(&state.data_dir, &kit, profile_coverage, now)?;
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
            "full recovery bundle account/root binding mismatch; use account recovery to attach it"
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
            let principal = require_active_passkey_principal_for_context(state, &context)?;
            if principal.principal_id != input.principal_id
                || principal.localhost_root != input.localhost_root
            {
                anyhow::bail!("completed recovery principal/root binding mismatch");
            }
            // The signed local receipt records the original session. A later
            // authenticated session can read its result without repeating import.
            validate_completed_recovery_outcome(
                &completed_event,
                &completed_audit_id,
                bundle_principal,
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
    session_id: &str,
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
            session_id: Some(session_id.to_string()),
            result: "ok",
            reason: "principal recovery material retained locally",
            occurred_at: now,
            ..AuditEventInput::default()
        }),
    )?;
    Ok(kit)
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
    validate_completed_recovery_outcome(
        event,
        expected_event_id,
        principal_id,
        expected_wallet_count,
        bundle_sha256,
    )?;
    if event.proof_binding_id.as_deref() != proof_binding_id
        || event.session_id.as_deref() != Some(session_id)
    {
        anyhow::bail!("Recovery terminal retry evidence binding mismatch");
    }
    Ok(())
}

fn validate_completed_recovery_outcome(
    event: &RuntimeAuditEventV1,
    expected_event_id: &str,
    principal_id: &str,
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
            profile_coverage: None,
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
    intent: Option<&crate::auth::PasskeyEnrollmentIntent>,
) -> anyhow::Result<PasskeyBeginResponse<Option<CreationOptions>>> {
    let ceremony_id = format!("passkey:register:{}", random_hex(16));
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let claimant = owner_claim(headers)?.unwrap_or_default();
    if let Some((ceremony_id, options)) = crate::auth::begin_owner_enrollment_with_intent(
        &state.data_dir,
        &mut manager,
        &crate::auth::OwnerAdmission {
            origin: &rp.origin,
            rp_id: &rp.id,
            claimant: &claimant,
            loopback: local_first_owner,
        },
        &ceremony_id,
        intent,
        crate::auth::now_ts(),
    )? {
        return Ok(PasskeyBeginResponse {
            schema: "elastos.auth.passkey.register.begin/v1".into(),
            ceremony_id,
            options,
            guest_client_cookie: None,
        });
    }
    drop(manager);
    require_passkey_registration_allowed(state)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let (options, guest_client_cookie) = if let Some(intent) = intent {
        if !local_first_owner && !rp.origin.starts_with("https://") {
            return Err(crate::auth::OwnerEnrollmentDenied.into());
        }
        // Guest admission above is separate from remote first-owner admission.
        // This client claim has its own cookie namespace and no operator meaning.
        let claim = guest_registration_client_claim(headers, &rp.origin)?
            .unwrap_or_else(|| zeroize::Zeroizing::new(crate::auth::random_secret_hex()));
        let options = crate::auth::begin_guest_registration(
            &state.data_dir,
            &mut manager,
            &crate::auth::GuestRegistrationClient {
                origin: &rp.origin,
                rp_id: &rp.id,
                client_claim: &claim,
            },
            &ceremony_id,
            intent,
        )?;
        let mut cookie = HeaderValue::from_str(&format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict{}",
            guest_registration_cookie_name(&rp.origin),
            claim.as_str(),
            if rp.origin.starts_with("https://") {
                "; Secure"
            } else {
                ""
            },
        ))?;
        cookie.set_sensitive(true);
        (options, Some(cookie))
    } else {
        (
            manager.begin_principal_registration(&ceremony_id, &rp.id, &rp.origin)?,
            None,
        )
    };
    Ok(PasskeyBeginResponse {
        schema: "elastos.auth.passkey.register.begin/v1".to_string(),
        ceremony_id,
        options: Some(options),
        guest_client_cookie,
    })
}

async fn passkey_register_complete_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyRegisterCompleteRequest,
    local_first_owner: bool,
) -> anyhow::Result<PasskeyVerifyResponse> {
    let rp = super::handlers::identity::derive_rp(headers)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let claimant = owner_claim(headers)?.unwrap_or_default();
    if let Some(name) = &input.profile_display_name {
        crate::collaboration_profile_authority::validate_profile_authority_update(
            &state.data_dir,
            name,
            None,
        )?;
    }
    if let Some(grant) = crate::auth::complete_owner_enrollment_with_intent(
        &state.data_dir,
        &mut manager,
        &crate::auth::OwnerAdmission {
            origin: &rp.origin,
            rp_id: &rp.id,
            claimant: &claimant,
            loopback: local_first_owner,
        },
        &input.ceremony_id,
        input.response.as_ref(),
        crate::auth::OwnerEnrollmentCompletion {
            intent: input.intent.as_ref(),
            names: crate::auth::OwnerEnrollmentNames {
                display_name: input.display_name.as_deref(),
                profile_display_name: input.profile_display_name.as_deref(),
            },
        },
        crate::auth::now_ts(),
    )? {
        drop(manager);
        return passkey_response_for_grant(state, grant);
    }
    drop(manager);
    require_passkey_registration_allowed(state)?;
    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    if let Some(intent) = &input.intent {
        let claim = guest_registration_client_claim(headers, &rp.origin)?
            .ok_or(crate::auth::OwnerEnrollmentDenied)?;
        let grant = crate::auth::complete_guest_registration(
            &state.data_dir,
            &mut manager,
            &crate::auth::GuestRegistrationClient {
                origin: &rp.origin,
                rp_id: &rp.id,
                client_claim: &claim,
            },
            &input.ceremony_id,
            input
                .response
                .as_ref()
                .ok_or(crate::auth::OwnerEnrollmentDenied)?,
            intent,
        )?;
        drop(manager);
        return passkey_response_for_grant(state, grant);
    }
    let outcome = manager.complete_registration(
        &input.ceremony_id,
        input
            .response
            .as_ref()
            .ok_or(crate::auth::OwnerEnrollmentDenied)?,
        &rp.id,
        &rp.origin,
    )?;
    let credential = outcome.credential.clone();
    let origin = outcome.origin.clone();
    let user_verified = outcome.user_verified;
    drop(manager);
    let grant = crate::auth::grant_passkey_session(
        &state.data_dir,
        crate::auth::PasskeySessionRequest {
            credential: &credential,
            origin: &origin,
            user_verified,
            display_name: input.display_name.as_deref(),
            reason: "passkey registration verified and session granted",
            profile_display_name: input.profile_display_name.as_deref(),
            purpose: crate::auth::PasskeySessionPurpose::GuestRegistration,
        },
    )?;
    passkey_response_for_grant(state, grant)
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
        guest_client_cookie: None,
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

#[derive(Debug)]
enum PasskeyRegistrationDenied {
    GuestRegistrationDisabled,
}

impl std::fmt::Display for PasskeyRegistrationDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::GuestRegistrationDisabled => "guest passkey registration is disabled",
        })
    }
}

impl std::error::Error for PasskeyRegistrationDenied {}

fn require_passkey_registration_allowed(state: &GatewayState) -> anyhow::Result<()> {
    if crate::auth::guest_registration_enabled(&state.data_dir)?
        && crate::auth::active_admin_passkey_principal_count(&state.data_dir)? > 0
    {
        return Ok(());
    }
    Err(PasskeyRegistrationDenied::GuestRegistrationDisabled.into())
}

pub(in crate::api) fn local_first_owner_registration(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> bool {
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

const OWNER_CLAIM_HEADER: &str = "x-elastos-owner-enrollment";

fn guest_registration_cookie_name(origin: &str) -> &'static str {
    if origin.starts_with("https://") {
        "__Host-elastos-guest-registration"
    } else {
        "elastos-guest-registration"
    }
}

fn guest_registration_client_claim(
    headers: &HeaderMap,
    origin: &str,
) -> anyhow::Result<Option<zeroize::Zeroizing<String>>> {
    let name = guest_registration_cookie_name(origin);
    let cookies = headers.get_all(axum::http::header::COOKIE);
    let mut claims = cookies
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .filter(|(key, _)| *key == name);
    let Some((_, claim)) = claims.next() else {
        return Ok(None);
    };
    if claims.next().is_some()
        || claim.len() != 64
        || !claim
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(crate::auth::OwnerEnrollmentDenied.into());
    }
    Ok(Some(zeroize::Zeroizing::new(claim.to_string())))
}

pub(in crate::api) fn owner_claim(
    headers: &HeaderMap,
) -> anyhow::Result<Option<zeroize::Zeroizing<String>>> {
    let public = super::handlers::identity::derive_rp(headers)?
        .origin
        .starts_with("https://");
    let cookie_name = if public {
        "__Host-elastos-owner-claim="
    } else {
        "elastos-owner-claim="
    };
    let value = if let Some(header) = headers.get(OWNER_CLAIM_HEADER) {
        Some(
            header
                .to_str()
                .map_err(|_| crate::auth::OwnerEnrollmentDenied)?,
        )
    } else {
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|header| header.to_str().ok())
            .and_then(|cookies| {
                cookies
                    .split(';')
                    .find_map(|cookie| cookie.trim().strip_prefix(cookie_name))
            })
    };
    value
        .map(|value| {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(crate::auth::OwnerEnrollmentDenied.into());
            }
            Ok(zeroize::Zeroizing::new(value.to_string()))
        })
        .transpose()
}

pub(in crate::api) fn prepare_owner_claim(
    headers: &mut HeaderMap,
    loopback: bool,
) -> anyhow::Result<()> {
    let claim = owner_claim(headers)?
        .or_else(|| loopback.then(|| zeroize::Zeroizing::new(crate::auth::random_secret_hex())));
    if let Some(claim) = claim {
        headers.insert(OWNER_CLAIM_HEADER, HeaderValue::from_str(&claim)?);
    }
    Ok(())
}

pub(in crate::api) fn with_owner_claim_cookie(
    headers: &HeaderMap,
    mut response: Response,
) -> Response {
    if let Ok(Some(claim)) = owner_claim(headers) {
        let public = super::handlers::identity::derive_rp(headers)
            .is_ok_and(|rp| rp.origin.starts_with("https://"));
        let name = if public {
            "__Host-elastos-owner-claim"
        } else {
            "elastos-owner-claim"
        };
        let cookie = format!(
            "{name}={}; Path=/; HttpOnly; SameSite=Strict{}",
            claim.as_str(),
            if public { "; Secure" } else { "" }
        );
        if let Ok(cookie) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(SET_COOKIE, cookie);
        }
    }
    response
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
    let uri = format!("{origin}{HOME_ROUTE}");
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
    let uri = format!("{scheme}://{domain}{HOME_ROUTE}");
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
    let grant = crate::auth::grant_passkey_session(
        &state.data_dir,
        crate::auth::PasskeySessionRequest {
            credential,
            origin,
            user_verified,
            reason,
            display_name,
            purpose: crate::auth::PasskeySessionPurpose::SignIn,
            profile_display_name: None,
        },
    )?;
    passkey_response_for_grant(state, grant)
}

fn passkey_response_for_grant(
    state: &GatewayState,
    grant: AuthSessionGrantV1,
) -> anyhow::Result<PasskeyVerifyResponse> {
    let principal =
        crate::auth::load_principal_for_proof_binding(&state.data_dir, &grant.proof_binding_id)?;
    if let Some(name) = crate::auth::confirmed_initial_profile_name(&state.data_dir, &grant)? {
        initialize_local_profile(state, &principal, &grant.session_id, &name)?;
    }
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
    let status = if err.is::<PasskeyRegistrationDenied>()
        || err.is::<crate::auth::OwnerEnrollmentDenied>()
        || text.contains("missing")
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

    fn seed_test_passkey_principal(
        state: &GatewayState,
        credential: &StoredCredential,
        origin: &str,
        role: crate::auth::RuntimePrincipalRole,
    ) {
        let now = crate::auth::now_ts();
        let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
            credential_id: credential.credential_id.clone(),
            public_key: credential.public_key.clone(),
            sign_count: credential.sign_count,
            user_verified: true,
            origin: origin.to_string(),
            rp_id: credential.rp_id.clone(),
            created_at: now,
            last_used_at: now,
            revoked_at: None,
        });
        let auth = crate::auth::load_auth_state(&state.data_dir).unwrap();
        assert!(auth
            .principals
            .iter()
            .all(|principal| principal.proof_binding_id != binding.id()));
        let principal_id = crate::auth::passkey_credential_principal_id(
            &credential.rp_id,
            &credential.credential_id,
        )
        .unwrap();
        crate::auth::upsert_principal_for_binding_as_role(
            &state.data_dir,
            binding,
            principal_id,
            role,
            now,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn owner_enrollment_initial_profile_resumes_without_export() {
        use super::super::handlers::identity::{self, IdentityState};
        use axum::{
            body::{to_bytes, Body},
            http::Request,
            routing::post,
            Extension, Router,
        };
        use elastos_runtime::{
            primitives::audit::AuditLog,
            session::{SessionRegistry, SessionType},
        };
        use tower::ServiceExt;

        for direct in [false, true] {
            for public in [false, true] {
                let root = local_profile_fixture_root();
                let _auth_dir =
                    super::super::gateway::set_test_home_launch_auth_data_dir(root.path());
                let origin = if public {
                    "https://home.example"
                } else {
                    "http://localhost:61180"
                };
                let rp = if public { "home.example" } else { "localhost" };
                let secret = public.then(|| {
                    crate::auth::arm_owner_enrollment(root.path(), origin, rp, 300).unwrap()
                });
                let registry = Arc::new(SessionRegistry::new(Arc::new(AuditLog::new())));
                let session = registry.create_session(SessionType::Shell, None).await;
                let bearer = session.token.clone();
                let make_router = || {
                    let router = if direct {
                        Router::new()
                            .route("/begin", post(identity::register_begin))
                            .route("/complete", post(identity::register_complete))
                            .with_state(IdentityState {
                                manager: Arc::new(tokio::sync::Mutex::new(
                                    elastos_identity::IdentityManager::new(root.path().into())
                                        .unwrap(),
                                )),
                                session_registry: registry.clone(),
                                audit_log: None,
                                data_dir: root.path().into(),
                            })
                            .layer(Extension(session.clone()))
                    } else {
                        Router::new()
                            .route("/begin", post(passkey_register_begin))
                            .route("/complete", post(passkey_register_complete))
                            .with_state(test_gateway_state(root.path()))
                    };
                    let peer: SocketAddr = if public {
                        "192.0.2.1:1234"
                    } else {
                        "127.0.0.1:1234"
                    }
                    .parse()
                    .unwrap();
                    router.layer(Extension(ConnectInfo(peer)))
                };
                let request =
                    |path: &str, cookie: &str, ceremony: &str, body: serde_json::Value| {
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header("origin", origin)
                            .header("host", rp)
                            .header("content-type", "application/json")
                            .header("cookie", cookie)
                            .header("x-elastos-owner-ceremony", ceremony)
                            .body(Body::from(body.to_string()))
                            .unwrap()
                    };
                let app = make_router();
                let intent = json!({"purpose": "create", "public_name": "Shared Name"});
                let begin_body = if direct {
                    json!({})
                } else {
                    json!({"intent": intent})
                };
                let mut begin_request = request("/begin", "", "", begin_body.clone());
                if let Some(secret) = &secret {
                    begin_request
                        .headers_mut()
                        .insert(OWNER_CLAIM_HEADER, secret.as_str().parse().unwrap());
                }
                let begin = app.clone().oneshot(begin_request).await.unwrap();
                assert_eq!(begin.status(), StatusCode::OK);
                let cookie = begin.headers()[SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
                    .to_string();
                let selector_header = begin
                    .headers()
                    .get("x-elastos-owner-ceremony")
                    .map(|v| v.to_str().unwrap().to_string());
                let begin: serde_json::Value =
                    serde_json::from_slice(&to_bytes(begin.into_body(), 65536).await.unwrap())
                        .unwrap();
                let ceremony = selector_header
                    .unwrap_or_else(|| begin["ceremony_id"].as_str().unwrap().to_string());
                assert_ne!(ceremony, bearer);
                assert!(ceremony.len() <= 128);
                let options = if direct { &begin } else { &begin["options"] };
                let attestation = crate::auth::owner_attestation_for_test(
                    options["publicKey"]["challenge"].as_str().unwrap(),
                    rp,
                    origin,
                );
                let body = if direct {
                    serde_json::to_value(attestation).unwrap()
                } else {
                    json!({"ceremony_id": ceremony, "response": attestation, "intent": intent})
                };
                if !direct {
                    let auth_before =
                        std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap();
                    for invalid in [String::new(), "bad/name".into(), "x".repeat(65)] {
                        let mut invalid_body = body.clone();
                        invalid_body["intent"]["public_name"] = json!(invalid);
                        let denied = app
                            .clone()
                            .oneshot(request("/complete", &cookie, &ceremony, invalid_body))
                            .await
                            .unwrap();
                        assert_ne!(denied.status(), StatusCode::OK);
                        assert_eq!(
                            std::fs::read(crate::auth::auth_state_path(root.path()).unwrap())
                                .unwrap(),
                            auth_before
                        );
                        assert!(!elastos_identity::IdentityManager::new(root.path().into())
                            .unwrap()
                            .has_credential_history());
                    }
                }
                if direct {
                    let denied = app
                        .clone()
                        .oneshot(request("/complete", &cookie, &bearer, body.clone()))
                        .await
                        .unwrap();
                    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
                    assert!(!elastos_identity::IdentityManager::new(root.path().into())
                        .unwrap()
                        .has_credential_history());
                    assert!(crate::auth::load_auth_state(root.path())
                        .unwrap()
                        .sessions
                        .is_empty());
                }
                crate::auth::fail_owner_enrollment_once_for_test(
                    root.path(),
                    crate::auth::OwnerEnrollmentTestFault::AfterCredential,
                );
                let failed = app
                    .oneshot(request("/complete", &cookie, &ceremony, body))
                    .await
                    .unwrap();
                assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
                assert!(crate::auth::owner_setup_pending(root.path()).unwrap());
                assert_eq!(
                    elastos_identity::IdentityManager::new(root.path().into())
                        .unwrap()
                        .credentials()
                        .len(),
                    1
                );
                assert!(crate::auth::load_auth_state(root.path())
                    .unwrap()
                    .principals
                    .is_empty());
                let persisted =
                    std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap();
                assert!(!String::from_utf8_lossy(&persisted).contains(&bearer));

                // New handler/IdentityManager state; no original attestation in the retry.
                let app = make_router();
                let wrong_cookie = format!(
                    "{}={}",
                    cookie.split('=').next().unwrap(),
                    crate::auth::random_secret_hex()
                );
                let denied = app
                    .clone()
                    .oneshot(request("/begin", &wrong_cookie, "", begin_body.clone()))
                    .await
                    .unwrap();
                assert_eq!(denied.status(), StatusCode::FORBIDDEN);
                assert_eq!(
                    std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap(),
                    persisted
                );
                let resumed = app
                    .clone()
                    .oneshot(request("/begin", &cookie, "", begin_body))
                    .await
                    .unwrap();
                assert_eq!(resumed.status(), StatusCode::OK);
                if direct {
                    assert_eq!(
                        resumed.headers()["x-elastos-owner-ceremony"]
                            .to_str()
                            .unwrap(),
                        ceremony
                    );
                }
                let resumed: serde_json::Value =
                    serde_json::from_slice(&to_bytes(resumed.into_body(), 65536).await.unwrap())
                        .unwrap();
                if direct {
                    assert!(resumed.is_null());
                } else {
                    assert_eq!(resumed["ceremony_id"], ceremony);
                    assert!(resumed["options"].is_null());
                }
                let completion = if direct {
                    serde_json::Value::Null
                } else {
                    json!({"ceremony_id": ceremony, "intent": intent})
                };
                let first = app
                    .clone()
                    .oneshot(request("/complete", &cookie, &ceremony, completion.clone()))
                    .await
                    .unwrap();
                assert_eq!(first.status(), StatusCode::OK);
                let first: serde_json::Value =
                    serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap())
                        .unwrap();
                assert!(first["session_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()));
                let replay = app
                    .oneshot(request("/complete", &cookie, &ceremony, completion))
                    .await
                    .unwrap();
                assert_eq!(replay.status(), StatusCode::OK);
                let replay: serde_json::Value =
                    serde_json::from_slice(&to_bytes(replay.into_body(), 65536).await.unwrap())
                        .unwrap();
                assert!(replay["session_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()));
                assert_eq!(first["session_id"], replay["session_id"]);
                let auth = crate::auth::load_auth_state(root.path()).unwrap();
                let principal = &auth.principals[0];
                let profile = crate::collaboration_profile_authority::load_profile_authority(
                    root.path(),
                    &principal.principal_id,
                    &principal.localhost_root,
                )
                .unwrap();
                if direct {
                    assert!(
                        profile.is_none(),
                        "private account labels do not create Profiles"
                    );
                } else {
                    let profile =
                        profile.expect("verified enrollment establishes the confirmed Profile");
                    assert_eq!(profile.document().display_name, "Shared Name");
                    assert_eq!(
                        profile.document().revision,
                        1,
                        "replay preserves the initial identity"
                    );
                    let protection = crate::auth::load_principal_root_protection(
                        root.path(),
                        &principal.principal_id,
                        &principal.localhost_root,
                    )
                    .unwrap()
                    .unwrap();
                    assert!(protection
                        .protectors
                        .iter()
                        .all(|p| p.verified_at.is_none() && p.profile_coverage.is_none()));
                }
                assert_eq!(
                    (auth.principals.len(), auth.sessions.len(), auth.audit.len()),
                    (1, 1, if direct { 1 } else { 2 })
                );
                assert_eq!(
                    auth.principals[0].role,
                    crate::auth::RuntimePrincipalRole::Admin
                );
                assert_eq!(
                    auth.principals[0].display_name,
                    if direct { "" } else { "Shared Name" }
                );
            }
        }
    }

    async fn assert_guest_registration_rejects_missing_admin(direct: bool, after_begin: bool) {
        use super::super::handlers::identity::{self, IdentityState};
        use axum::{
            body::{to_bytes, Body},
            http::Request,
            routing::post,
            Extension, Router,
        };
        use elastos_runtime::{
            primitives::audit::AuditLog,
            session::{SessionRegistry, SessionType},
        };
        use tower::ServiceExt;

        let root = tempfile::tempdir().unwrap();
        let state = test_gateway_state(root.path());
        store_test_credential(root.path(), test_credential());
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        crate::auth::set_guest_registration_enabled(root.path(), true, crate::auth::now_ts())
            .unwrap();
        let manager = elastos_identity::IdentityManager::new(root.path().into()).unwrap();
        let registry = Arc::new(SessionRegistry::new(Arc::new(AuditLog::new())));
        registry
            .set_default_owner(manager.status().user_id.unwrap())
            .await;
        let session = registry.create_session(SessionType::Shell, None).await;
        let app = if direct {
            Router::new()
                .route("/begin", post(identity::register_begin))
                .route("/complete", post(identity::register_complete))
                .with_state(IdentityState {
                    manager: Arc::new(tokio::sync::Mutex::new(manager)),
                    session_registry: registry,
                    audit_log: None,
                    data_dir: root.path().into(),
                })
                .layer(Extension(session))
        } else {
            Router::new()
                .route("/begin", post(passkey_register_begin))
                .route("/complete", post(passkey_register_complete))
                .with_state(state)
        };
        let request = |path: &str, body: serde_json::Value, cookie: &str| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", "https://elastos.elacitylabs.com")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let intent = json!({"purpose": "recover"});
        let begin_body = if direct {
            json!({})
        } else {
            json!({"intent": intent})
        };
        let mut cookie = String::new();
        let completion = if after_begin {
            let response = app
                .clone()
                .oneshot(request("/begin", begin_body.clone(), ""))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            if !direct {
                cookie = response.headers()[SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
                    .to_string();
                assert!(cookie.starts_with("__Host-elastos-guest-registration="));
            }
            let begin: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            let options = if direct { &begin } else { &begin["options"] };
            let attestation = crate::auth::owner_attestation_for_test(
                options["publicKey"]["challenge"].as_str().unwrap(),
                "elastos.elacitylabs.com",
                "https://elastos.elacitylabs.com",
            );
            if direct {
                serde_json::to_value(attestation).unwrap()
            } else {
                json!({"ceremony_id": begin["ceremony_id"], "response": attestation, "intent": intent})
            }
        } else {
            begin_body
        };
        // Revocation occurs after the valid begin in the completion cases.
        let mut auth = crate::auth::load_auth_state(root.path()).unwrap();
        auth.principals[0]
            .proof_binding
            .passkey
            .as_mut()
            .unwrap()
            .revoked_at = Some(crate::auth::now_ts());
        crate::auth::save_auth_state(root.path(), &auth).unwrap();
        let auth_path = crate::auth::auth_state_path(root.path()).unwrap();
        let identity_path = root.path().join("identity/credentials.json");
        let auth_before = std::fs::read(&auth_path).unwrap();
        let identity_before = std::fs::read(&identity_path).unwrap();
        let result = app
            .oneshot(request(
                if after_begin { "/complete" } else { "/begin" },
                completion,
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            std::fs::read(auth_path).unwrap(),
            auth_before,
            "guest denial changed auth state"
        );
        assert_eq!(
            std::fs::read(identity_path).unwrap(),
            identity_before,
            "guest denial persisted an unusable credential"
        );
    }

    #[tokio::test]
    async fn guest_registration_gateway_rejects_no_live_admin_begin() {
        assert_guest_registration_rejects_missing_admin(false, false).await;
    }

    #[tokio::test]
    async fn guest_registration_gateway_rejects_no_live_admin_completion() {
        assert_guest_registration_rejects_missing_admin(false, true).await;
    }

    #[tokio::test]
    async fn guest_registration_direct_rejects_no_live_admin_begin() {
        assert_guest_registration_rejects_missing_admin(true, false).await;
    }

    #[tokio::test]
    async fn guest_registration_direct_rejects_no_live_admin_completion() {
        assert_guest_registration_rejects_missing_admin(true, true).await;
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
    async fn first_owner_registration_public_begin_returns_forbidden() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://home.example"));
        let response = passkey_register_begin(
            State(state.clone()),
            None,
            headers,
            Json(PasskeyRegisterBeginRequest {
                intent: Some(crate::auth::PasskeyEnrollmentIntent::Recover {}),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            !state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .status()
                .registered
        );
        assert_eq!(
            crate::auth::active_passkey_principal_count(temp.path()).unwrap(),
            0
        );
    }

    fn guided_owner_test_router(state: GatewayState) -> axum::Router {
        axum::Router::new()
            .route("/begin", axum::routing::post(passkey_register_begin))
            .route("/complete", axum::routing::post(passkey_register_complete))
            .with_state(state)
    }

    async fn guided_owner_http(
        app: &axum::Router,
        path: &str,
        cookie: &str,
        body: serde_json::Value,
    ) -> (StatusCode, String, serde_json::Value) {
        use tower::ServiceExt;
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", "localhost:61180")
                    .header("origin", "http://localhost:61180")
                    .header("cookie", cookie)
                    .header("content-type", "application/json")
                    .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        assert!(response.headers().get_all(SET_COOKIE).iter().count() <= 1);
        let cookie = response
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .map(|value| {
                value
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
                    .to_string()
            })
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
        (status, cookie, body)
    }

    #[tokio::test]
    async fn guided_owner_http_rejects_malformed_intent_before_credentials() {
        for body in [
            json!({}),
            json!({"intent": null}),
            json!({"unexpected": true}),
            json!({"intent": {"purpose": "recover", "public_name": "Alice"}}),
            json!({"intent": {"purpose": "create"}}),
            json!({"intent": {"purpose": "unknown"}}),
            json!({"intent": {"purpose": "recover"}, "display_name": "Alice"}),
            json!({"intent": {"purpose": "recover"}, "display_name": null}),
            json!({"intent": {"purpose": "recover"}, "profile_display_name": "Alice"}),
            json!({"intent": {"purpose": "recover"}, "profile_display_name": null}),
        ] {
            let root = local_profile_fixture_root();
            let state = test_gateway_state(root.path());
            let app = guided_owner_test_router(state.clone());
            let (status, cookie, output) =
                guided_owner_http(&app, "/begin", "", body.clone()).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {output}");
            assert!(
                cookie.is_empty(),
                "rejected begin issued an enrollment cookie"
            );
            assert!(state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .credentials()
                .is_empty());
            assert!(crate::auth::load_auth_state(root.path())
                .unwrap()
                .principals
                .is_empty());
        }
        for name in ["", " Alice ", "Alice\nSmith", "Person", "ElastOS Home"] {
            let root = local_profile_fixture_root();
            let state = test_gateway_state(root.path());
            let app = guided_owner_test_router(state.clone());
            let (status, _, output) = guided_owner_http(
                &app,
                "/begin",
                "",
                json!({
                    "intent": {"purpose": "create", "public_name": name}
                }),
            )
            .await;
            assert!(!status.is_success(), "accepted {name:?}: {output}");
            assert!(state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .credentials()
                .is_empty());
            assert!(crate::auth::load_auth_state(root.path())
                .unwrap()
                .principals
                .is_empty());
        }
    }

    #[tokio::test]
    async fn guided_enrollment_http_rejects_name_before_effects_then_accepts_correction() {
        for guest in [false, true] {
            let root = local_profile_fixture_root();
            let state = test_gateway_state(root.path());
            if guest {
                let credential = test_credential();
                store_test_credential(root.path(), credential.clone());
                seed_test_passkey_principal(
                    &state,
                    &credential,
                    "http://localhost:61180",
                    crate::auth::RuntimePrincipalRole::Admin,
                );
                crate::auth::set_guest_registration_enabled(
                    root.path(),
                    true,
                    crate::auth::now_ts(),
                )
                .unwrap();
            }
            let app = guided_owner_test_router(state.clone());
            let credentials_before = state.identity_manager().unwrap().lock().await.credentials();
            let auth_before = crate::auth::load_auth_state(root.path()).unwrap();
            let auth_path = crate::auth::auth_state_path(root.path()).unwrap();
            let auth_bytes_before = std::fs::read(&auth_path).ok();
            let credential_path = root.path().join("identity/credentials.json");
            let credential_bytes_before = std::fs::read(&credential_path).ok();
            for name in [
                "Person",
                "ElastOS Home",
                "ElastOS user",
                "Device deadbeef",
                "",
                " Alice ",
                "Alice/Smith",
                "Alice\nSmith",
                "Alice\u{0085}Smith",
                "Alice\u{009f}Smith",
            ] {
                let (status, cookie, output) = guided_owner_http(
                    &app,
                    "/begin",
                    "",
                    json!({"intent": {"purpose": "create", "public_name": name}}),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "guest={guest}, name={name:?}: {output}"
                );
                assert!(
                    cookie.is_empty(),
                    "rejected name issued an enrollment cookie"
                );
                assert_eq!(std::fs::read(&auth_path).ok(), auth_bytes_before);
                assert_eq!(
                    std::fs::read(&credential_path).ok(),
                    credential_bytes_before
                );
                assert_eq!(
                    serde_json::to_value(crate::auth::load_auth_state(root.path()).unwrap())
                        .unwrap(),
                    serde_json::to_value(&auth_before).unwrap(),
                    "rejected name changed auth or enrollment claim state"
                );
                assert_eq!(
                    serde_json::to_value(
                        state.identity_manager().unwrap().lock().await.credentials()
                    )
                    .unwrap(),
                    serde_json::to_value(&credentials_before).unwrap()
                );
            }
            let intent = json!({"purpose": "create", "public_name": "Alice"});
            let (status, cookie, begin) =
                guided_owner_http(&app, "/begin", "", json!({"intent": intent})).await;
            assert_eq!(status, StatusCode::OK, "corrected name: {begin}");
            assert!(cookie.starts_with(if guest {
                "elastos-guest-registration="
            } else {
                "elastos-owner-claim="
            }));
            let response = crate::auth::owner_attestation_for_test(
                begin["options"]["publicKey"]["challenge"].as_str().unwrap(),
                "localhost",
                "http://localhost:61180",
            );
            let (status, _, completed) = guided_owner_http(
                &app,
                "/complete",
                &cookie,
                json!({"ceremony_id": begin["ceremony_id"], "response": response, "intent": intent}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "corrected completion: {completed}");
            let after = crate::auth::load_auth_state(root.path()).unwrap();
            assert_eq!(after.principals.len(), auth_before.principals.len() + 1);
            assert_eq!(after.sessions.len(), 1);
            assert_eq!(
                state
                    .identity_manager()
                    .unwrap()
                    .lock()
                    .await
                    .credentials()
                    .len(),
                credentials_before.len() + 1
            );
            let principal = crate::auth::load_principal_for_proof_binding(
                root.path(),
                completed["proof_binding_id"].as_str().unwrap(),
            )
            .unwrap();
            assert_eq!(
                principal.role,
                if guest {
                    crate::auth::RuntimePrincipalRole::Guest
                } else {
                    crate::auth::RuntimePrincipalRole::Admin
                }
            );
            let profile = crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &principal.principal_id,
                &principal.localhost_root,
            )
            .unwrap()
            .unwrap();
            assert_eq!(profile.document().display_name, "Alice");
            assert_eq!(
                principal.initial_profile_display_name.as_deref(),
                Some("Alice")
            );
        }
    }

    #[tokio::test]
    async fn guided_owner_http_rejects_missing_intent_and_legacy_names_at_completion() {
        let root = local_profile_fixture_root();
        let state = test_gateway_state(root.path());
        let app = guided_owner_test_router(state.clone());
        let intent = json!({"purpose": "create", "public_name": "Alice"});
        let (status, cookie, begin) =
            guided_owner_http(&app, "/begin", "", json!({"intent": intent})).await;
        assert_eq!(status, StatusCode::OK);
        let response = crate::auth::owner_attestation_for_test(
            begin["options"]["publicKey"]["challenge"].as_str().unwrap(),
            "localhost",
            "http://localhost:61180",
        );
        let completion =
            json!({"ceremony_id": begin["ceremony_id"], "response": response, "intent": intent});
        let auth_path = crate::auth::auth_state_path(root.path()).unwrap();
        let before = std::fs::read(&auth_path).unwrap();
        for (field, value) in [
            ("intent", None),
            ("intent", Some(serde_json::Value::Null)),
            ("display_name", Some(json!("Alice"))),
            ("display_name", Some(serde_json::Value::Null)),
            ("profile_display_name", Some(json!("Alice"))),
            ("profile_display_name", Some(serde_json::Value::Null)),
        ] {
            let mut invalid = completion.clone();
            if let Some(value) = value {
                invalid[field] = value;
            } else {
                invalid.as_object_mut().unwrap().remove(field);
            }
            let (status, new_cookie, output) =
                guided_owner_http(&app, "/complete", &cookie, invalid.clone()).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{invalid}: {output}"
            );
            assert!(
                new_cookie.is_empty(),
                "rejected completion changed a cookie"
            );
            assert_eq!(std::fs::read(&auth_path).unwrap(), before);
            assert!(state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .credentials()
                .is_empty());
            assert!(crate::auth::load_auth_state(root.path())
                .unwrap()
                .principals
                .is_empty());
        }
    }

    #[tokio::test]
    async fn guided_owner_http_preserves_intent_and_exact_retry_after_restart() {
        for intent in [
            json!({"purpose": "create", "public_name": "Alice"}),
            json!({"purpose": "recover"}),
        ] {
            let root = local_profile_fixture_root();
            let state = test_gateway_state(root.path());
            let app = guided_owner_test_router(state.clone());
            let (status, cookie, begin) =
                guided_owner_http(&app, "/begin", "", json!({"intent": intent})).await;
            assert_eq!(status, StatusCode::OK, "{begin}");
            let response = crate::auth::owner_attestation_for_test(
                begin["options"]["publicKey"]["challenge"].as_str().unwrap(),
                "localhost",
                "http://localhost:61180",
            );
            let completion = json!({"ceremony_id": begin["ceremony_id"], "response": response, "intent": intent});
            let auth_path = crate::auth::auth_state_path(root.path()).unwrap();
            let before = std::fs::read(&auth_path).unwrap();
            let (status, _, _) = guided_owner_http(
                &app,
                "/begin",
                &cookie,
                json!({"intent": {"purpose": "create", "public_name": "Other"}}),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            for replacement in [
                None,
                Some(json!({"purpose": "create", "public_name": "Other"})),
                Some(if intent["purpose"] == "create" {
                    json!({"purpose": "recover"})
                } else {
                    json!({"purpose": "create", "public_name": "Alice"})
                }),
            ] {
                let mut invalid = completion.clone();
                let expected = if replacement.is_none() {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::FORBIDDEN
                };
                if let Some(replacement) = replacement {
                    invalid["intent"] = replacement;
                } else {
                    invalid.as_object_mut().unwrap().remove("intent");
                }
                let (status, _, output) =
                    guided_owner_http(&app, "/complete", &cookie, invalid).await;
                assert_eq!(status, expected, "{output}");
            }
            let mut invalid = completion.clone();
            invalid["unexpected"] = json!(true);
            assert_eq!(
                guided_owner_http(&app, "/complete", &cookie, invalid)
                    .await
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
            let mut invalid = completion.clone();
            invalid["profile_display_name"] = json!("Injected");
            assert_eq!(
                guided_owner_http(&app, "/complete", &cookie, invalid)
                    .await
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(std::fs::read(&auth_path).unwrap(), before);
            assert!(state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .credentials()
                .is_empty());

            crate::auth::fail_owner_enrollment_once_for_test(
                root.path(),
                crate::auth::OwnerEnrollmentTestFault::AfterTerminal,
            );
            assert_eq!(
                guided_owner_http(&app, "/complete", &cookie, completion.clone())
                    .await
                    .0,
                StatusCode::INTERNAL_SERVER_ERROR
            );
            drop(app);
            drop(state);
            let state = test_gateway_state(root.path());
            let app = guided_owner_test_router(state.clone());
            let (status, _, completed) =
                guided_owner_http(&app, "/complete", &cookie, completion.clone()).await;
            assert_eq!(status, StatusCode::OK, "{completed}");
            let principal = crate::auth::load_principal_for_proof_binding(
                root.path(),
                completed["proof_binding_id"].as_str().unwrap(),
            )
            .unwrap();
            let profile = crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &principal.principal_id,
                &principal.localhost_root,
            )
            .unwrap();
            if intent["purpose"] == "create" {
                assert_eq!(profile.as_ref().unwrap().document().display_name, "Alice");
                assert_eq!(
                    principal.initial_profile_display_name.as_deref(),
                    Some("Alice")
                );
            } else {
                assert!(profile.is_none());
                assert!(principal.initial_profile_display_name.is_none());
            }
            let profile_before =
                profile.map(|value| serde_json::to_value(value.document()).unwrap());
            let mut retry = completion;
            retry.as_object_mut().unwrap().remove("response");
            let (status, _, replay) = guided_owner_http(&app, "/complete", &cookie, retry).await;
            assert_eq!(status, StatusCode::OK, "{replay}");
            assert_eq!(replay["session_id"], completed["session_id"]);
            assert_eq!(
                crate::collaboration_profile_authority::load_profile_authority(
                    root.path(),
                    &principal.principal_id,
                    &principal.localhost_root
                )
                .unwrap()
                .map(|value| serde_json::to_value(value.document()).unwrap()),
                profile_before
            );
            let auth = crate::auth::load_auth_state(root.path()).unwrap();
            assert_eq!(
                (
                    auth.principals.len(),
                    auth.sessions.len(),
                    state
                        .identity_manager()
                        .unwrap()
                        .lock()
                        .await
                        .credentials()
                        .len()
                ),
                (1, 1, 1)
            );
        }
    }

    #[tokio::test]
    async fn guided_guest_https_cookie_is_separate_from_remote_owner_authority() {
        use tower::ServiceExt;
        let root = local_profile_fixture_root();
        let state = test_gateway_state(root.path());
        let app = guided_owner_test_router(state.clone());
        let request = |cookie: &str| {
            axum::http::Request::builder()
                .method("POST")
                .uri("/begin")
                .header("host", "home.example")
                .header("origin", "https://home.example")
                .header("x-forwarded-proto", "https")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .extension(ConnectInfo("192.0.2.5:1234".parse::<SocketAddr>().unwrap()))
                .body(axum::body::Body::from(
                    json!({"intent": {"purpose": "recover"}}).to_string(),
                ))
                .unwrap()
        };
        let guest_cookie = format!("__Host-elastos-guest-registration={}", "a".repeat(64));
        let denied = app.clone().oneshot(request(&guest_cookie)).await.unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(denied.headers().get(SET_COOKIE).is_none());
        assert!(state
            .identity_manager()
            .unwrap()
            .lock()
            .await
            .credentials()
            .is_empty());
        let credential = test_credential();
        store_test_credential(root.path(), credential.clone());
        seed_test_passkey_principal(
            &state,
            &credential,
            "https://home.example",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        crate::auth::set_guest_registration_enabled(root.path(), true, crate::auth::now_ts())
            .unwrap();
        let allowed = app.oneshot(request("")).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        let cookies = allowed
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(cookies.len(), 1);
        let cookie = cookies[0].to_str().unwrap();
        assert!(cookie.starts_with("__Host-elastos-guest-registration="));
        assert!(cookie.ends_with("; Path=/; HttpOnly; SameSite=Strict; Secure"));
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("home.example"));
        headers.insert("origin", HeaderValue::from_static("https://home.example"));
        headers.insert(
            "cookie",
            HeaderValue::from_str(cookie.split(';').next().unwrap()).unwrap(),
        );
        assert!(owner_claim(&headers).unwrap().is_none());
        assert!(
            guest_registration_client_claim(&headers, "https://home.example")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn guided_guest_http_disabled_policy_rejects_intent_before_effects() {
        let root = local_profile_fixture_root();
        let state = test_gateway_state(root.path());
        let credential = test_credential();
        store_test_credential(root.path(), credential.clone());
        seed_test_passkey_principal(
            &state,
            &credential,
            "http://localhost:61180",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        let app = guided_owner_test_router(state.clone());
        let auth_path = crate::auth::auth_state_path(root.path()).unwrap();
        let before = std::fs::read(&auth_path).unwrap();
        let credential_path = root.path().join("identity/credentials.json");
        let credentials_before = std::fs::read(&credential_path).unwrap();
        for intent in [
            json!({"purpose": "create", "public_name": "Alice"}),
            json!({"purpose": "recover"}),
        ] {
            assert_eq!(
                guided_owner_http(&app, "/begin", "", json!({"intent": intent}))
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(std::fs::read(&auth_path).unwrap(), before);
            assert_eq!(std::fs::read(&credential_path).unwrap(), credentials_before);
        }
    }

    #[tokio::test]
    async fn guided_guest_http_create_recover_keeps_role_and_replays_completion() {
        for intent in [
            json!({"purpose": "create", "public_name": "Alice"}),
            json!({"purpose": "recover"}),
        ] {
            let root = local_profile_fixture_root();
            let state = test_gateway_state(root.path());
            let credential = test_credential();
            store_test_credential(root.path(), credential.clone());
            seed_test_passkey_principal(
                &state,
                &credential,
                "http://localhost:61180",
                crate::auth::RuntimePrincipalRole::Admin,
            );
            crate::auth::set_guest_registration_enabled(root.path(), true, crate::auth::now_ts())
                .unwrap();
            let admin = crate::auth::load_auth_state(root.path())
                .unwrap()
                .principals[0]
                .clone();
            let app = guided_owner_test_router(state.clone());
            let (status, cookie, begin) =
                guided_owner_http(&app, "/begin", "", json!({"intent": intent})).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "enabled guest {intent} rejected: {begin}"
            );
            assert!(cookie.starts_with("elastos-guest-registration="));
            assert!(!cookie.contains("owner-claim"));
            let response = crate::auth::owner_attestation_for_test(
                begin["options"]["publicKey"]["challenge"].as_str().unwrap(),
                "localhost",
                "http://localhost:61180",
            );
            let completion = json!({
                "ceremony_id": begin["ceremony_id"],
                "response": response,
                "intent": intent,
            });
            let credentials_before =
                std::fs::read(root.path().join("identity/credentials.json")).unwrap();
            for wrong_cookie in [
                String::new(),
                format!("elastos-owner-claim={}", "a".repeat(64)),
                format!("elastos-guest-registration={}", "b".repeat(64)),
                format!("{cookie}; {cookie}"),
            ] {
                assert_eq!(
                    guided_owner_http(&app, "/complete", &wrong_cookie, completion.clone())
                        .await
                        .0,
                    StatusCode::FORBIDDEN
                );
            }
            let mut missing_intent = completion.clone();
            missing_intent.as_object_mut().unwrap().remove("intent");
            assert_eq!(
                guided_owner_http(&app, "/complete", &cookie, missing_intent)
                    .await
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
            let mut changed_intent = completion.clone();
            changed_intent["intent"] = json!({"purpose": "create", "public_name": "Other"});
            assert_eq!(
                guided_owner_http(&app, "/complete", &cookie, changed_intent)
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                std::fs::read(root.path().join("identity/credentials.json")).unwrap(),
                credentials_before
            );
            // The server accepts completion, but the client loses the response.
            let (status, _, _) =
                guided_owner_http(&app, "/complete", &cookie, completion.clone()).await;
            assert_eq!(status, StatusCode::OK);
            let settled = crate::auth::load_auth_state(root.path()).unwrap();
            assert_eq!((settled.principals.len(), settled.sessions.len()), (2, 1));
            let guest = settled
                .principals
                .iter()
                .find(|principal| principal.principal_id != admin.principal_id)
                .unwrap();
            assert_eq!(guest.role, crate::auth::RuntimePrincipalRole::Guest);
            let profile_before = crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &guest.principal_id,
                &guest.localhost_root,
            )
            .unwrap()
            .map(|profile| serde_json::to_value(profile.document()).unwrap());
            if intent["purpose"] == "create" {
                assert_eq!(guest.initial_profile_display_name.as_deref(), Some("Alice"));
                assert_eq!(profile_before.as_ref().unwrap()["display_name"], "Alice");
            } else {
                assert!(guest.initial_profile_display_name.is_none());
                assert!(profile_before.is_none());
            }
            drop(app);
            drop(state);
            let state = test_gateway_state(root.path());
            let app = guided_owner_test_router(state.clone());
            for _ in 0..2 {
                let (status, _, replay) =
                    guided_owner_http(&app, "/complete", &cookie, completion.clone()).await;
                assert_eq!(status, StatusCode::OK, "{intent} retry failed: {replay}");
                assert_eq!(replay["principal_id"], guest.principal_id);
                assert_eq!(replay["session_id"], settled.sessions[0].grant.session_id);
                assert_eq!(replay["proof_binding_id"], guest.proof_binding_id);
            }
            let after = crate::auth::load_auth_state(root.path()).unwrap();
            assert_eq!((after.principals.len(), after.sessions.len()), (2, 1));
            let same_admin = after
                .principals
                .iter()
                .find(|principal| principal.principal_id == admin.principal_id)
                .unwrap();
            assert_eq!(
                serde_json::to_value(same_admin).unwrap(),
                serde_json::to_value(&admin).unwrap()
            );
            assert_eq!(
                after
                    .principals
                    .iter()
                    .filter(|principal| principal.role == crate::auth::RuntimePrincipalRole::Guest)
                    .count(),
                1
            );
            let credentials = state.identity_manager().unwrap().lock().await.credentials();
            assert_eq!(credentials.len(), 2);
            assert_eq!(
                credentials
                    .iter()
                    .filter(|item| item.credential_id == credential.credential_id)
                    .count(),
                1
            );
            assert_eq!(
                serde_json::to_value(
                    credentials
                        .iter()
                        .find(|item| item.credential_id == credential.credential_id)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(&credential).unwrap()
            );
            assert_eq!(
                crate::collaboration_profile_authority::load_profile_authority(
                    root.path(),
                    &guest.principal_id,
                    &guest.localhost_root,
                )
                .unwrap()
                .map(|profile| serde_json::to_value(profile.document()).unwrap()),
                profile_before
            );
        }
    }

    #[tokio::test]
    async fn first_owner_registration_public_complete_returns_forbidden() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://home.example"));
        let input = serde_json::from_value(json!({
            "ceremony_id": "passkey:register:absent",
            "intent": {"purpose": "recover"},
            "response": {
                "id": "fixture", "rawId": "fixture", "type": "public-key",
                "response": { "clientDataJson": "AA", "attestationObject": "AA" }
            }
        }))
        .unwrap();
        let response =
            passkey_register_complete(State(state.clone()), None, headers, Json(input)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            !state
                .identity_manager()
                .unwrap()
                .lock()
                .await
                .status()
                .registered
        );
        assert_eq!(
            crate::auth::active_passkey_principal_count(temp.path()).unwrap(),
            0
        );
    }

    #[test]
    fn registration_denial_is_typed_even_with_context() {
        for error in [
            anyhow::Error::new(crate::auth::OwnerEnrollmentDenied),
            anyhow::Error::new(PasskeyRegistrationDenied::GuestRegistrationDisabled),
        ] {
            let error = error.context("enrollment refused");
            assert_eq!(auth_error_response(error).status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(
            auth_error_response(anyhow::anyhow!("storage failure")).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn owner_enrollment_orphan_login_cannot_create_authority() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        let credentials_path = temp.path().join("identity/credentials.json");
        let before = std::fs::read(&credentials_path).unwrap();
        assert!(issue_named_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://home.example",
            true,
            "passkey authentication verified and session granted",
            None
        )
        .is_err());
        assert_eq!(std::fs::read(credentials_path).unwrap(), before);
        let auth = crate::auth::load_auth_state(temp.path()).unwrap();
        assert!(auth.principals.is_empty());
        assert!(auth.sessions.is_empty());
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

        let response = passkey_register_begin_inner(&state, &headers, false, None)
            .await
            .unwrap_err();

        assert!(response
            .downcast_ref::<crate::auth::OwnerEnrollmentDenied>()
            .is_some());
    }

    #[tokio::test]
    async fn first_owner_registration_accepts_local_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        headers.insert("origin", HeaderValue::from_static("http://localhost:61180"));
        prepare_owner_claim(&mut headers, true).unwrap();

        let response = passkey_register_begin_inner(&state, &headers, true, None)
            .await
            .unwrap();

        assert_eq!(response.schema, "elastos.auth.passkey.register.begin/v1");
        assert_eq!(
            response.options.as_ref().unwrap().public_key.rp.id,
            "localhost"
        );
    }

    #[tokio::test]
    async fn allowed_https_guest_registration_binds_begin_origin() {
        let temp = local_profile_fixture_root();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        seed_test_passkey_principal(
            &state,
            &credential,
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        let admin = issue_passkey_session_grant(
            &state,
            "identity-test",
            &credential,
            "https://elastos.elacitylabs.com",
            true,
            "test existing admin",
        )
        .unwrap();
        crate::auth::set_guest_registration_enabled(temp.path(), true, crate::auth::now_ts())
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://home.example"));
        let begin = passkey_register_begin_inner(&state, &headers, false, None)
            .await
            .unwrap();
        assert_eq!(
            begin.options.as_ref().unwrap().public_key.rp.id,
            "home.example"
        );

        // A fmt:none ES256 attestation using the P-256 generator point.
        let credential_id = b"https-guest";
        let mut auth_data = Sha256::digest(b"home.example").to_vec();
        auth_data.push(0x45); // user present, user verified, attested credential
        auth_data.extend_from_slice(&[0; 20]); // sign count and AAGUID
        auth_data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        auth_data.extend_from_slice(credential_id);
        auth_data.extend_from_slice(
            &hex::decode(concat!(
                "a5010203262001215820",
                "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
                "225820",
                "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5"
            ))
            .unwrap(),
        );
        let mut attestation = vec![0xa3, 0x63];
        attestation.extend_from_slice(b"fmt");
        attestation.push(0x64);
        attestation.extend_from_slice(b"none");
        attestation.push(0x67);
        attestation.extend_from_slice(b"attStmt");
        attestation.extend_from_slice(&[0xa0, 0x68]);
        attestation.extend_from_slice(b"authData");
        attestation.extend_from_slice(&[0x58, u8::try_from(auth_data.len()).unwrap()]);
        attestation.extend_from_slice(&auth_data);
        let response_for = |challenge: &str| {
            serde_json::from_value::<RegistrationResponse>(json!({
                "id": URL_SAFE_NO_PAD.encode(credential_id),
                "rawId": URL_SAFE_NO_PAD.encode(credential_id),
                "type": "public-key",
                "response": {
                    "clientDataJson": URL_SAFE_NO_PAD.encode(json!({
                        "type": "webauthn.create", "challenge": challenge,
                        "origin": "https://home.example"
                    }).to_string()),
                    "attestationObject": URL_SAFE_NO_PAD.encode(&attestation)
                }
            }))
            .unwrap()
        };
        let response = response_for(&begin.options.as_ref().unwrap().public_key.challenge);
        let result = passkey_register_complete_inner(
            &state,
            &headers,
            PasskeyRegisterCompleteRequest {
                ceremony_id: begin.ceremony_id,
                intent: None,
                response: Some(response),
                display_name: Some("Private guest label".into()),
                profile_display_name: Some("Public guest name".into()),
            },
            false,
        )
        .await
        .unwrap();
        assert!(!result.home_token.is_empty());
        assert_ne!(result.principal_id, admin.principal_id);
        let guest_principal =
            crate::auth::load_principal_for_proof_binding(temp.path(), &result.proof_binding_id)
                .unwrap();
        assert_eq!(guest_principal.display_name, "Private guest label");
        assert_eq!(
            guest_principal.initial_profile_display_name.as_deref(),
            Some("Public guest name")
        );
        assert_eq!(
            crate::collaboration_profile_authority::load_profile_authority(
                temp.path(),
                &guest_principal.principal_id,
                &guest_principal.localhost_root
            )
            .unwrap()
            .unwrap()
            .document()
            .display_name,
            "Public guest name"
        );
        assert_eq!(
            crate::auth::load_principal_for_proof_binding(temp.path(), &result.proof_binding_id)
                .unwrap()
                .role,
            crate::auth::RuntimePrincipalRole::Guest
        );
        let manager = state.identity_manager().unwrap();
        let manager = manager.lock().await;
        assert_eq!(manager.credentials().len(), 2);
        assert!(manager
            .credentials()
            .iter()
            .any(|credential| credential.rp_id == "home.example"));
        drop(manager);
        let credential_path = temp.path().join("identity/credentials.json");
        let auth_path = crate::auth::auth_state_path(temp.path()).unwrap();
        let credential_bytes = std::fs::read(&credential_path).unwrap();
        let auth_bytes = std::fs::read(&auth_path).unwrap();
        let fresh = passkey_register_begin_inner(&state, &headers, false, None)
            .await
            .unwrap();
        assert!(passkey_register_complete_inner(
            &state,
            &headers,
            PasskeyRegisterCompleteRequest {
                ceremony_id: fresh.ceremony_id,
                intent: None,
                response: Some(response_for(
                    &fresh.options.as_ref().unwrap().public_key.challenge
                )),
                display_name: None,
                profile_display_name: None,
            },
            false,
        )
        .await
        .is_err());
        // A fresh fmt:none ceremony cannot adopt another person's existing key.
        assert_eq!(std::fs::read(&credential_path).unwrap(), credential_bytes);
        assert_eq!(std::fs::read(&auth_path).unwrap(), auth_bytes);
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
        seed_test_passkey_principal(
            &state,
            &credential,
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );

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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );

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

    #[tokio::test]
    async fn passkey_list_returns_runtime_bound_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_gateway_state(temp.path());
        let credential = test_credential();
        store_test_credential(temp.path(), credential.clone());
        seed_test_passkey_principal(
            &state,
            &credential,
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        crate::auth::set_principal_display_name(
            temp.path(),
            &crate::auth::load_auth_state(temp.path())
                .unwrap()
                .principals[0]
                .proof_binding_id,
            "Work laptop",
            crate::auth::now_ts(),
        )
        .unwrap();
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
                profile_coverage: None,
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
                profile_coverage: None,
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
            seed_test_passkey_principal(
                &state,
                &test_credential(),
                "https://elastos.elacitylabs.com",
                crate::auth::RuntimePrincipalRole::Admin,
            );
            seed_test_passkey_principal(
                &state,
                &test_credential_2(),
                "https://elastos.elacitylabs.com",
                crate::auth::RuntimePrincipalRole::Guest,
            );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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

        let denied = passkey_register_begin_inner(&state, &empty_headers, false, None)
            .await
            .unwrap_err()
            .to_string();
        let admin_denied = passkey_register_begin_inner(
            &state,
            &home_token_headers(&grant.home_token),
            false,
            None,
        )
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
        let guest_denied = passkey_register_begin_inner(
            &state,
            &home_token_headers(&guest.home_token),
            false,
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        crate::auth::set_guest_registration_enabled(temp.path(), true, crate::auth::now_ts())
            .unwrap();
        let public_allowed = passkey_register_begin_inner(&state, &empty_headers, false, None)
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
            .as_ref()
            .unwrap()
            .public_key
            .exclude_credentials
            .is_empty());
    }

    fn local_profile_fixture_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    fn local_profile_owner_grant(
        data_dir: &std::path::Path,
    ) -> (StoredCredential, AuthSessionGrantV1) {
        let secret = crate::auth::random_secret_hex();
        let admission = crate::auth::OwnerAdmission {
            origin: "http://localhost:61180",
            rp_id: "localhost",
            claimant: &secret,
            loopback: true,
        };
        let mut identity = elastos_identity::IdentityManager::new(data_dir.into()).unwrap();
        let (ceremony, options) = crate::auth::begin_owner_enrollment(
            data_dir,
            &mut identity,
            &admission,
            "confirmed-owner",
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        let response = crate::auth::owner_attestation_for_test(
            &options.unwrap().public_key.challenge,
            admission.rp_id,
            admission.origin,
        );
        let grant = crate::auth::complete_owner_enrollment_with_names(
            data_dir,
            &mut identity,
            &admission,
            &ceremony,
            Some(&response),
            crate::auth::OwnerEnrollmentNames {
                display_name: Some("Private label"),
                profile_display_name: Some("Public name"),
            },
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        let before = std::fs::read(crate::auth::auth_state_path(data_dir).unwrap()).unwrap();
        assert!(crate::auth::complete_owner_enrollment_with_names(
            data_dir,
            &mut identity,
            &admission,
            &ceremony,
            None,
            crate::auth::OwnerEnrollmentNames {
                display_name: None,
                profile_display_name: Some("Changed consent")
            },
            crate::auth::now_ts(),
        )
        .is_err());
        assert_eq!(
            std::fs::read(crate::auth::auth_state_path(data_dir).unwrap()).unwrap(),
            before
        );
        (identity.credentials()[0].clone(), grant)
    }

    fn local_profile_sign_in(
        data_dir: &std::path::Path,
        credential: &StoredCredential,
    ) -> AuthSessionGrantV1 {
        crate::auth::grant_passkey_session(
            data_dir,
            crate::auth::PasskeySessionRequest {
                credential,
                origin: "http://localhost:61180",
                user_verified: true,
                display_name: None,
                profile_display_name: None,
                reason: "onboarding retry fixture",
                purpose: crate::auth::PasskeySessionPurpose::SignIn,
            },
        )
        .unwrap()
    }

    #[test]
    fn local_profile_setup_resumes_with_current_grant_after_interruption() {
        for fault in ["after-terminal", "after-root", "after-profile"] {
            for prune in [false, true] {
                let root = local_profile_fixture_root();
                let _auth = super::super::gateway::set_test_home_launch_auth_data_dir(root.path());
                let state = test_gateway_state(root.path());
                let (credential, original) = local_profile_owner_grant(root.path());
                let credentials =
                    std::fs::read(root.path().join("identity/credentials.json")).unwrap();
                if fault != "after-terminal" {
                    LOCAL_PROFILE_SETUP_FAULT
                        .with(|slot| *slot.borrow_mut() = Some((root.path().into(), fault)));
                    assert!(passkey_response_for_grant(&state, original.clone()).is_err());
                }
                let before = crate::auth::load_auth_state(root.path()).unwrap();
                let protections = serde_json::to_value(&before.principal_root_protections).unwrap();
                let mut auth = before;
                if prune {
                    auth.sessions.clear();
                } else {
                    auth.sessions[0].revoked_at = Some(crate::auth::now_ts());
                }
                crate::auth::save_auth_state(root.path(), &auth).unwrap();
                assert!(
                    crate::auth::confirmed_initial_profile_name(root.path(), &original).is_err()
                );
                let current = local_profile_sign_in(root.path(), &credential);
                assert_eq!(
                    crate::auth::confirmed_initial_profile_name(root.path(), &current)
                        .unwrap()
                        .as_deref(),
                    Some("Public name")
                );
                let restarted = test_gateway_state(root.path());
                let response = passkey_response_for_grant(&restarted, current.clone()).unwrap();
                assert_eq!(response.principal_id, original.principal_id);
                assert_eq!(response.proof_binding_id, original.proof_binding_id);
                let principal = crate::auth::load_principal_for_proof_binding(
                    root.path(),
                    &current.proof_binding_id,
                )
                .unwrap();
                assert_eq!(principal.display_name, "Private label");
                let profile = crate::collaboration_profile_authority::load_profile_authority(
                    root.path(),
                    &principal.principal_id,
                    &principal.localhost_root,
                )
                .unwrap()
                .unwrap();
                assert_eq!(profile.document().display_name, "Public name");
                assert_eq!(profile.document().revision, 1);
                let auth = crate::auth::load_auth_state(root.path()).unwrap();
                if fault != "after-terminal" {
                    assert_eq!(
                        serde_json::to_value(&auth.principal_root_protections).unwrap(),
                        protections
                    );
                }
                assert!(auth
                    .principal_root_protections
                    .iter()
                    .flat_map(|p| &p.protectors)
                    .all(|p| p.verified_at.is_none() && p.profile_coverage.is_none()));
                assert_eq!(
                    std::fs::read(root.path().join("identity/credentials.json")).unwrap(),
                    credentials
                );
                let stable_auth =
                    std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap();
                let profile_path = crate::collaboration_profile_authority::profile_authority_path(
                    root.path(),
                    &principal.localhost_root,
                )
                .unwrap();
                let profile_bytes = std::fs::read(&profile_path).unwrap();
                passkey_response_for_grant(&restarted, current).unwrap();
                assert_eq!(std::fs::read(&profile_path).unwrap(), profile_bytes);
                assert_eq!(
                    std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap(),
                    stable_auth
                );
            }
        }
    }

    #[test]
    fn local_profile_setup_concurrent_retry_preserves_edited_profile() {
        let root = local_profile_fixture_root();
        let _auth = super::super::gateway::set_test_home_launch_auth_data_dir(root.path());
        let (credential, grant) = local_profile_owner_grant(root.path());
        let principal =
            crate::auth::load_principal_for_proof_binding(root.path(), &grant.proof_binding_id)
                .unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        let state = test_gateway_state(root.path());
                        barrier.wait();
                        initialize_local_profile(
                            &state,
                            &principal,
                            &grant.session_id,
                            "Public name",
                        )
                        .unwrap();
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let initial = crate::collaboration_profile_authority::load_profile_authority(
            root.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .unwrap();
        assert_eq!(initial.document().revision, 1);
        crate::collaboration_profile_authority::update_profile_authority(
            root.path(),
            &principal.principal_id,
            &principal.localhost_root,
            &principal.proof_binding_id,
            "Edited public name",
            None,
            crate::auth::now_ts(),
        )
        .unwrap();
        let path = crate::collaboration_profile_authority::profile_authority_path(
            root.path(),
            &principal.localhost_root,
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let current = local_profile_sign_in(root.path(), &credential);
        let auth = std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap();
        let state = test_gateway_state(root.path());
        passkey_response_for_grant(&state, current).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap(),
            auth
        );
        let edited = crate::collaboration_profile_authority::load_profile_authority(
            root.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            edited.document().profile_did,
            initial.document().profile_did
        );
        assert_eq!(edited.document().display_name, "Edited public name");
    }

    #[test]
    fn local_profile_guest_retry_and_unrelated_sign_in_keep_separate_consent() {
        let root = local_profile_fixture_root();
        let _auth = super::super::gateway::set_test_home_launch_auth_data_dir(root.path());
        let (_, owner) = local_profile_owner_grant(root.path());
        crate::auth::set_guest_registration_enabled(root.path(), true, crate::auth::now_ts())
            .unwrap();
        let guest_credential = test_credential_2();
        store_test_credential(root.path(), guest_credential.clone());
        let guest = crate::auth::grant_passkey_session(
            root.path(),
            crate::auth::PasskeySessionRequest {
                credential: &guest_credential,
                origin: "http://localhost:61180",
                user_verified: true,
                display_name: Some("Guest private label"),
                profile_display_name: Some("Guest public name"),
                reason: "guest registration fixture",
                purpose: crate::auth::PasskeySessionPurpose::GuestRegistration,
            },
        )
        .unwrap();
        let state = test_gateway_state(root.path());
        LOCAL_PROFILE_SETUP_FAULT
            .with(|slot| *slot.borrow_mut() = Some((root.path().into(), "after-root")));
        assert!(passkey_response_for_grant(&state, guest.clone()).is_err());
        let mut auth = crate::auth::load_auth_state(root.path()).unwrap();
        auth.sessions.clear();
        crate::auth::save_auth_state(root.path(), &auth).unwrap();
        let current = local_profile_sign_in(root.path(), &guest_credential);
        let response = passkey_response_for_grant(&state, current.clone()).unwrap();
        assert_eq!(response.principal_id, guest.principal_id);
        assert_ne!(response.principal_id, owner.principal_id);
        let principal =
            crate::auth::load_principal_for_proof_binding(root.path(), &guest.proof_binding_id)
                .unwrap();
        let profile = crate::collaboration_profile_authority::load_profile_authority(
            root.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .unwrap();
        assert_eq!(profile.document().display_name, "Guest public name");
        let mut forged = current.clone();
        forged.principal_id = owner.principal_id.clone();
        assert!(crate::auth::confirmed_initial_profile_name(root.path(), &forged).is_err());
        let before = std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap();
        assert!(crate::auth::grant_passkey_session(
            root.path(),
            crate::auth::PasskeySessionRequest {
                credential: &guest_credential,
                origin: "http://localhost:61180",
                user_verified: true,
                display_name: None,
                profile_display_name: Some("Changed consent"),
                reason: "invalid consent change",
                purpose: crate::auth::PasskeySessionPurpose::SignIn,
            }
        )
        .is_err());
        assert_eq!(
            std::fs::read(crate::auth::auth_state_path(root.path()).unwrap()).unwrap(),
            before
        );
        let owner_principal =
            crate::auth::load_principal_for_proof_binding(root.path(), &owner.proof_binding_id)
                .unwrap();
        assert!(
            crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &owner.principal_id,
                &owner_principal.localhost_root
            )
            .unwrap()
            .is_none()
        );
        let unrelated_credential = test_credential();
        store_test_credential(root.path(), unrelated_credential.clone());
        let unrelated = crate::auth::grant_passkey_session(
            root.path(),
            crate::auth::PasskeySessionRequest {
                credential: &unrelated_credential,
                origin: "http://localhost:61180",
                user_verified: true,
                display_name: Some("Unconfirmed private label"),
                profile_display_name: None,
                reason: "existing guest without public consent",
                purpose: crate::auth::PasskeySessionPurpose::GuestRegistration,
            },
        )
        .unwrap();
        let unrelated_response = passkey_response_for_grant(&state, unrelated.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(unrelated_response.profile_readiness).unwrap()["status"],
            "setup_required"
        );
        assert!(
            crate::auth::confirmed_initial_profile_name(root.path(), &unrelated)
                .unwrap()
                .is_none()
        );
        assert!(
            crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &owner.principal_id,
                &owner_principal.localhost_root
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            crate::collaboration_profile_authority::load_profile_authority(
                root.path(),
                &principal.principal_id,
                &principal.localhost_root
            )
            .unwrap()
            .unwrap()
            .document(),
            profile.document()
        );
    }

    #[test]
    fn registration_with_a_name_keeps_profile_setup_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let _auth_data_dir = super::super::gateway::set_test_home_launch_auth_data_dir(temp.path());
        let state = test_gateway_state(temp.path());
        let secret = crate::auth::random_secret_hex();
        let admission = crate::auth::OwnerAdmission {
            origin: "http://localhost:61180",
            rp_id: "localhost",
            claimant: &secret,
            loopback: true,
        };
        let mut identity = elastos_identity::IdentityManager::new(temp.path().into()).unwrap();
        let (ceremony, options) = crate::auth::begin_owner_enrollment(
            temp.path(),
            &mut identity,
            &admission,
            "named-owner",
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        let response = crate::auth::owner_attestation_for_test(
            &options.unwrap().public_key.challenge,
            admission.rp_id,
            admission.origin,
        );
        let grant = crate::auth::complete_owner_enrollment(
            temp.path(),
            &mut identity,
            &admission,
            &ceremony,
            Some(&response),
            Some("Anders"),
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        let credential = identity.credentials()[0].clone();
        let registered = passkey_response_for_grant(&state, grant).unwrap();
        let principal = crate::auth::load_principal_for_proof_binding(
            temp.path(),
            &registered.proof_binding_id,
        )
        .unwrap();
        assert_eq!(principal.display_name, "Anders");
        let legacy_record = serde_json::to_value(&principal).unwrap();
        assert!(legacy_record.get("initial_profile_display_name").is_none());
        assert!(
            serde_json::from_value::<crate::auth::PrincipalRecord>(legacy_record)
                .unwrap()
                .initial_profile_display_name
                .is_none()
        );
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
            "http://localhost:61180",
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
        seed_test_passkey_principal(
            &state,
            &test_credential_2(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Guest,
        );
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
        seed_test_passkey_principal(
            &state,
            &test_credential(),
            "https://elastos.elacitylabs.com",
            crate::auth::RuntimePrincipalRole::Admin,
        );
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
