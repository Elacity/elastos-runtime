use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use elastos_identity::{AuthenticationResponse, CredentialDescriptor, RequestOptions};
use elastos_runtime::auth::{PasskeyWebAuthnBinding, ProofBinding, RuntimeAuditEventV1};

use super::*;

const PASSKEY_STEP_UP_SCHEMA: &str = "elastos.auth.passkey-step-up/v1";
const PASSKEY_STEP_UP_DOMAIN: &str = "elastos.auth.passkey-step-up.v1";
const PASSKEY_STEP_UP_BEGIN_REQUEST_SCHEMA: &str = "elastos.auth.passkey-step-up.begin.request/v1";
const PASSKEY_STEP_UP_BEGIN_RESULT_SCHEMA: &str = "elastos.auth.passkey-step-up.begin.result/v1";
const PASSKEY_STEP_UP_COMPLETE_REQUEST_SCHEMA: &str =
    "elastos.auth.passkey-step-up.complete.request/v1";
const PASSKEY_STEP_UP_COMPLETE_RESULT_SCHEMA: &str =
    "elastos.auth.passkey-step-up.complete.result/v1";
const PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA: &str =
    "elastos.auth.passkey-step-up.cancel.request/v1";
const PASSKEY_STEP_UP_CANCEL_RESULT_SCHEMA: &str = "elastos.auth.passkey-step-up.cancel.result/v1";
const PASSKEY_STEP_UP_PENDING_SCHEMA: &str = "elastos.auth.passkey-step-up.pending/v1";
const PASSKEY_STEP_UP_CONSUMED_SCHEMA: &str = "elastos.auth.passkey-step-up.consumed/v1";
const PASSKEY_STEP_UP_TTL_SECS: u64 = 180;
const PASSKEY_STEP_UP_CEREMONY_TTL_SECS: u64 = 120;
const PASSKEY_STEP_UP_PENDING_CAPACITY: usize = 64;
const PASSKEY_STEP_UP_CONSUMED_CAPACITY: usize = 1024;
const PASSKEY_STEP_UP_MAX_OPERATION_BYTES: usize = 128;
const PASSKEY_STEP_UP_MAX_REQUEST_BYTES: usize = 64 * 1024;
const PASSKEY_STEP_UP_MAX_TOKEN_BYTES: usize = 32 * 1024;
const PASSKEY_STEP_UP_STAGING_PREFIX: &str = ".passkey-step-up-stage-";
const PASSKEY_STEP_UP_STAGING_SUFFIX: &str = ".tmp";
const PASSKEY_STEP_UP_ACTORS: &[&str] = &[
    HOME_CAPSULE_ID,
    INBOX_CAPSULE_ID,
    SYSTEM_CAPSULE_ID,
    WALLET_CAPSULE_ID,
];

static PASSKEY_STEP_UP_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasskeyStepUpBeginRequest {
    schema: String,
    app_token: String,
    operation: String,
    request: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub(super) struct PasskeyStepUpBeginResponse {
    schema: String,
    ceremony_id: String,
    options: RequestOptions,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasskeyStepUpCompleteRequest {
    schema: String,
    ceremony_id: String,
    response: AuthenticationResponse,
}

#[derive(Debug, Serialize)]
pub(super) struct PasskeyStepUpCompleteResponse {
    schema: String,
    step_up_token: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasskeyStepUpCancelRequest {
    schema: String,
    ceremony_id: String,
}

#[derive(Debug, Serialize)]
pub(super) struct PasskeyStepUpCancelResponse {
    schema: String,
    ceremony_id: String,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyStepUpPayload {
    schema: String,
    step_up_id: String,
    original_launch_id: String,
    launch_context: HomeLaunchContext,
    principal_id: String,
    session_id: String,
    proof_binding_id: String,
    grant_id: String,
    operation: String,
    request_sha256: String,
    iat: u64,
    exp: u64,
    non_delegatable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyStepUpEnvelope {
    payload: PasskeyStepUpPayload,
    signature: String,
    signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingPasskeyStepUp {
    schema: String,
    ceremony_id: String,
    original_launch_id: String,
    launch_context: HomeLaunchContext,
    principal_id: String,
    session_id: String,
    proof_binding_id: String,
    grant_id: String,
    credential_id: String,
    credential_public_key_sha256: String,
    rp_id: String,
    rp_origin: String,
    operation: String,
    request_sha256: String,
    created_at: u64,
    expires_at: u64,
    non_delegatable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumedPasskeyStepUp {
    schema: String,
    token_sha256: String,
    step_up_id: String,
    original_launch_id: String,
    principal_id: String,
    session_id: String,
    operation: String,
    request_sha256: String,
    expires_at: u64,
    consumed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::api) struct PasskeyStepUpEffectIdentity {
    pub(in crate::api) step_up_id: String,
    pub(in crate::api) request_sha256: String,
    pub(in crate::api) recovered: bool,
}

struct IssuedPasskeyStepUp {
    token: String,
    step_up_id: String,
}

pub(super) async fn passkey_step_up_begin(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<PasskeyStepUpBeginRequest>,
) -> Response {
    match passkey_step_up_begin_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => passkey_step_up_error_response(err),
    }
}

pub(super) async fn passkey_step_up_complete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<PasskeyStepUpCompleteRequest>,
) -> Response {
    match passkey_step_up_complete_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => passkey_step_up_error_response(err),
    }
}

pub(super) async fn passkey_step_up_cancel(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<PasskeyStepUpCancelRequest>,
) -> Response {
    match passkey_step_up_cancel_inner(&state, &headers, input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => passkey_step_up_error_response(err),
    }
}

async fn passkey_step_up_begin_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyStepUpBeginRequest,
) -> anyhow::Result<PasskeyStepUpBeginResponse> {
    if input.schema != PASSKEY_STEP_UP_BEGIN_REQUEST_SCHEMA {
        anyhow::bail!("unsupported passkey step-up begin request schema");
    }
    let host_context = require_home_token_context(&state.data_dir, headers)?;
    let launch = require_carried_home_launch_token(
        &state.data_dir,
        &input.app_token,
        PASSKEY_STEP_UP_ACTORS,
    )?;
    require_hosted_step_up_launch(&host_context, &launch)?;
    let operation = validate_step_up_operation(&input.operation)?;
    let request_sha256 = canonical_request_sha256(&input.request)?;
    let passkey = passkey_for_launch(&state.data_dir, &launch)?;
    let rp = super::super::handlers::identity::derive_rp(headers)?;
    if passkey.rp_id != rp.id {
        anyhow::bail!("passkey step-up relying party mismatch");
    }

    let now = now_ts();
    let ceremony_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
    let pending = PendingPasskeyStepUp {
        schema: PASSKEY_STEP_UP_PENDING_SCHEMA.to_string(),
        ceremony_id: ceremony_id.clone(),
        original_launch_id: launch.launch_id,
        launch_context: launch.launch_context,
        principal_id: launch.context.principal_id,
        session_id: launch.context.session_id,
        proof_binding_id: passkey_proof_binding_id(&passkey),
        grant_id: launch.context.grant_id,
        credential_id: passkey.credential_id.clone(),
        credential_public_key_sha256: hex::encode(Sha256::digest(passkey.public_key.as_bytes())),
        rp_id: rp.id.clone(),
        rp_origin: rp.origin,
        operation,
        request_sha256,
        created_at: now,
        expires_at: now.saturating_add(PASSKEY_STEP_UP_CEREMONY_TTL_SECS),
        non_delegatable: true,
    };
    pending.validate(now, false)?;

    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let _state_guard = step_up_state_guard()?;
    let expired = prepare_pending_capacity(&state.data_dir, now)?;
    for expired_id in expired {
        manager.cancel_challenge(&expired_id);
    }
    let matching_credentials = manager
        .credentials()
        .into_iter()
        .filter(|credential| {
            credential.credential_id == pending.credential_id
                && credential.rp_id == pending.rp_id
                && hex::encode(Sha256::digest(credential.public_key.as_bytes()))
                    == pending.credential_public_key_sha256
        })
        .count();
    if matching_credentials != 1 {
        anyhow::bail!("original passkey credential is unavailable");
    }
    let mut options = manager.begin_authentication(&ceremony_id, &rp.id)?;
    options
        .public_key
        .allow_credentials
        .retain(|credential| credential.id == pending.credential_id);
    if options.public_key.allow_credentials.len() != 1 {
        manager.cancel_challenge(&ceremony_id);
        anyhow::bail!("original passkey credential is unavailable");
    }
    options.public_key.allow_credentials = vec![CredentialDescriptor {
        type_: "public-key".to_string(),
        id: pending.credential_id.clone(),
    }];
    options.public_key.timeout = PASSKEY_STEP_UP_CEREMONY_TTL_SECS * 1000;
    if let Err(err) = persist_new_json(
        &pending_path(&state.data_dir, &ceremony_id)?,
        &pending,
        "passkey step-up state",
        |existing| existing.validate(now, false),
    ) {
        manager.cancel_challenge(&ceremony_id);
        return Err(err);
    }

    Ok(PasskeyStepUpBeginResponse {
        schema: PASSKEY_STEP_UP_BEGIN_RESULT_SCHEMA.to_string(),
        ceremony_id,
        options,
        expires_at: pending.expires_at,
    })
}

async fn passkey_step_up_complete_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyStepUpCompleteRequest,
) -> anyhow::Result<PasskeyStepUpCompleteResponse> {
    if input.schema != PASSKEY_STEP_UP_COMPLETE_REQUEST_SCHEMA {
        anyhow::bail!("unsupported passkey step-up complete request schema");
    }
    validate_ceremony_id(&input.ceremony_id)?;
    let host_context = require_home_token_context(&state.data_dir, headers)?;
    let rp = super::super::handlers::identity::derive_rp(headers)?;
    let now = now_ts();
    let pending = claim_pending(
        &state.data_dir,
        &input.ceremony_id,
        &host_context,
        &rp.id,
        &rp.origin,
        Some(&input.response.raw_id),
        now,
    )?;

    let manager = state.identity_manager()?;
    let mut manager = manager.lock().await;
    let outcome = manager.complete_authentication(
        &pending.ceremony_id,
        &input.response,
        &pending.rp_id,
        &pending.rp_origin,
    )?;
    if !outcome.user_verified
        || outcome.credential.credential_id != pending.credential_id
        || outcome.credential.rp_id != pending.rp_id
        || hex::encode(Sha256::digest(outcome.credential.public_key.as_bytes()))
            != pending.credential_public_key_sha256
        || passkey_proof_binding_id(&PasskeyWebAuthnBinding {
            credential_id: outcome.credential.credential_id.clone(),
            public_key: outcome.credential.public_key.clone(),
            sign_count: outcome.credential.sign_count,
            user_verified: true,
            origin: outcome.origin.clone(),
            rp_id: outcome.credential.rp_id.clone(),
            created_at: 0,
            last_used_at: 0,
            revoked_at: None,
        }) != pending.proof_binding_id
    {
        anyhow::bail!("passkey step-up used a different registered passkey");
    }
    drop(manager);

    complete_step_up(&state.data_dir, &pending, now)
}

async fn passkey_step_up_cancel_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    input: PasskeyStepUpCancelRequest,
) -> anyhow::Result<PasskeyStepUpCancelResponse> {
    if input.schema != PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA {
        anyhow::bail!("unsupported passkey step-up cancel request schema");
    }
    validate_ceremony_id(&input.ceremony_id)?;
    let host_context = require_home_token_context(&state.data_dir, headers)?;
    let rp = super::super::handlers::identity::derive_rp(headers)?;
    let pending = claim_pending(
        &state.data_dir,
        &input.ceremony_id,
        &host_context,
        &rp.id,
        &rp.origin,
        None,
        now_ts(),
    )?;
    let manager = state.identity_manager()?;
    manager.lock().await.cancel_challenge(&pending.ceremony_id);
    Ok(PasskeyStepUpCancelResponse {
        schema: PASSKEY_STEP_UP_CANCEL_RESULT_SCHEMA.to_string(),
        ceremony_id: pending.ceremony_id,
        status: "cancelled".to_string(),
    })
}

pub(in crate::api) fn consume_passkey_step_up_token(
    data_dir: &Path,
    token: &str,
    launch: &RequiredHomeLaunchToken,
    max_age_secs: u64,
    operation: &str,
    request: &serde_json::Value,
) -> anyhow::Result<()> {
    consume_passkey_step_up_for_effect(
        data_dir,
        token,
        launch,
        max_age_secs,
        operation,
        request,
        false,
    )
    .map(|_| ())
}

pub(in crate::api) fn consume_or_recover_passkey_step_up_effect(
    data_dir: &Path,
    token: &str,
    launch: &RequiredHomeLaunchToken,
    max_age_secs: u64,
    operation: &str,
    request: &serde_json::Value,
) -> anyhow::Result<PasskeyStepUpEffectIdentity> {
    consume_passkey_step_up_for_effect(
        data_dir,
        token,
        launch,
        max_age_secs,
        operation,
        request,
        true,
    )
}

fn consume_passkey_step_up_for_effect(
    data_dir: &Path,
    token: &str,
    launch: &RequiredHomeLaunchToken,
    max_age_secs: u64,
    operation: &str,
    request: &serde_json::Value,
    allow_exact_recovery: bool,
) -> anyhow::Result<PasskeyStepUpEffectIdentity> {
    let operation = validate_step_up_operation(operation)?;
    let request_sha256 = canonical_request_sha256(request)?;
    let token = token.trim();
    if token.is_empty() {
        anyhow::bail!("missing passkey step-up token");
    }
    if token.len() > PASSKEY_STEP_UP_MAX_TOKEN_BYTES {
        anyhow::bail!("invalid passkey step-up token encoding");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| anyhow::anyhow!("invalid passkey step-up token encoding"))?;
    require_step_up_schema(&bytes)?;
    let envelope: PasskeyStepUpEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid passkey step-up token payload"))?;
    let local_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        PASSKEY_STEP_UP_DOMAIN,
        &[local_did],
    )
    .map_err(|err| anyhow::anyhow!("invalid passkey step-up token: {err}"))?;

    let now = now_ts();
    envelope.payload.validate(now, max_age_secs)?;
    require_step_up_binding(&envelope.payload, launch, &operation, &request_sha256)?;
    let auth_data_dir = home_launch_auth_data_dir(data_dir);
    let grant =
        crate::auth::load_active_session_grant(&auth_data_dir, &envelope.payload.session_id, now)
            .map_err(|_| anyhow::anyhow!("passkey step-up auth session is not active"))?;
    if grant.principal_id != envelope.payload.principal_id
        || grant.proof_binding_id != envelope.payload.proof_binding_id
        || grant.grant_id != envelope.payload.grant_id
    {
        anyhow::bail!("passkey step-up authority context mismatch");
    }
    let principal = crate::auth::load_principal_for_proof_binding(
        &auth_data_dir,
        &envelope.payload.proof_binding_id,
    )?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if principal.proof_binding.passkey.is_none() {
        anyhow::bail!("passkey step-up proof binding is not a passkey");
    }

    let marker = ConsumedPasskeyStepUp {
        schema: PASSKEY_STEP_UP_CONSUMED_SCHEMA.to_string(),
        token_sha256: hex::encode(Sha256::digest(token.as_bytes())),
        step_up_id: envelope.payload.step_up_id,
        original_launch_id: envelope.payload.original_launch_id,
        principal_id: envelope.payload.principal_id,
        session_id: envelope.payload.session_id,
        operation,
        request_sha256,
        expires_at: envelope.payload.exp,
        consumed_at: now,
    };
    marker.validate()?;
    let _state_guard = step_up_state_guard()?;
    let path = consumed_path(data_dir, &marker.step_up_id)?;
    if path.is_file() {
        let existing: ConsumedPasskeyStepUp =
            read_strict_json(&path, "passkey step-up consumed state")?;
        existing.validate()?;
        if allow_exact_recovery
            && existing.token_sha256 == marker.token_sha256
            && existing.step_up_id == marker.step_up_id
            && existing.original_launch_id == marker.original_launch_id
            && existing.principal_id == marker.principal_id
            && existing.session_id == marker.session_id
            && existing.operation == marker.operation
            && existing.request_sha256 == marker.request_sha256
            && existing.expires_at == marker.expires_at
        {
            return Ok(PasskeyStepUpEffectIdentity {
                step_up_id: existing.step_up_id,
                request_sha256: existing.request_sha256,
                recovered: true,
            });
        }
        anyhow::bail!("passkey step-up token has already been used");
    }
    prepare_consumed_capacity(data_dir, now)?;
    persist_new_json(
        &path,
        &marker,
        "passkey step-up consumed state",
        ConsumedPasskeyStepUp::validate,
    )
    .map_err(|err| {
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|err| err.kind() == std::io::ErrorKind::AlreadyExists)
        {
            anyhow::anyhow!("passkey step-up token has already been used")
        } else {
            err
        }
    })?;
    Ok(PasskeyStepUpEffectIdentity {
        step_up_id: marker.step_up_id,
        request_sha256: marker.request_sha256,
        recovered: false,
    })
}

#[cfg(test)]
pub(crate) fn issue_passkey_step_up_token_for_test(
    data_dir: &Path,
    app_token: &str,
    expected_app: &str,
    operation: &str,
    request: &serde_json::Value,
) -> anyhow::Result<String> {
    issue_passkey_step_up_token_at_for_test(
        data_dir,
        app_token,
        expected_app,
        operation,
        request,
        now_ts(),
    )
}

#[cfg(test)]
pub(crate) fn issue_passkey_step_up_token_at_for_test(
    data_dir: &Path,
    app_token: &str,
    expected_app: &str,
    operation: &str,
    request: &serde_json::Value,
    issued_at: u64,
) -> anyhow::Result<String> {
    let launch = require_carried_home_launch_token(data_dir, app_token, &[expected_app])?;
    let passkey = passkey_for_launch(data_dir, &launch)?;
    issue_step_up_token(
        data_dir,
        &PendingPasskeyStepUp {
            schema: PASSKEY_STEP_UP_PENDING_SCHEMA.to_string(),
            ceremony_id: format!("passkey:step-up:{}", gateway_home_token::uuid_like_token()),
            original_launch_id: launch.launch_id,
            launch_context: launch.launch_context,
            principal_id: launch.context.principal_id,
            session_id: launch.context.session_id,
            proof_binding_id: passkey_proof_binding_id(&passkey),
            grant_id: launch.context.grant_id,
            credential_id: passkey.credential_id,
            credential_public_key_sha256: hex::encode(Sha256::digest(
                passkey.public_key.as_bytes(),
            )),
            rp_id: passkey.rp_id,
            rp_origin: passkey.origin,
            operation: validate_step_up_operation(operation)?,
            request_sha256: canonical_request_sha256(request)?,
            created_at: issued_at,
            expires_at: issued_at.saturating_add(PASSKEY_STEP_UP_CEREMONY_TTL_SECS),
            non_delegatable: true,
        },
        issued_at,
    )
    .map(|issued| issued.token)
}

fn require_hosted_step_up_launch(
    host_context: &HomeLaunchTokenContext,
    launch: &RequiredHomeLaunchToken,
) -> anyhow::Result<()> {
    if launch.launch_context.authority_actor != HOME_CAPSULE_ID
        || launch.launch_context.selected_resource != launch.launch_context.executable_actor
    {
        anyhow::bail!("passkey step-up requires an original Home-hosted app launch");
    }
    let Some(proof_binding_id) = launch.context.proof_binding_id.as_deref() else {
        anyhow::bail!("passkey step-up requires a proof-bound original launch");
    };
    if !proof_binding_id.starts_with("proof:passkey:")
        || host_context.principal_id != launch.context.principal_id
        || host_context.session_id != launch.context.session_id
        || host_context.proof_binding_id.as_deref() != Some(proof_binding_id)
        || host_context.grant_id != launch.context.grant_id
    {
        anyhow::bail!("passkey step-up original launch authority mismatch");
    }
    Ok(())
}

fn passkey_for_launch(
    data_dir: &Path,
    launch: &RequiredHomeLaunchToken,
) -> anyhow::Result<PasskeyWebAuthnBinding> {
    let proof_binding_id =
        launch.context.proof_binding_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("passkey step-up requires a proof-bound original launch")
        })?;
    if !proof_binding_id.starts_with("proof:passkey:") {
        anyhow::bail!("passkey step-up requires the original registered passkey");
    }
    let auth_data_dir = home_launch_auth_data_dir(data_dir);
    let principal =
        crate::auth::load_principal_for_proof_binding(&auth_data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if principal.principal_id != launch.context.principal_id {
        anyhow::bail!("passkey step-up principal binding mismatch");
    }
    principal
        .proof_binding
        .passkey
        .ok_or_else(|| anyhow::anyhow!("original proof binding is not a registered passkey"))
}

fn issue_step_up_token(
    data_dir: &Path,
    pending: &PendingPasskeyStepUp,
    issued_at: u64,
) -> anyhow::Result<IssuedPasskeyStepUp> {
    let step_up_id = format!("step-up:{}", gateway_home_token::uuid_like_token());
    let payload = PasskeyStepUpPayload {
        schema: PASSKEY_STEP_UP_SCHEMA.to_string(),
        step_up_id: step_up_id.clone(),
        original_launch_id: pending.original_launch_id.clone(),
        launch_context: pending.launch_context.clone(),
        principal_id: pending.principal_id.clone(),
        session_id: pending.session_id.clone(),
        proof_binding_id: pending.proof_binding_id.clone(),
        grant_id: pending.grant_id.clone(),
        operation: pending.operation.clone(),
        request_sha256: pending.request_sha256.clone(),
        iat: issued_at,
        exp: issued_at.saturating_add(PASSKEY_STEP_UP_TTL_SECS),
        non_delegatable: true,
    };
    payload.validate(issued_at, PASSKEY_STEP_UP_TTL_SECS)?;
    let (signing_key, _did) = elastos_identity::load_or_create_did(data_dir)?;
    let canonical = serde_json::to_string(&serde_json::to_value(&payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        PASSKEY_STEP_UP_DOMAIN,
        canonical.as_bytes(),
    );
    let envelope = PasskeyStepUpEnvelope {
        payload,
        signature,
        signer_did,
    };
    Ok(IssuedPasskeyStepUp {
        token: URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope)?),
        step_up_id,
    })
}

fn complete_step_up(
    data_dir: &Path,
    pending: &PendingPasskeyStepUp,
    completed_at: u64,
) -> anyhow::Result<PasskeyStepUpCompleteResponse> {
    let issued = issue_step_up_token(data_dir, pending, completed_at)?;
    append_step_up_completion_audit(data_dir, pending, &issued.step_up_id, completed_at)?;
    Ok(PasskeyStepUpCompleteResponse {
        schema: PASSKEY_STEP_UP_COMPLETE_RESULT_SCHEMA.to_string(),
        step_up_token: issued.token,
        expires_at: completed_at.saturating_add(PASSKEY_STEP_UP_TTL_SECS),
    })
}

fn append_step_up_completion_audit(
    data_dir: &Path,
    pending: &PendingPasskeyStepUp,
    step_up_id: &str,
    completed_at: u64,
) -> anyhow::Result<()> {
    let reason = serde_json::to_string(&serde_json::json!({
        "original_launch_id": pending.original_launch_id,
        "selected_resource": pending.launch_context.selected_resource,
        "executable_actor": pending.launch_context.executable_actor,
        "authority_actor": pending.launch_context.authority_actor,
        "operation": pending.operation,
        "request_sha256": pending.request_sha256,
        "ceremony_id": pending.ceremony_id,
        "step_up_id": step_up_id,
    }))?;
    crate::auth::append_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!("audit:{}", gateway_home_token::uuid_like_token()),
            event_type: "auth.passkey_step_up.completed".to_string(),
            principal_id: Some(pending.principal_id.clone()),
            proof_binding_id: Some(pending.proof_binding_id.clone()),
            session_id: Some(pending.session_id.clone()),
            challenge_id: Some(pending.ceremony_id.clone()),
            capsule_id: Some(pending.launch_context.executable_actor.clone()),
            result: "success".to_string(),
            reason,
            occurred_at: completed_at,
            signer_did: None,
            signature: None,
        },
    )
    .map_err(|err| anyhow::anyhow!("failed to persist passkey step-up completion audit: {err}"))
}

fn require_step_up_binding(
    payload: &PasskeyStepUpPayload,
    launch: &RequiredHomeLaunchToken,
    operation: &str,
    request_sha256: &str,
) -> anyhow::Result<()> {
    let proof_binding_id = launch
        .context
        .proof_binding_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("original launch is not passkey proof-bound"))?;
    if payload.original_launch_id != launch.launch_id
        || payload.launch_context != launch.launch_context
        || payload.principal_id != launch.context.principal_id
        || payload.session_id != launch.context.session_id
        || payload.proof_binding_id != proof_binding_id
        || payload.grant_id != launch.context.grant_id
    {
        anyhow::bail!("passkey step-up original launch binding mismatch");
    }
    if payload.operation != operation || payload.request_sha256 != request_sha256 {
        anyhow::bail!("passkey step-up intent mismatch");
    }
    Ok(())
}

impl PasskeyStepUpPayload {
    fn validate(&self, now: u64, max_age_secs: u64) -> anyhow::Result<()> {
        if self.schema != PASSKEY_STEP_UP_SCHEMA {
            anyhow::bail!("unsupported passkey step-up token schema");
        }
        validate_step_up_id(&self.step_up_id)?;
        validate_launch_id(&self.original_launch_id)?;
        self.launch_context.validate()?;
        validate_authority_value(&self.principal_id, "principal")?;
        validate_authority_value(&self.session_id, "session")?;
        validate_authority_value(&self.proof_binding_id, "proof binding")?;
        validate_authority_value(&self.grant_id, "grant")?;
        validate_step_up_operation(&self.operation)?;
        validate_sha256(&self.request_sha256)?;
        if !self.proof_binding_id.starts_with("proof:passkey:") {
            anyhow::bail!("passkey step-up is not bound to a registered passkey");
        }
        if !self.non_delegatable
            || self.exp <= self.iat
            || self.exp.saturating_sub(self.iat) > PASSKEY_STEP_UP_TTL_SECS
        {
            anyhow::bail!("passkey step-up token lifetime is invalid");
        }
        if self.exp <= now {
            anyhow::bail!("passkey step-up token expired");
        }
        if self.iat > now.saturating_add(60)
            || now.saturating_sub(self.iat) > max_age_secs.min(PASSKEY_STEP_UP_TTL_SECS)
        {
            anyhow::bail!("passkey step-up token is too old");
        }
        Ok(())
    }
}

impl PendingPasskeyStepUp {
    fn validate(&self, now: u64, require_active: bool) -> anyhow::Result<()> {
        if self.schema != PASSKEY_STEP_UP_PENDING_SCHEMA {
            anyhow::bail!("malformed passkey step-up state");
        }
        validate_ceremony_id(&self.ceremony_id)?;
        validate_launch_id(&self.original_launch_id)?;
        self.launch_context.validate()?;
        validate_authority_value(&self.principal_id, "principal")?;
        validate_authority_value(&self.session_id, "session")?;
        validate_authority_value(&self.proof_binding_id, "proof binding")?;
        validate_authority_value(&self.grant_id, "grant")?;
        validate_authority_value(&self.credential_id, "credential")?;
        validate_sha256(&self.credential_public_key_sha256)?;
        validate_authority_value(&self.rp_id, "relying party")?;
        if self.rp_origin.len() > 512 || url::Url::parse(&self.rp_origin).is_err() {
            anyhow::bail!("malformed passkey step-up state");
        }
        validate_step_up_operation(&self.operation)?;
        validate_sha256(&self.request_sha256)?;
        if !self.non_delegatable
            || self.expires_at <= self.created_at
            || self.expires_at.saturating_sub(self.created_at) > PASSKEY_STEP_UP_CEREMONY_TTL_SECS
        {
            anyhow::bail!("malformed passkey step-up state");
        }
        if require_active && self.expires_at <= now {
            anyhow::bail!("passkey step-up ceremony expired");
        }
        Ok(())
    }
}

fn claim_pending(
    data_dir: &Path,
    ceremony_id: &str,
    host_context: &HomeLaunchTokenContext,
    rp_id: &str,
    rp_origin: &str,
    response_credential_id: Option<&str>,
    now: u64,
) -> anyhow::Result<PendingPasskeyStepUp> {
    let _state_guard = step_up_state_guard()?;
    let path = pending_path(data_dir, ceremony_id)?;
    let root = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("passkey step-up state root is unavailable"))?;
    ensure_private_record_root(root)?;
    let pending: PendingPasskeyStepUp = read_strict_json(&path, "passkey step-up state")?;
    pending.validate(now, true)?;
    if pending.principal_id != host_context.principal_id
        || pending.session_id != host_context.session_id
        || host_context.proof_binding_id.as_deref() != Some(pending.proof_binding_id.as_str())
        || pending.grant_id != host_context.grant_id
        || pending.rp_id != rp_id
        || pending.rp_origin.trim_end_matches('/') != rp_origin.trim_end_matches('/')
    {
        anyhow::bail!("passkey step-up ceremony authority mismatch");
    }
    if response_credential_id.is_some_and(|credential| credential != pending.credential_id) {
        anyhow::bail!("passkey step-up used a different registered passkey");
    }
    std::fs::remove_file(&path)?;
    sync_directory(root)?;
    Ok(pending)
}

fn prepare_pending_capacity(data_dir: &Path, now: u64) -> anyhow::Result<Vec<String>> {
    let root = pending_root(data_dir)?;
    let mut active = 0usize;
    let mut expired = Vec::new();
    let mut removed = false;
    for path in committed_record_paths(&root, "passkey step-up state")? {
        let pending: PendingPasskeyStepUp = read_strict_json(&path, "passkey step-up state")?;
        pending.validate(now, false)?;
        if pending.expires_at <= now {
            std::fs::remove_file(path)?;
            expired.push(pending.ceremony_id);
            removed = true;
        } else {
            active = active.saturating_add(1);
        }
    }
    if removed {
        sync_directory(&root)?;
    }
    if active >= PASSKEY_STEP_UP_PENDING_CAPACITY {
        anyhow::bail!("passkey step-up ceremony capacity exceeded");
    }
    Ok(expired)
}

fn prepare_consumed_capacity(data_dir: &Path, now: u64) -> anyhow::Result<()> {
    let root = consumed_root(data_dir)?;
    let mut active = 0usize;
    let mut removed = false;
    for path in committed_record_paths(&root, "passkey step-up consumed state")? {
        let marker: ConsumedPasskeyStepUp =
            read_strict_json(&path, "passkey step-up consumed state")?;
        marker.validate()?;
        if marker.expires_at <= now {
            std::fs::remove_file(path)?;
            removed = true;
        } else {
            active = active.saturating_add(1);
        }
    }
    if removed {
        sync_directory(&root)?;
    }
    if active >= PASSKEY_STEP_UP_CONSUMED_CAPACITY {
        anyhow::bail!("passkey step-up consumption capacity exceeded");
    }
    Ok(())
}

impl ConsumedPasskeyStepUp {
    fn validate(&self) -> anyhow::Result<()> {
        if self.schema != PASSKEY_STEP_UP_CONSUMED_SCHEMA {
            anyhow::bail!("malformed passkey step-up consumed state");
        }
        validate_sha256(&self.token_sha256)?;
        validate_step_up_id(&self.step_up_id)?;
        validate_launch_id(&self.original_launch_id)?;
        validate_authority_value(&self.principal_id, "principal")?;
        validate_authority_value(&self.session_id, "session")?;
        validate_step_up_operation(&self.operation)?;
        validate_sha256(&self.request_sha256)?;
        if self.expires_at <= self.consumed_at {
            anyhow::bail!("malformed passkey step-up consumed state");
        }
        Ok(())
    }
}

fn canonical_request_sha256(request: &serde_json::Value) -> anyhow::Result<String> {
    if !request.is_object() {
        anyhow::bail!("passkey step-up request must be a JSON object");
    }
    let canonical = canonical_json(request);
    let bytes = serde_json::to_vec(&canonical)?;
    if bytes.len() > PASSKEY_STEP_UP_MAX_REQUEST_BYTES {
        anyhow::bail!("passkey step-up request is too large");
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        other => other.clone(),
    }
}

fn validate_step_up_operation(value: &str) -> anyhow::Result<String> {
    if value.is_empty()
        || value.len() > PASSKEY_STEP_UP_MAX_OPERATION_BYTES
        || value != value.trim()
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':')))
    {
        anyhow::bail!("invalid passkey step-up operation");
    }
    Ok(value.to_string())
}

fn validate_authority_value(value: &str, label: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 512
        || value != value.trim()
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid passkey step-up {label}");
    }
    Ok(())
}

fn validate_ceremony_id(value: &str) -> anyhow::Result<()> {
    validate_prefixed_hex_id(value, "passkey:step-up:", "ceremony")
}

fn validate_step_up_id(value: &str) -> anyhow::Result<()> {
    validate_prefixed_hex_id(value, "step-up:", "token")
}

fn validate_launch_id(value: &str) -> anyhow::Result<()> {
    validate_prefixed_hex_id(value, "launch:", "launch")
}

fn validate_prefixed_hex_id(value: &str, prefix: &str, label: &str) -> anyhow::Result<()> {
    if !value
        .strip_prefix(prefix)
        .is_some_and(|suffix| suffix.len() == 32 && suffix.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        anyhow::bail!("invalid passkey step-up {label} id");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> anyhow::Result<()> {
    if value.len() != 64
        || !value
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
    {
        anyhow::bail!("invalid passkey step-up SHA-256");
    }
    Ok(())
}

fn passkey_proof_binding_id(passkey: &PasskeyWebAuthnBinding) -> String {
    ProofBinding::passkey_webauthn(passkey.clone()).id()
}

fn require_step_up_schema(bytes: &[u8]) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid passkey step-up token payload"))?;
    if value
        .get("payload")
        .and_then(|payload| payload.get("schema"))
        .and_then(serde_json::Value::as_str)
        != Some(PASSKEY_STEP_UP_SCHEMA)
    {
        anyhow::bail!("unsupported passkey step-up token schema");
    }
    Ok(())
}

fn step_up_state_guard() -> anyhow::Result<std::sync::MutexGuard<'static, ()>> {
    PASSKEY_STEP_UP_STATE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("passkey step-up state lock is poisoned"))
}

fn state_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let auth_data_dir = home_launch_auth_data_dir(data_dir);
    let auth_state = crate::auth::auth_state_path(&auth_data_dir)?;
    Ok(auth_state
        .parent()
        .ok_or_else(|| anyhow::anyhow!("authentication state root is unavailable"))?
        .join("passkey-step-up"))
}

fn pending_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    Ok(state_root(data_dir)?.join("pending"))
}

fn consumed_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    Ok(state_root(data_dir)?.join("consumed"))
}

fn pending_path(data_dir: &Path, ceremony_id: &str) -> anyhow::Result<PathBuf> {
    validate_ceremony_id(ceremony_id)?;
    Ok(pending_root(data_dir)?.join(format!(
        "{}.json",
        hex::encode(Sha256::digest(ceremony_id.as_bytes()))
    )))
}

fn consumed_path(data_dir: &Path, step_up_id: &str) -> anyhow::Result<PathBuf> {
    validate_step_up_id(step_up_id)?;
    Ok(consumed_root(data_dir)?.join(format!(
        "{}.json",
        hex::encode(Sha256::digest(step_up_id.as_bytes()))
    )))
}

fn read_strict_json<T: serde::de::DeserializeOwned>(path: &Path, label: &str) -> anyhow::Result<T> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| anyhow::anyhow!("{label} is unavailable"))?;
    if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 {
        anyhow::bail!("malformed {label}");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|_| anyhow::anyhow!("malformed {label}"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        anyhow::bail!("malformed {label}");
    }
    set_private_open_file_permissions(&file)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("malformed {label}"))
}

fn persist_new_json<T>(
    path: &Path,
    value: &T,
    label: &str,
    validate_existing: impl FnOnce(&T) -> anyhow::Result<()>,
) -> anyhow::Result<()>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("passkey step-up state root is unavailable"))?;
    ensure_private_record_root(parent)?;
    let record_digest = committed_record_digest(
        path.file_name()
            .ok_or_else(|| anyhow::anyhow!("passkey step-up state path has no file name"))?,
    )
    .ok_or_else(|| anyhow::anyhow!("invalid passkey step-up state path"))?;
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() > 64 * 1024 {
        anyhow::bail!("{label} is too large");
    }
    let (staging_path, mut staging_file) = create_staging_file(parent, record_digest)?;
    let staged = (|| -> anyhow::Result<()> {
        staging_file.write_all(&bytes)?;
        staging_file.sync_all()?;
        Ok(())
    })();
    drop(staging_file);
    if let Err(err) = staged {
        remove_staging_and_sync(&staging_path, parent)?;
        return Err(err);
    }

    match std::fs::hard_link(&staging_path, path) {
        Ok(()) => {
            set_private_file_permissions(path)?;
            remove_staging_and_sync(&staging_path, parent)?;
            Ok(())
        }
        Err(publish_err) => {
            remove_staging_and_sync(&staging_path, parent)?;
            if publish_err.kind() == ErrorKind::AlreadyExists {
                let existing: T = read_strict_json(path, label)?;
                validate_existing(&existing)?;
            }
            Err(publish_err.into())
        }
    }
}

fn create_staging_file(parent: &Path, record_digest: &str) -> anyhow::Result<(PathBuf, File)> {
    for _ in 0..8 {
        let nonce = gateway_home_token::uuid_like_token();
        let path = parent.join(format!(
            "{PASSKEY_STEP_UP_STAGING_PREFIX}{record_digest}-{nonce}{PASSKEY_STEP_UP_STAGING_SUFFIX}"
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        match options.open(&path) {
            Ok(file) => {
                if !file.metadata()?.is_file() {
                    anyhow::bail!("passkey step-up staging entry is not a regular file");
                }
                set_private_file_permissions(&path)?;
                return Ok((path, file));
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    anyhow::bail!("passkey step-up staging capacity exceeded")
}

fn remove_staging_and_sync(path: &Path, parent: &Path) -> anyhow::Result<()> {
    std::fs::remove_file(path)?;
    sync_directory(parent)
}

fn committed_record_paths(root: &Path, label: &str) -> anyhow::Result<Vec<PathBuf>> {
    ensure_private_record_root(root)?;
    let mut committed = Vec::new();
    let mut removed_staging = false;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("malformed {label}"))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if is_private_staging_name(&name) {
            if metadata.file_type().is_symlink() || metadata.file_type().is_file() {
                std::fs::remove_file(path)?;
                removed_staging = true;
                continue;
            }
            anyhow::bail!("malformed {label}");
        }
        if committed_record_digest(OsStr::new(&name)).is_none()
            || metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
        {
            anyhow::bail!("malformed {label}");
        }
        committed.push(path);
    }
    if removed_staging {
        sync_directory(root)?;
    }
    Ok(committed)
}

fn committed_record_digest(name: &OsStr) -> Option<&str> {
    let name = name.to_str()?;
    let digest = name.strip_suffix(".json")?;
    is_lower_hex(digest, 64).then_some(digest)
}

fn is_private_staging_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix(PASSKEY_STEP_UP_STAGING_PREFIX)
        .and_then(|value| value.strip_suffix(PASSKEY_STEP_UP_STAGING_SUFFIX))
    else {
        return false;
    };
    let Some((record_digest, nonce)) = body.split_once('-') else {
        return false;
    };
    is_lower_hex(record_digest, 64) && is_lower_hex(nonce, 32)
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn ensure_private_record_root(root: &Path) -> anyhow::Result<()> {
    let state = root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("passkey step-up state root is unavailable"))?;
    ensure_private_directory(state)?;
    ensure_private_directory(root)
}

fn ensure_private_directory(path: &Path) -> anyhow::Result<()> {
    let (metadata, created) = match std::fs::symlink_metadata(path) {
        Ok(metadata) => (metadata, false),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => (std::fs::symlink_metadata(path)?, true),
                Err(create_err) if create_err.kind() == ErrorKind::AlreadyExists => {
                    (std::fs::symlink_metadata(path)?, false)
                }
                Err(create_err) => return Err(create_err.into()),
            }
        }
        Err(err) => return Err(err.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        anyhow::bail!("passkey step-up state path must use regular non-symlink directories");
    }
    set_private_directory_permissions(path)?;
    sync_directory(path)?;
    if created {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("passkey step-up state root is unavailable"))?;
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_open_file_permissions(file: &File) -> anyhow::Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_open_file_permissions(_file: &File) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> anyhow::Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn passkey_step_up_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("missing home launch token")
        || text.contains("original launch authority")
        || text.contains("Home-hosted")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("capacity") {
        StatusCode::TOO_MANY_REQUESTS
    } else if text.contains("unavailable") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, text).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_identity::{AuthenticatorAssertionResponse, IdentityStore, StoredCredential};
    use elastos_runtime::auth::{AuthSessionGrantV1, ProofBinding};

    struct StepUpFixture {
        data_dir: tempfile::TempDir,
        state: GatewayState,
        host_headers: HeaderMap,
        app_token: String,
        launch: RequiredHomeLaunchToken,
        context: HomeLaunchTokenContext,
        credential_id: String,
    }

    fn fixture() -> StepUpFixture {
        let data_dir = tempfile::tempdir().unwrap();
        let now = now_ts();
        let credential_id = "step-up-test-credential".to_string();
        let public_key = "step-up-test-public-key".to_string();
        let rp_id = "localhost".to_string();
        let rp_origin = "http://localhost:61180".to_string();

        let mut identity_store = IdentityStore::new(data_dir.path()).unwrap();
        identity_store.load().unwrap();
        identity_store.add_credential(StoredCredential {
            credential_id: credential_id.clone(),
            public_key: public_key.clone(),
            sign_count: 1,
            rp_id: rp_id.clone(),
        });
        identity_store.save().unwrap();

        let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
            credential_id: credential_id.clone(),
            public_key,
            sign_count: 1,
            user_verified: true,
            origin: rp_origin,
            rp_id,
            created_at: now,
            last_used_at: now,
            revoked_at: None,
        });
        let principal_id =
            crate::auth::passkey_credential_principal_id("localhost", &credential_id).unwrap();
        let principal = crate::auth::upsert_principal_for_binding_as_role_named(
            data_dir.path(),
            binding,
            principal_id,
            crate::auth::RuntimePrincipalRole::Admin,
            Some("Step-up tester"),
            now,
        )
        .unwrap();
        let grant = AuthSessionGrantV1 {
            schema: AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: format!("grant:{}", gateway_home_token::uuid_like_token()),
            session_id: format!("auth:{}", gateway_home_token::uuid_like_token()),
            principal_id: principal.principal_id,
            proof_binding_id: principal.proof_binding_id,
            issued_at: now,
            expires_at: now.saturating_add(HOME_LAUNCH_TOKEN_TTL_SECS),
            apps: vec![
                HOME_CAPSULE_ID.to_string(),
                INBOX_CAPSULE_ID.to_string(),
                SYSTEM_CAPSULE_ID.to_string(),
                WALLET_CAPSULE_ID.to_string(),
            ],
        };
        crate::auth::store_session_grant(data_dir.path(), grant.clone()).unwrap();
        let context = HomeLaunchTokenContext {
            principal_id: grant.principal_id.clone(),
            session_id: grant.session_id.clone(),
            proof_binding_id: Some(grant.proof_binding_id.clone()),
            grant_id: grant.grant_id.clone(),
        };
        let home_token =
            issue_home_launch_token_for_auth_grant(data_dir.path(), HOME_CAPSULE_ID, &grant)
                .unwrap();
        let app_token = issue_home_projection_launch_token_with_context(
            data_dir.path(),
            INBOX_CAPSULE_ID,
            INBOX_CAPSULE_ID,
            &context,
        )
        .unwrap();
        let launch =
            require_carried_home_launch_token(data_dir.path(), &app_token, &[INBOX_CAPSULE_ID])
                .unwrap();
        let mut host_headers = HeaderMap::new();
        host_headers.insert("x-elastos-home-token", home_token.parse().unwrap());
        host_headers.insert("host", "localhost:61180".parse().unwrap());
        host_headers.insert("origin", "http://localhost:61180".parse().unwrap());
        host_headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        let state = GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(OnceLock::new()),
            cache_dir: data_dir.path().join("cache"),
            data_dir: data_dir.path().to_path_buf(),
        };
        StepUpFixture {
            data_dir,
            state,
            host_headers,
            app_token,
            launch,
            context,
            credential_id,
        }
    }

    fn begin_request(fixture: &StepUpFixture) -> PasskeyStepUpBeginRequest {
        PasskeyStepUpBeginRequest {
            schema: PASSKEY_STEP_UP_BEGIN_REQUEST_SCHEMA.to_string(),
            app_token: fixture.app_token.clone(),
            operation: "inspect.approve".to_string(),
            request: serde_json::json!({ "request_id": "inspect-action-1" }),
        }
    }

    fn pending_for(
        fixture: &StepUpFixture,
        ceremony_id: String,
        created_at: u64,
    ) -> PendingPasskeyStepUp {
        PendingPasskeyStepUp {
            schema: PASSKEY_STEP_UP_PENDING_SCHEMA.to_string(),
            ceremony_id,
            original_launch_id: fixture.launch.launch_id.clone(),
            launch_context: fixture.launch.launch_context.clone(),
            principal_id: fixture.context.principal_id.clone(),
            session_id: fixture.context.session_id.clone(),
            proof_binding_id: fixture.context.proof_binding_id.clone().unwrap(),
            grant_id: fixture.context.grant_id.clone(),
            credential_id: fixture.credential_id.clone(),
            credential_public_key_sha256: hex::encode(Sha256::digest(b"step-up-test-public-key")),
            rp_id: "localhost".to_string(),
            rp_origin: "http://localhost:61180".to_string(),
            operation: "inspect.approve".to_string(),
            request_sha256: canonical_request_sha256(
                &serde_json::json!({ "request_id": "inspect-action-1" }),
            )
            .unwrap(),
            created_at,
            expires_at: created_at.saturating_add(PASSKEY_STEP_UP_CEREMONY_TTL_SECS),
            non_delegatable: true,
        }
    }

    fn consumed_for(
        fixture: &StepUpFixture,
        step_up_id: String,
        consumed_at: u64,
    ) -> ConsumedPasskeyStepUp {
        ConsumedPasskeyStepUp {
            schema: PASSKEY_STEP_UP_CONSUMED_SCHEMA.to_string(),
            token_sha256: "a".repeat(64),
            step_up_id,
            original_launch_id: fixture.launch.launch_id.clone(),
            principal_id: fixture.context.principal_id.clone(),
            session_id: fixture.context.session_id.clone(),
            operation: "inspect.approve".to_string(),
            request_sha256: canonical_request_sha256(
                &serde_json::json!({ "request_id": "inspect-action-1" }),
            )
            .unwrap(),
            expires_at: consumed_at.saturating_add(PASSKEY_STEP_UP_TTL_SECS),
            consumed_at,
        }
    }

    #[tokio::test]
    async fn begin_limits_authentication_to_original_passkey_and_cancel_is_one_shot() {
        let fixture = fixture();
        let begin = passkey_step_up_begin_inner(
            &fixture.state,
            &fixture.host_headers,
            begin_request(&fixture),
        )
        .await
        .unwrap();
        assert_eq!(begin.schema, PASSKEY_STEP_UP_BEGIN_RESULT_SCHEMA);
        assert_eq!(begin.options.public_key.allow_credentials.len(), 1);
        assert_eq!(
            begin.options.public_key.allow_credentials[0].id,
            fixture.credential_id
        );
        assert_eq!(
            begin.options.public_key.timeout,
            PASSKEY_STEP_UP_CEREMONY_TTL_SECS * 1000
        );
        assert!(pending_path(fixture.data_dir.path(), &begin.ceremony_id)
            .unwrap()
            .is_file());

        let cancelled = passkey_step_up_cancel_inner(
            &fixture.state,
            &fixture.host_headers,
            PasskeyStepUpCancelRequest {
                schema: PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA.to_string(),
                ceremony_id: begin.ceremony_id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(cancelled.schema, PASSKEY_STEP_UP_CANCEL_RESULT_SCHEMA);
        assert_eq!(cancelled.status, "cancelled");
        assert!(!pending_path(fixture.data_dir.path(), &begin.ceremony_id)
            .unwrap()
            .exists());
        assert!(!fixture
            .state
            .identity_manager()
            .unwrap()
            .lock()
            .await
            .cancel_challenge(&begin.ceremony_id));

        let replay = passkey_step_up_cancel_inner(
            &fixture.state,
            &fixture.host_headers,
            PasskeyStepUpCancelRequest {
                schema: PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA.to_string(),
                ceremony_id: begin.ceremony_id,
            },
        )
        .await
        .unwrap_err();
        assert!(replay.to_string().contains("unavailable"));
    }

    #[tokio::test]
    async fn complete_rejects_substituted_credential_before_webauthn() {
        let fixture = fixture();
        let begin = passkey_step_up_begin_inner(
            &fixture.state,
            &fixture.host_headers,
            begin_request(&fixture),
        )
        .await
        .unwrap();
        let error = passkey_step_up_complete_inner(
            &fixture.state,
            &fixture.host_headers,
            PasskeyStepUpCompleteRequest {
                schema: PASSKEY_STEP_UP_COMPLETE_REQUEST_SCHEMA.to_string(),
                ceremony_id: begin.ceremony_id.clone(),
                response: AuthenticationResponse {
                    _id: "other-credential".to_string(),
                    raw_id: "other-credential".to_string(),
                    response: AuthenticatorAssertionResponse {
                        client_data_json: String::new(),
                        authenticator_data: String::new(),
                        signature: String::new(),
                        _user_handle: None,
                    },
                    _type: "public-key".to_string(),
                },
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("different registered passkey"));
        assert!(pending_path(fixture.data_dir.path(), &begin.ceremony_id)
            .unwrap()
            .exists());
        passkey_step_up_cancel_inner(
            &fixture.state,
            &fixture.host_headers,
            PasskeyStepUpCancelRequest {
                schema: PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA.to_string(),
                ceremony_id: begin.ceremony_id,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn begin_rejects_same_credential_id_with_different_public_key() {
        let fixture = fixture();
        let now = now_ts();
        crate::auth::upsert_principal_for_binding_as_role_named(
            fixture.data_dir.path(),
            ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
                credential_id: fixture.credential_id.clone(),
                public_key: "substituted-public-key".to_string(),
                sign_count: 1,
                user_verified: true,
                origin: "http://localhost:61180".to_string(),
                rp_id: "localhost".to_string(),
                created_at: now,
                last_used_at: now,
                revoked_at: None,
            }),
            fixture.context.principal_id.clone(),
            crate::auth::RuntimePrincipalRole::Admin,
            Some("Step-up tester"),
            now,
        )
        .unwrap();

        let error = passkey_step_up_begin_inner(
            &fixture.state,
            &fixture.host_headers,
            begin_request(&fixture),
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("original passkey credential is unavailable"));
    }

    #[test]
    fn signed_step_up_is_exact_intent_and_single_use() {
        let fixture = fixture();
        let request = serde_json::json!({ "request_id": "inspect-action-1" });
        let token = issue_passkey_step_up_token_for_test(
            fixture.data_dir.path(),
            &fixture.app_token,
            INBOX_CAPSULE_ID,
            "inspect.approve",
            &request,
        )
        .unwrap();

        let wrong_operation = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "wallet.approve",
            &request,
        )
        .unwrap_err();
        assert!(wrong_operation.to_string().contains("intent mismatch"));
        let wrong_request = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &serde_json::json!({ "request_id": "inspect-action-2" }),
        )
        .unwrap_err();
        assert!(wrong_request.to_string().contains("intent mismatch"));

        let substituted_app_token = issue_home_projection_launch_token_with_context(
            fixture.data_dir.path(),
            INBOX_CAPSULE_ID,
            INBOX_CAPSULE_ID,
            &fixture.context,
        )
        .unwrap();
        let substituted_launch = require_carried_home_launch_token(
            fixture.data_dir.path(),
            &substituted_app_token,
            &[INBOX_CAPSULE_ID],
        )
        .unwrap();
        let wrong_launch = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &substituted_launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(wrong_launch
            .to_string()
            .contains("original launch binding mismatch"));
        let wrong_actor_token = issue_home_projection_launch_token_with_context(
            fixture.data_dir.path(),
            SYSTEM_CAPSULE_ID,
            SYSTEM_CAPSULE_ID,
            &fixture.context,
        )
        .unwrap();
        let wrong_actor_launch = require_carried_home_launch_token(
            fixture.data_dir.path(),
            &wrong_actor_token,
            &[SYSTEM_CAPSULE_ID],
        )
        .unwrap();
        let wrong_actor = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &wrong_actor_launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(wrong_actor
            .to_string()
            .contains("original launch binding mismatch"));

        consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap();
        let decoded = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let reordered = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &serde_json::to_value(
                    serde_json::from_slice::<PasskeyStepUpEnvelope>(&decoded).unwrap(),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        assert_ne!(reordered, token);
        let reordered_replay = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &reordered,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(reordered_replay.to_string().contains("already been used"));
        let replay = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(replay.to_string().contains("already been used"));
    }

    #[test]
    fn transaction_effect_recovery_accepts_only_the_identical_consumed_step_up() {
        let fixture = fixture();
        let request = serde_json::json!({ "request_id": "inspect-action-1" });
        let token = issue_passkey_step_up_token_for_test(
            fixture.data_dir.path(),
            &fixture.app_token,
            INBOX_CAPSULE_ID,
            "inspect.approve",
            &request,
        )
        .unwrap();

        let consumed = consume_or_recover_passkey_step_up_effect(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap();
        assert!(!consumed.recovered);
        let recovered = consume_or_recover_passkey_step_up_effect(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap();
        assert!(recovered.recovered);
        assert_eq!(recovered.step_up_id, consumed.step_up_id);
        assert_eq!(recovered.request_sha256, consumed.request_sha256);

        let changed = consume_or_recover_passkey_step_up_effect(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &serde_json::json!({ "request_id": "inspect-action-2" }),
        )
        .unwrap_err();
        assert!(changed.to_string().contains("intent mismatch"));
        let ordinary_replay = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(ordinary_replay.to_string().contains("already been used"));
    }

    #[test]
    fn token_rejects_expiry_mixed_schema_and_extra_envelope_fields() {
        let fixture = fixture();
        let request = serde_json::json!({ "request_id": "inspect-action-1" });
        let now = now_ts();
        let pending = pending_for(
            &fixture,
            format!("passkey:step-up:{}", gateway_home_token::uuid_like_token()),
            now.saturating_sub(PASSKEY_STEP_UP_TTL_SECS + 1),
        );
        let expired = issue_step_up_token(
            fixture.data_dir.path(),
            &pending,
            now.saturating_sub(PASSKEY_STEP_UP_TTL_SECS + 1),
        )
        .unwrap()
        .token;
        assert!(consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &expired,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err()
        .to_string()
        .contains("expired"));

        let token = issue_passkey_step_up_token_for_test(
            fixture.data_dir.path(),
            &fixture.app_token,
            INBOX_CAPSULE_ID,
            "inspect.approve",
            &request,
        )
        .unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let mut mixed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        mixed["payload"]["schema"] =
            serde_json::Value::String("elastos.home.launch-token/v4".to_string());
        let mixed = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&mixed).unwrap());
        assert!(consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &mixed,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err()
        .to_string()
        .contains("unsupported passkey step-up token schema"));

        let mut extra: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        extra["home_token"] = serde_json::Value::String(fixture.app_token.clone());
        let extra = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&extra).unwrap());
        assert!(consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &extra,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err()
        .to_string()
        .contains("invalid passkey step-up token payload"));
    }

    #[test]
    fn committed_record_publication_never_overwrites_a_collision() {
        let fixture = fixture();
        let now = now_ts();
        let ceremony_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let path = pending_path(fixture.data_dir.path(), &ceremony_id).unwrap();
        let original = pending_for(&fixture, ceremony_id, now);
        persist_new_json(&path, &original, "passkey step-up state", |existing| {
            existing.validate(now, false)
        })
        .unwrap();
        let original_bytes = std::fs::read(&path).unwrap();

        let mut replacement = original.clone();
        replacement.operation = "wallet.send".to_string();
        let collision =
            persist_new_json(&path, &replacement, "passkey step-up state", |existing| {
                existing.validate(now, false)
            })
            .unwrap_err();
        assert_eq!(
            collision
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
        assert!(std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .all(|entry| {
                !is_private_staging_name(
                    entry
                        .unwrap()
                        .file_name()
                        .to_str()
                        .expect("UTF-8 test path"),
                )
            }));
    }

    #[test]
    fn replay_is_rejected_after_consumed_state_is_reopened() {
        let fixture = fixture();
        let request = serde_json::json!({ "request_id": "inspect-action-1" });
        let token = issue_passkey_step_up_token_for_test(
            fixture.data_dir.path(),
            &fixture.app_token,
            INBOX_CAPSULE_ID,
            "inspect.approve",
            &request,
        )
        .unwrap();
        consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &fixture.launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap();

        let reopened_launch = require_carried_home_launch_token(
            fixture.data_dir.path(),
            &fixture.app_token,
            &[INBOX_CAPSULE_ID],
        )
        .unwrap();
        prepare_consumed_capacity(fixture.data_dir.path(), now_ts()).unwrap();
        let replay = consume_passkey_step_up_token(
            fixture.data_dir.path(),
            &token,
            &reopened_launch,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(replay.to_string().contains("already been used"));
    }

    #[test]
    fn committed_state_rejects_malformed_and_nonregular_records() {
        let malformed = fixture();
        let malformed_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let malformed_path = pending_path(malformed.data_dir.path(), &malformed_id).unwrap();
        ensure_private_record_root(malformed_path.parent().unwrap()).unwrap();
        std::fs::write(&malformed_path, b"{}").unwrap();
        assert!(
            prepare_pending_capacity(malformed.data_dir.path(), now_ts())
                .unwrap_err()
                .to_string()
                .contains("malformed passkey step-up state")
        );

        let nonregular = fixture();
        let nonregular_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let nonregular_path = pending_path(nonregular.data_dir.path(), &nonregular_id).unwrap();
        ensure_private_record_root(nonregular_path.parent().unwrap()).unwrap();
        std::fs::create_dir(&nonregular_path).unwrap();
        assert!(
            prepare_pending_capacity(nonregular.data_dir.path(), now_ts())
                .unwrap_err()
                .to_string()
                .contains("malformed passkey step-up state")
        );
    }

    #[cfg(unix)]
    #[test]
    fn committed_state_rejects_symlink_records() {
        let fixture = fixture();
        let ceremony_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let path = pending_path(fixture.data_dir.path(), &ceremony_id).unwrap();
        ensure_private_record_root(path.parent().unwrap()).unwrap();
        let target = fixture.data_dir.path().join("outside-step-up-record");
        std::fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert!(prepare_pending_capacity(fixture.data_dir.path(), now_ts())
            .unwrap_err()
            .to_string()
            .contains("malformed passkey step-up state"));
    }

    #[test]
    fn cleanup_removes_only_exact_private_staging_names() {
        let fixture = fixture();
        let root = pending_root(fixture.data_dir.path()).unwrap();
        ensure_private_record_root(&root).unwrap();
        let record_digest = "b".repeat(64);
        let nonce = "c".repeat(32);
        let staging = root.join(format!(
            "{PASSKEY_STEP_UP_STAGING_PREFIX}{record_digest}-{nonce}{PASSKEY_STEP_UP_STAGING_SUFFIX}"
        ));
        std::fs::write(&staging, b"{").unwrap();
        prepare_pending_capacity(fixture.data_dir.path(), now_ts()).unwrap();
        assert!(!staging.exists());
        assert!(!root.join(format!("{record_digest}.json")).exists());

        let near_miss = root.join(format!(
            "{PASSKEY_STEP_UP_STAGING_PREFIX}{record_digest}-{nonce}{PASSKEY_STEP_UP_STAGING_SUFFIX}.other"
        ));
        std::fs::write(&near_miss, b"{").unwrap();
        assert!(prepare_pending_capacity(fixture.data_dir.path(), now_ts())
            .unwrap_err()
            .to_string()
            .contains("malformed passkey step-up state"));
        assert!(near_miss.exists());
    }

    #[cfg(unix)]
    #[test]
    fn persistence_enforces_private_directory_and_file_modes() {
        let fixture = fixture();
        let now = now_ts();
        let state = state_root(fixture.data_dir.path()).unwrap();
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o777)).unwrap();
        let pending_root = pending_root(fixture.data_dir.path()).unwrap();
        std::fs::create_dir(&pending_root).unwrap();
        std::fs::set_permissions(&pending_root, std::fs::Permissions::from_mode(0o777)).unwrap();

        let ceremony_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let pending = pending_for(&fixture, ceremony_id.clone(), now);
        let pending_path = pending_path(fixture.data_dir.path(), &ceremony_id).unwrap();
        persist_new_json(
            &pending_path,
            &pending,
            "passkey step-up state",
            |existing| existing.validate(now, false),
        )
        .unwrap();

        let consumed = consumed_for(
            &fixture,
            format!("step-up:{}", gateway_home_token::uuid_like_token()),
            now,
        );
        let consumed_path = consumed_path(fixture.data_dir.path(), &consumed.step_up_id).unwrap();
        persist_new_json(
            &consumed_path,
            &consumed,
            "passkey step-up consumed state",
            ConsumedPasskeyStepUp::validate,
        )
        .unwrap();

        let mode = |path: &Path| {
            std::fs::symlink_metadata(path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&pending_root), 0o700);
        assert_eq!(mode(consumed_path.parent().unwrap()), 0o700);
        assert_eq!(mode(&pending_path), 0o600);
        assert_eq!(mode(&consumed_path), 0o600);
    }

    #[test]
    fn completion_audit_binds_only_verified_context_and_request_hash() {
        let fixture = fixture();
        let now = now_ts();
        let ceremony_id = format!("passkey:step-up:{}", gateway_home_token::uuid_like_token());
        let pending = pending_for(&fixture, ceremony_id.clone(), now);
        let response = complete_step_up(fixture.data_dir.path(), &pending, now).unwrap();
        let envelope: PasskeyStepUpEnvelope = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(response.step_up_token.as_bytes())
                .unwrap(),
        )
        .unwrap();
        let state = crate::auth::load_auth_state(fixture.data_dir.path()).unwrap();
        let event = state
            .audit
            .iter()
            .find(|event| event.event_type == "auth.passkey_step_up.completed")
            .expect("passkey step-up completion audit");
        assert_eq!(
            event.principal_id.as_deref(),
            Some(pending.principal_id.as_str())
        );
        assert_eq!(
            event.proof_binding_id.as_deref(),
            Some(pending.proof_binding_id.as_str())
        );
        assert_eq!(
            event.session_id.as_deref(),
            Some(pending.session_id.as_str())
        );
        assert_eq!(event.challenge_id.as_deref(), Some(ceremony_id.as_str()));
        assert_eq!(
            event.capsule_id.as_deref(),
            Some(pending.launch_context.executable_actor.as_str())
        );
        assert_eq!(event.result, "success");
        assert!(event.signer_did.is_some());
        assert!(event.signature.is_some());

        let reason: serde_json::Value = serde_json::from_str(&event.reason).unwrap();
        assert_eq!(reason["original_launch_id"], pending.original_launch_id);
        assert_eq!(
            reason["selected_resource"],
            pending.launch_context.selected_resource
        );
        assert_eq!(
            reason["executable_actor"],
            pending.launch_context.executable_actor
        );
        assert_eq!(
            reason["authority_actor"],
            pending.launch_context.authority_actor
        );
        assert_eq!(reason["operation"], pending.operation);
        assert_eq!(reason["request_sha256"], pending.request_sha256);
        assert_eq!(reason["ceremony_id"], pending.ceremony_id);
        assert_eq!(reason["step_up_id"], envelope.payload.step_up_id);
        assert!(!event.reason.contains("inspect-action-1"));
        assert!(!event.reason.contains("request_id"));
    }

    #[test]
    fn completion_fails_closed_when_audit_cannot_be_persisted() {
        let fixture = fixture();
        let now = now_ts();
        let pending = pending_for(
            &fixture,
            format!("passkey:step-up:{}", gateway_home_token::uuid_like_token()),
            now,
        );
        let auth_state = crate::auth::auth_state_path(fixture.data_dir.path()).unwrap();
        std::fs::remove_file(&auth_state).unwrap();
        std::fs::create_dir(&auth_state).unwrap();

        let error = complete_step_up(fixture.data_dir.path(), &pending, now).unwrap_err();
        assert!(error
            .to_string()
            .contains("failed to persist passkey step-up completion audit"));
    }

    #[test]
    fn state_rejects_malformed_entries_and_both_capacity_overflows() {
        let malformed = fixture();
        let pending_root = pending_root(malformed.data_dir.path()).unwrap();
        std::fs::create_dir_all(&pending_root).unwrap();
        std::fs::write(pending_root.join("malformed.json"), b"{}").unwrap();
        assert!(
            prepare_pending_capacity(malformed.data_dir.path(), now_ts())
                .unwrap_err()
                .to_string()
                .contains("malformed passkey step-up state")
        );

        let pending_capacity = fixture();
        let now = now_ts();
        for index in 0..PASSKEY_STEP_UP_PENDING_CAPACITY {
            let ceremony_id = format!("passkey:step-up:{index:032x}");
            let pending = pending_for(&pending_capacity, ceremony_id.clone(), now);
            persist_new_json(
                &pending_path(pending_capacity.data_dir.path(), &ceremony_id).unwrap(),
                &pending,
                "passkey step-up state",
                |existing| existing.validate(now, false),
            )
            .unwrap();
        }
        assert!(
            prepare_pending_capacity(pending_capacity.data_dir.path(), now)
                .unwrap_err()
                .to_string()
                .contains("ceremony capacity exceeded")
        );

        let consumed_capacity = fixture();
        for index in 0..PASSKEY_STEP_UP_CONSUMED_CAPACITY {
            let token_sha256 = format!("{index:064x}");
            let marker = ConsumedPasskeyStepUp {
                schema: PASSKEY_STEP_UP_CONSUMED_SCHEMA.to_string(),
                token_sha256: token_sha256.clone(),
                step_up_id: format!("step-up:{index:032x}"),
                original_launch_id: consumed_capacity.launch.launch_id.clone(),
                principal_id: consumed_capacity.context.principal_id.clone(),
                session_id: consumed_capacity.context.session_id.clone(),
                operation: "inspect.approve".to_string(),
                request_sha256: canonical_request_sha256(
                    &serde_json::json!({ "request_id": "inspect-action-1" }),
                )
                .unwrap(),
                expires_at: now.saturating_add(PASSKEY_STEP_UP_TTL_SECS),
                consumed_at: now,
            };
            persist_new_json(
                &consumed_path(consumed_capacity.data_dir.path(), &marker.step_up_id).unwrap(),
                &marker,
                "passkey step-up consumed state",
                ConsumedPasskeyStepUp::validate,
            )
            .unwrap();
        }
        assert!(
            prepare_consumed_capacity(consumed_capacity.data_dir.path(), now)
                .unwrap_err()
                .to_string()
                .contains("consumption capacity exceeded")
        );
    }

    #[test]
    fn begin_and_cancel_requests_are_closed_and_versioned() {
        assert!(
            serde_json::from_value::<PasskeyStepUpBeginRequest>(serde_json::json!({
                "schema": PASSKEY_STEP_UP_BEGIN_REQUEST_SCHEMA,
                "app_token": "token",
                "operation": "inspect.approve",
                "request": {},
                "home_token": "legacy"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PasskeyStepUpCancelRequest>(serde_json::json!({
                "schema": "elastos.auth.passkey.authenticate.begin/v1",
                "ceremony_id": format!("passkey:step-up:{}", gateway_home_token::uuid_like_token())
            }))
            .unwrap()
            .schema
                != PASSKEY_STEP_UP_CANCEL_REQUEST_SCHEMA
        );
    }

    #[test]
    fn legacy_home_token_step_up_bodies_are_rejected() {
        let fixture = fixture();
        let launch_bytes = URL_SAFE_NO_PAD.decode(&fixture.app_token).unwrap();
        let mut launch_value: serde_json::Value = serde_json::from_slice(&launch_bytes).unwrap();
        assert!(launch_value.pointer("/payload/intent").is_none());
        launch_value["payload"]["intent"] = serde_json::json!({
            "operation": "inspect.approve",
            "request_sha256": "0".repeat(64)
        });
        let legacy_intent_token =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&launch_value).unwrap());
        assert!(require_carried_home_launch_token(
            fixture.data_dir.path(),
            &legacy_intent_token,
            &[INBOX_CAPSULE_ID],
        )
        .is_err());

        assert!(
            serde_json::from_value::<WalletAccountDeleteRequest>(serde_json::json!({
                "home_token": "legacy"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WalletApprovalApproveRequest>(serde_json::json!({
                "reason": "Approved in Wallet",
                "home_token": "legacy"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<InboxActionRequest>(serde_json::json!({
                "action_id": "wallet-approve-request:request-1",
                "home_token": "legacy"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::api::auth_gateway::FullRecoveryBundleExportRequest>(
                serde_json::json!({
                    "schema": "elastos.full-recovery-bundle.export.request/v1",
                    "principal_id": "person:local:test",
                    "localhost_root": "localhost://Users/test",
                    "home_token": "legacy"
                })
            )
            .is_err()
        );
    }
}
