//! Browser gateway helper contracts.
//!
//! Keep Browser-specific provider envelope handling here so the public gateway
//! module stays focused on HTTP route registration and response shaping.

use super::*;
use crate::api::browser_engine_protocol::{
    BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA, BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
    BROWSER_ENGINE_PROTOCOL_VERSION, BROWSER_ENGINE_PROVIDER_ID,
};
use std::sync::{Mutex as StdMutex, Weak};
use tokio::sync::{watch, Notify};
#[path = "gateway_browser_engine.rs"]
mod gateway_browser_engine;
#[path = "gateway_browser_response.rs"]
mod gateway_browser_response;
#[path = "gateway_browser_sessions.rs"]
mod gateway_browser_sessions;
#[path = "gateway_browser_stream.rs"]
mod gateway_browser_stream;
#[path = "gateway_browser_transport.rs"]
mod gateway_browser_transport;
#[path = "gateway_browser_validation.rs"]
mod gateway_browser_validation;
#[path = "gateway_browser_wallet.rs"]
mod gateway_browser_wallet;

pub(in crate::api::gateway) use gateway_browser_engine::*;
pub(in crate::api::gateway) use gateway_browser_response::*;
pub(in crate::api::gateway) use gateway_browser_sessions::*;
pub(in crate::api::gateway) use gateway_browser_stream::*;
pub(in crate::api::gateway) use gateway_browser_transport::*;
pub(in crate::api::gateway) use gateway_browser_validation::*;
pub(in crate::api::gateway) use gateway_browser_wallet::*;

const BROWSER_PROFILE_STORAGE: &str = "principal_owned_profile_disk";
const BROWSER_PROFILE_STORAGE_POSTURE: &str = "principal_owned_reset_scoped_unprotected";
const BROWSER_PROFILE_RECOVERY: &str = "not_recovery_kit_packaged";

#[derive(Serialize)]
pub(super) struct BrowserSummaryResponse {
    pub(super) schema: String,
    pub(super) app: HomeCapsuleIdentity,
    pub(super) principal_id: String,
    pub(super) sessions: serde_json::Value,
    pub(super) engine_adapter: serde_json::Value,
    pub(super) net: serde_json::Value,
    pub(super) wallet_bridge: serde_json::Value,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserSummaryQuery {
    #[serde(default)]
    pub(super) browser_instance: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserOpenRequest {
    pub(super) url: String,
    #[serde(default)]
    pub(super) reason: Option<String>,
    #[serde(default)]
    pub(super) remote_exit_id: Option<String>,
    #[serde(default)]
    pub(super) adapter_id: Option<String>,
    #[serde(default)]
    pub(super) browser_instance: Option<String>,
    #[serde(default)]
    pub(super) viewport: Option<BrowserViewportRequest>,
    pub(super) display_mode: BrowserDisplayMode,
    pub(super) guarantee_level: BrowserGuaranteeLevel,
    #[serde(default)]
    pub(super) async_open: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserViewportRequest {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum BrowserDisplayMode {
    WebrtcRemoteDisplay,
    NativeSurface,
}

impl BrowserDisplayMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::WebrtcRemoteDisplay => "webrtc_remote_display",
            Self::NativeSurface => "native_surface",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum BrowserGuaranteeLevel {
    MechanismMicrovm,
    OperatorRbi,
    PolicyWebview,
    Diagnostic,
}

impl BrowserGuaranteeLevel {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MechanismMicrovm => "mechanism_microvm",
            Self::OperatorRbi => "operator_rbi",
            Self::PolicyWebview => "policy_webview",
            Self::Diagnostic => "diagnostic",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserInputRequest {
    pub(super) event: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserWebrtcSignalRequest {
    #[serde(rename = "type")]
    pub(super) signal_type: String,
    #[serde(default)]
    pub(super) channel: Option<String>,
    #[serde(default)]
    pub(super) sdp: Option<String>,
    #[serde(default)]
    pub(super) candidate: Option<serde_json::Value>,
}

pub(super) struct BrowserProviderResourceCall {
    pub(super) scheme: &'static str,
    pub(super) resource: String,
    pub(super) request: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserPageCloseRequest {
    pub(super) schema: String,
    pub(super) cleanup_id: String,
}

#[derive(Debug)]
struct BrowserOpenFailure {
    status: StatusCode,
    body: BrowserOpenFailureBody,
    outcome: serde_json::Value,
}

#[derive(Debug)]
enum BrowserOpenFailureBody {
    Text(String),
    Json(serde_json::Value),
}

fn browser_open_outcome(
    state: &str,
    page_acquired: bool,
    vm_acquired: bool,
    stream_acquired: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.browser.open-outcome/v1",
        "state": state,
        "effects": {
            "page_acquired": page_acquired,
            "vm_acquired": vm_acquired,
            "stream_acquired": stream_acquired,
        },
    })
}

fn browser_terminal_pre_effect_outcome() -> serde_json::Value {
    browser_open_outcome("terminal_pre_effect_failure", false, false, false)
}

fn browser_terminal_post_effect_outcome() -> serde_json::Value {
    browser_open_outcome("terminal_post_effect_cleanup", true, true, true)
}

fn browser_terminal_post_dispatch_outcome(
    page_acquired: bool,
    vm_acquired: bool,
) -> serde_json::Value {
    browser_open_outcome(
        "terminal_post_effect_cleanup",
        page_acquired,
        vm_acquired,
        true,
    )
}

fn browser_cleanup_pending_outcome(
    page_acquired: bool,
    vm_acquired: bool,
    stream_acquired: bool,
) -> serde_json::Value {
    browser_open_outcome(
        "cleanup_pending",
        page_acquired,
        vm_acquired,
        stream_acquired,
    )
}

fn browser_launch_reconciliation_pending_outcome() -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.browser.open-outcome/v1",
        "state": "cleanup_pending",
        "effects": {
            "page_acquired": serde_json::Value::Null,
            "vm_acquired": serde_json::Value::Null,
            "stream_acquired": true,
        },
        "ownership": "launch_reconciliation_pending",
    })
}

impl BrowserOpenFailure {
    fn text(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: BrowserOpenFailureBody::Text(message.into()),
            outcome: browser_terminal_pre_effect_outcome(),
        }
    }

    fn json(status: StatusCode, body: serde_json::Value) -> Self {
        Self {
            status,
            body: BrowserOpenFailureBody::Json(body),
            outcome: browser_terminal_pre_effect_outcome(),
        }
    }

    fn with_outcome(mut self, outcome: serde_json::Value) -> Self {
        self.outcome = outcome;
        self
    }

    fn provider(scheme: &str, err: anyhow::Error) -> Self {
        let (status, message) = gateway_provider_error_tuple(scheme, err);
        Self::text(status, message)
    }

    fn into_response(self) -> Response {
        let status = self.status;
        (status, Json(self.status_value())).into_response()
    }

    fn status_value(&self) -> serde_json::Value {
        let mut value = match &self.body {
            BrowserOpenFailureBody::Text(message) => serde_json::json!({
                "schema": "elastos.browser.open-error/v1",
                "ok": false,
                "message": message,
            }),
            BrowserOpenFailureBody::Json(body) => body.clone(),
        };
        if let Some(object) = value.as_object_mut() {
            object
                .entry("schema".to_string())
                .or_insert_with(|| serde_json::json!("elastos.browser.open-error/v1"));
            object
                .entry("ok".to_string())
                .or_insert_with(|| serde_json::json!(false));
            object.insert("outcome".to_string(), self.outcome.clone());
            object.insert(
                "http_status".to_string(),
                serde_json::json!(self.status.as_u16()),
            );
        }
        value
    }
}

pub(super) async fn browser_app_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<BrowserSummaryQuery>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();
    let browser_instance = match browser_instance_id(query.browser_instance) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let engine_adapter =
        browser_engine_summary(state.provider_registry.as_ref(), &context.principal_id).await;
    let net = browser_net_summary(state.provider_registry.as_ref(), &context.principal_id).await;
    let wallet_accounts = system_wallet_accounts_summary(&state, &authority).await;
    let wallet_status = if wallet_accounts.linked_count > 0 {
        "configured"
    } else {
        "no_accounts"
    };
    Json(BrowserSummaryResponse {
        schema: "elastos.browser.runtime/v1".to_string(),
        app: HomeCapsuleIdentity {
            id: BROWSER_CAPSULE_ID.to_string(),
            route: "/apps/browser/".to_string(),
        },
        sessions: browser_gateway_session_status(
            &state.data_dir,
            &context.principal_id,
            Some(authority.verified_context().launch_id()),
            browser_instance.as_deref(),
        )
        .await,
        principal_id: context.principal_id,
        engine_adapter,
        net,
        wallet_bridge: serde_json::json!({
            "status": wallet_status,
            "provider": "elastos://wallet/*",
            "injection": "runtime-mediated-eip1193",
            "accounts": wallet_accounts.linked_count,
            "reason": "Browser pages receive only a constrained Runtime-mediated EIP-1193 bridge. Signing requests become Runtime Wallet/Inbox approval requests."
        }),
    })
    .into_response()
}

pub(super) async fn browser_app_profile_reset(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if browser_principal_has_live_sessions(&state.data_dir, &context.principal_id).await {
        return (
            StatusCode::CONFLICT,
            "Browser profile reset requires all Browser pages for this account to be closed",
        )
            .into_response();
    }
    let (disk_path, _) =
        match browser_profile_launch_descriptor(&state.data_dir, &context.principal_id) {
            Ok(profile) => profile,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let removed = match tokio::fs::remove_file(&disk_path).await {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return gateway_provider_error_response(
                "browser",
                anyhow::anyhow!("Browser profile reset failed: {}", err),
            )
        }
    };
    Json(serde_json::json!({
        "schema": "elastos.browser.profile-reset/v1",
        "status": "ok",
        "profile": {
            "schema": "elastos.browser.profile/v1",
            "scope": "active_principal",
            "storage": BROWSER_PROFILE_STORAGE,
            "storage_posture": BROWSER_PROFILE_STORAGE_POSTURE,
            "protected_storage": false,
            "encrypted": false,
            "recoverable": false,
            "recovery": BROWSER_PROFILE_RECOVERY,
            "uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4",
            "reset": "whole_profile",
        },
        "removed_profile_disk": removed,
    }))
    .into_response()
}

fn browser_profile_launch_descriptor(
    data_dir: &FsPath,
    principal_id: &str,
) -> anyhow::Result<(PathBuf, serde_json::Value)> {
    let Some(profile_key) = elastos_common::browser_profile_key_from_value(principal_id) else {
        anyhow::bail!("Browser launch requires a principal profile key");
    };
    let principal_root = crate::auth::principal_localhost_root(principal_id);
    let profile_uri = format!("{principal_root}/BrowserProfiles/default/profile.ext4");
    let disk_path = rooted_localhost_fs_path(data_dir, &profile_uri)
        .ok_or_else(|| anyhow::anyhow!("invalid Browser profile disk path"))?;
    let disk_path_text = disk_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Browser profile disk path must be UTF-8"))?
        .to_string();
    Ok((
        disk_path,
        serde_json::json!({
            "schema": "elastos.browser.profile/v1",
            "scope": "active_principal",
            "storage": BROWSER_PROFILE_STORAGE,
            "storage_posture": BROWSER_PROFILE_STORAGE_POSTURE,
            "protected_storage": false,
            "encrypted": false,
            "recoverable": false,
            "recovery": BROWSER_PROFILE_RECOVERY,
            "uri": profile_uri,
            "public_uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4",
            "profile_key": profile_key,
            "disk_path": disk_path_text,
            "reset": "whole_profile",
        }),
    ))
}

pub(super) async fn browser_app_open(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserOpenRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let home_token = home_launch_token_header(&headers);
    let request_origin = browser_request_origin(&headers);
    if input.async_open {
        let owner_launch_id = authority.verified_context().launch_id();
        let intent_hash = browser_open_intent_hash(&input);
        let reservation = match create_browser_open_job(
            &state.data_dir,
            &context.principal_id,
            owner_launch_id,
            &intent_hash,
        )
        .await
        {
            Ok(reservation) => reservation,
            Err((status, message)) => return (status, message).into_response(),
        };
        let job = reservation.handle;
        let open_id = job.id.clone();
        if reservation.should_spawn {
            let state_for_task = state.clone();
            tokio::spawn(async move {
                match execute_browser_open(
                    &state_for_task,
                    context,
                    authority,
                    input,
                    home_token,
                    request_origin,
                )
                .await
                {
                    Ok(result) => complete_browser_open_job(&job, result).await,
                    Err(error) => fail_browser_open_job(&job, error.status_value()).await,
                }
            });
        }
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "schema": "elastos.browser.open-accepted/v1",
                "status": "pending",
                "open_id": open_id,
                "status_url": format!("/api/apps/browser/open/{open_id}"),
            })),
        )
            .into_response();
    }
    match execute_browser_open(
        &state,
        context,
        authority,
        input,
        home_token,
        request_origin,
    )
    .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => error.into_response(),
    }
}

pub(super) async fn browser_app_open_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(open_id): Path<String>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id();
    if !is_safe_runtime_id(&open_id) {
        return (StatusCode::BAD_REQUEST, "invalid Browser open id").into_response();
    }
    let Some(snapshot) = browser_open_job_for_owner(
        &state.data_dir,
        &open_id,
        &context.principal_id,
        owner_launch_id,
    )
    .await
    else {
        return (StatusCode::NOT_FOUND, "Browser open job not found").into_response();
    };
    Json(browser_open_status_value(&open_id, snapshot)).into_response()
}

fn browser_open_intent_hash(input: &BrowserOpenRequest) -> String {
    let intent = serde_json::json!({
        "url": input.url.trim(),
        "reason": input.reason.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "remote_exit_id": input.remote_exit_id.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "adapter_id": input.adapter_id.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "browser_instance": input.browser_instance.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "viewport": input.viewport.as_ref().map(|viewport| serde_json::json!({
            "width": viewport.width,
            "height": viewport.height,
        })),
        "display_mode": input.display_mode.as_str(),
        "guarantee_level": input.guarantee_level.as_str(),
    });
    let canonical = serde_json::to_vec(&intent).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

fn browser_open_status_value(open_id: &str, snapshot: BrowserOpenJobSnapshot) -> serde_json::Value {
    match snapshot {
        BrowserOpenJobSnapshot::Pending => serde_json::json!({
            "schema": "elastos.browser.open-status/v1",
            "open_id": open_id,
            "status": "pending",
        }),
        BrowserOpenJobSnapshot::Completed(result) => serde_json::json!({
            "schema": "elastos.browser.open-status/v1",
            "open_id": open_id,
            "status": "completed",
            "result": result,
        }),
        BrowserOpenJobSnapshot::Failed(error) => serde_json::json!({
            "schema": "elastos.browser.open-status/v1",
            "open_id": open_id,
            "status": "failed",
            "error": error,
        }),
    }
}

async fn execute_browser_open(
    state: &GatewayState,
    context: HomeLaunchTokenContext,
    authority: RuntimeWalletAuthority,
    input: BrowserOpenRequest,
    home_token: Option<String>,
    request_origin: Option<String>,
) -> Result<serde_json::Value, BrowserOpenFailure> {
    let BrowserOpenRequest {
        url: requested_url,
        reason,
        remote_exit_id,
        adapter_id,
        browser_instance,
        viewport,
        display_mode,
        guarantee_level,
        async_open: _,
    } = input;
    let (url, target) = match browser_url_to_stream_target(&requested_url) {
        Ok(value) => value,
        Err(err) => {
            return Err(BrowserOpenFailure::text(
                StatusCode::BAD_REQUEST,
                err.to_string(),
            ))
        }
    };
    if let Err(err) = validate_browser_launch_contract(display_mode, guarantee_level) {
        return Err(BrowserOpenFailure::text(
            StatusCode::BAD_REQUEST,
            err.to_string(),
        ));
    }
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return Err(BrowserOpenFailure::provider(
                "browser",
                anyhow::anyhow!("browser providers unavailable"),
            ));
        }
    };
    cleanup_stale_browser_pages(state).await;
    let reason = reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "open browser page".to_string());
    let open_request_id = browser_effect_request_id("open", &url);
    let stream_nonce = browser_stream_nonce(&open_request_id);
    let remote_exit_id = match browser_remote_exit_id(remote_exit_id) {
        Ok(value) => value,
        Err(message) => return Err(BrowserOpenFailure::text(StatusCode::BAD_REQUEST, message)),
    };
    let requested_adapter_id = match browser_engine_adapter_id(adapter_id) {
        Ok(value) => value,
        Err(message) => return Err(BrowserOpenFailure::text(StatusCode::BAD_REQUEST, message)),
    };
    let browser_instance = match browser_instance_id(browser_instance) {
        Ok(value) => value,
        Err(message) => return Err(BrowserOpenFailure::text(StatusCode::BAD_REQUEST, message)),
    };
    let viewport = match viewport {
        Some(viewport) => match browser_viewport_value(viewport) {
            Ok(value) => Some(value),
            Err(err) => {
                return Err(BrowserOpenFailure::text(
                    StatusCode::BAD_REQUEST,
                    err.to_string(),
                ))
            }
        },
        None => None,
    };
    let (_, profile) =
        match browser_profile_launch_descriptor(&state.data_dir, &context.principal_id) {
            Ok(profile) => profile,
            Err(err) => return Err(BrowserOpenFailure::provider("browser", err)),
        };
    let engine_registration = match registry
        .registration_for_uri("elastos://browser-engine/launch")
        .await
    {
        Some(registration) => registration,
        None => {
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!("Browser Engine provider route is unavailable"),
            ))
        }
    };
    let adapter_id = match resolve_browser_engine_adapter(
        registry.as_ref(),
        &context.principal_id,
        requested_adapter_id.as_deref(),
    )
    .await
    {
        Ok(adapter_id) => adapter_id,
        Err(message) => {
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!(message),
            ))
        }
    };
    let profile_key = profile
        .get("profile_key")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let lifecycle = BrowserLaunchLifecycle {
        owner_launch_id: authority.verified_context().launch_id().to_string(),
        browser_instance,
        url: url.clone(),
        exit_id: browser_lifecycle_exit_id(remote_exit_id.as_deref()),
        engine_route_provider: engine_registration.provider.clone(),
        selected_engine_adapter: Some(adapter_id.clone()),
        profile_key_hash: browser_lifecycle_hash(profile_key),
        vm_key_hash: browser_lifecycle_vm_key_hash(&[
            profile_key,
            remote_exit_id.as_deref().unwrap_or("local-runtime"),
            &adapter_id,
            display_mode.as_str(),
            guarantee_level.as_str(),
            &target,
        ]),
    };
    let launch_reservation =
        match reserve_browser_launch(&state.data_dir, &context.principal_id, lifecycle).await {
            Ok(reservation) => reservation,
            Err((status, message)) => return Err(BrowserOpenFailure::text(status, message)),
        };
    if let Err((status, message)) = append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.open.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &open_request_id,
            result: "requested",
            method: display_mode.as_str(),
            resource: &target,
            page_url: &url,
            origin: request_origin.as_deref(),
            decision: "runtime_net_exit_policy",
        },
    ) {
        release_browser_launch(&launch_reservation).await;
        return Err(BrowserOpenFailure::text(status, message));
    }
    mark_browser_launch_preparing_image(&launch_reservation).await;
    let stream_request = serde_json::json!({
        "op": "stream",
        "target": target,
        "principal_id": context.principal_id.clone(),
        "reason": reason.clone(),
        "stream_nonce": stream_nonce,
        "remote_exit_id": remote_exit_id,
    });
    let stream_session =
        match browser_reserve_stream_session(registry.as_ref(), &stream_request).await {
            Ok(receipt) => receipt,
            Err((provider, err)) => {
                release_browser_launch(&launch_reservation).await;
                return Err(BrowserOpenFailure::provider(provider, err));
            }
        };
    let stream_cleanup = browser_stream_cleanup(&stream_session);
    let stream_session =
        match browser_attach_runtime_stream_path(&state.data_dir, stream_session).await {
            Ok(receipt) => receipt,
            Err(err) => {
                let outcome = release_browser_open_resources(
                    state,
                    &launch_reservation,
                    stream_cleanup.clone(),
                    false,
                )
                .await;
                return Err(BrowserOpenFailure::provider("browser", err).with_outcome(outcome));
            }
        };
    let engine_stream_id = match stream_session
        .get("stream_id")
        .and_then(serde_json::Value::as_str)
    {
        Some(stream_id) => stream_id.to_string(),
        None => {
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(BrowserOpenFailure::provider(
                "browser",
                anyhow::anyhow!("Browser stream session omitted its exact stream identity"),
            )
            .with_outcome(outcome));
        }
    };
    let vz_transport_launch = match prepare_browser_vz_transport_launch(
        &state.data_dir,
        BrowserVzTransportLaunchBinding {
            generation: launch_reservation.generation(),
            page_id: launch_reservation.page_id(),
            vm_id: launch_reservation.vm_id(),
            principal_id: &context.principal_id,
            egress_stream_id: &engine_stream_id,
            egress_target: &target,
            egress_runtime_socket_path: stream_session
                .pointer("/adapter_ipc/runtime_stream_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        },
    ) {
        Ok(launch) => launch,
        Err(message) => {
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
    };
    if let Some(transport) = vz_transport_launch.as_ref() {
        if let Err(message) = bind_browser_vz_transport_authority(
            &state.data_dir,
            &launch_reservation,
            &engine_stream_id,
            stream_cleanup.clone(),
            transport.authority.clone(),
        )
        .await
        {
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
        if let Err(err) = spawn_browser_vz_fixed_media_listener(&transport.authority).await {
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(BrowserOpenFailure::provider("browser", err).with_outcome(outcome));
        }
    }
    let engine_stream_session = browser_engine_stream_session(&stream_session);
    let wallet = browser_wallet_bridge_payload(
        state,
        &context,
        &authority,
        home_token.as_deref(),
        request_origin.as_deref(),
    )
    .await;
    let mut engine_launch_request = serde_json::json!({
        "url": url.clone(),
        "stream_session": engine_stream_session,
        "lifecycle_generation": launch_reservation.generation(),
        "principal_id": context.principal_id.clone(),
        "reason": reason.clone(),
        "profile": profile,
        "wallet": wallet,
        "viewport": viewport,
        "display_mode": display_mode,
        "guarantee_level": guarantee_level,
        "adapter_id": adapter_id,
    });
    if let Some(transport) = vz_transport_launch.as_ref() {
        engine_launch_request["page_id"] = serde_json::json!(launch_reservation.page_id());
        engine_launch_request["vm_id"] = serde_json::json!(launch_reservation.vm_id());
        engine_launch_request["transport_authority"] = transport.authority.clone();
        engine_launch_request["transport_secret"] = transport.secret.clone();
    }
    mark_browser_launch_starting_vm(&launch_reservation).await;
    let engine_call = match browser_provider_resource_call(
        "browser-engine",
        "launch",
        "elastos://browser-engine/launch".to_string(),
        engine_launch_request,
    ) {
        Ok(call) => call,
        Err((status, message)) => {
            mark_browser_launch_failed(&launch_reservation, message.clone()).await;
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(BrowserOpenFailure::text(status, message).with_outcome(outcome));
        }
    };
    if vz_transport_launch.is_some() {
        if let Err(message) = mark_browser_vz_transport_dispatched(
            &state.data_dir,
            &launch_reservation,
            &engine_stream_id,
        )
        .await
        {
            let outcome = release_browser_open_resources(
                state,
                &launch_reservation,
                stream_cleanup.clone(),
                false,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
    }
    let engine_response = match browser_provider_resource_response(state, engine_call).await {
        Ok(value) => value,
        Err((status, message)) => {
            let outcome = reconcile_dispatched_browser_launch_failure(
                state,
                &launch_reservation,
                &context.principal_id,
                authority.verified_context().launch_id(),
                &engine_stream_id,
                stream_cleanup.clone(),
                &message,
            )
            .await;
            return Err(BrowserOpenFailure::text(status, message).with_outcome(outcome));
        }
    };
    if engine_response
        .get("status")
        .and_then(|value| value.as_str())
        == Some("error")
    {
        let code = engine_response
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("provider_error");
        let message = engine_response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("Browser Engine Adapter rejected page launch");
        let exact_did_not_act = consume_browser_vz_did_not_act_settlement(
            state,
            &launch_reservation,
            &engine_stream_id,
            stream_cleanup.clone(),
            &engine_response,
        )
        .await;
        if matches!(
            code,
            "engine_unavailable" | "byte_transport_unavailable" | "display_session_unavailable"
        ) {
            let outcome = match exact_did_not_act {
                Some(outcome) => outcome,
                None => {
                    reconcile_dispatched_browser_launch_failure(
                        state,
                        &launch_reservation,
                        &context.principal_id,
                        authority.verified_context().launch_id(),
                        &engine_stream_id,
                        stream_cleanup.clone(),
                        message,
                    )
                    .await
                }
            };
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider unavailable: {}", message),
            )
            .with_outcome(outcome));
        }
        if matches!(code, "browser_capacity_unavailable" | "resources_in_use") {
            let outcome = match exact_did_not_act {
                Some(outcome) => outcome,
                None => {
                    reconcile_dispatched_browser_launch_failure(
                        state,
                        &launch_reservation,
                        &context.principal_id,
                        authority.verified_context().launch_id(),
                        &engine_stream_id,
                        stream_cleanup.clone(),
                        message,
                    )
                    .await
                }
            };
            return Err(BrowserOpenFailure::json(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({
                    "schema": "elastos.browser.open-error/v1",
                    "ok": false,
                    "code": code,
                    "message": message,
                }),
            )
            .with_outcome(outcome));
        }
        let outcome = match exact_did_not_act {
            Some(outcome) => outcome,
            None => {
                reconcile_dispatched_browser_launch_failure(
                    state,
                    &launch_reservation,
                    &context.principal_id,
                    authority.verified_context().launch_id(),
                    &engine_stream_id,
                    stream_cleanup.clone(),
                    message,
                )
                .await
            }
        };
        return Err(BrowserOpenFailure::provider(
            "browser-engine",
            anyhow::anyhow!(message.to_string()),
        )
        .with_outcome(outcome));
    }
    if let Some(message) = provider_response_error_message(&engine_response) {
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            &message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    }
    let raw_engine_page = match provider_response_data(&engine_response) {
        Some(data) => data,
        None => {
            let message = "browser-engine provider returned an invalid launch response";
            let outcome = reconcile_dispatched_browser_launch_failure(
                state,
                &launch_reservation,
                &context.principal_id,
                authority.verified_context().launch_id(),
                &engine_stream_id,
                stream_cleanup.clone(),
                message,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
    };
    let engine_page = match validate_browser_engine_page(
        raw_engine_page.clone(),
        display_mode,
        guarantee_level,
    ) {
        Ok(data) => data,
        Err(err) => {
            let outcome = reconcile_dispatched_browser_launch_failure(
                state,
                &launch_reservation,
                &context.principal_id,
                authority.verified_context().launch_id(),
                &engine_stream_id,
                stream_cleanup.clone(),
                &err.to_string(),
            )
            .await;
            return Err(BrowserOpenFailure::provider("browser-engine", err).with_outcome(outcome));
        }
    };
    let Some(page_id) = engine_page.get("page_id").and_then(|value| value.as_str()) else {
        let message = "browser-engine provider returned page without page_id";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    };
    let Some(engine_adapter) = engine_page.get("adapter").and_then(|value| value.as_str()) else {
        let message = "browser-engine provider returned page without adapter binding";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    };
    let Some(engine) = engine_page.get("engine").and_then(|value| value.as_str()) else {
        let message = "browser-engine provider returned page without engine binding";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    };
    let Some(engine_provider) = engine_page.get("provider").and_then(|value| value.as_str()) else {
        let message = "browser-engine provider omitted provider identity";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    };
    let Some(engine_protocol_version) = engine_page
        .get("protocol_version")
        .and_then(|value| value.as_str())
    else {
        let message = "browser-engine provider omitted protocol version";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    };
    if !is_safe_runtime_id(page_id)
        || vz_transport_launch
            .as_ref()
            .is_some_and(|_| page_id != launch_reservation.page_id())
        || engine_provider != BROWSER_ENGINE_PROVIDER_ID
        || engine_protocol_version != BROWSER_ENGINE_PROTOCOL_VERSION
        || !is_safe_runtime_id(engine_adapter)
        || !is_safe_runtime_id(engine)
    {
        let message = "browser-engine provider returned an unsafe lifecycle binding";
        let outcome = reconcile_dispatched_browser_launch_failure(
            state,
            &launch_reservation,
            &context.principal_id,
            authority.verified_context().launch_id(),
            &engine_stream_id,
            stream_cleanup.clone(),
            message,
        )
        .await;
        return Err(
            BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                .with_outcome(outcome),
        );
    }
    let public_transport_proof = if let Some(transport) = vz_transport_launch.as_ref() {
        match browser_vz_public_transport_proof(
            &transport.authority,
            raw_engine_page
                .get("transport_receipt")
                .unwrap_or(&serde_json::Value::Null),
        ) {
            Ok(proof) => Some(proof),
            Err(message) => {
                let outcome = reconcile_dispatched_browser_launch_failure(
                    state,
                    &launch_reservation,
                    &context.principal_id,
                    authority.verified_context().launch_id(),
                    &engine_stream_id,
                    stream_cleanup.clone(),
                    &message,
                )
                .await;
                return Err(BrowserOpenFailure::provider(
                    "browser-engine",
                    anyhow::anyhow!(message),
                )
                .with_outcome(outcome));
            }
        }
    } else {
        None
    };
    let provider_cleanup = match browser_provider_cleanup_binding(
        &raw_engine_page,
        launch_reservation.generation(),
        page_id,
        engine_adapter,
        engine,
        engine_page
            .get("stream_id")
            .and_then(|value| value.as_str()),
        vz_transport_launch
            .as_ref()
            .map(|transport| &transport.authority),
    ) {
        Ok(binding) => binding,
        Err(message) => {
            let outcome = reconcile_dispatched_browser_launch_failure(
                state,
                &launch_reservation,
                &context.principal_id,
                authority.verified_context().launch_id(),
                &engine_stream_id,
                stream_cleanup.clone(),
                &message,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser-engine", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
    };
    let mut browser_page = engine_page.clone();
    if let Some(page) = browser_page.as_object_mut() {
        page.remove("runtime_cleanup");
        page.remove("transport_authority");
        page.remove("transport_receipt");
        if let Some(proof) = public_transport_proof {
            page.insert("transport_proof".to_string(), proof);
        }
    }
    let runtime_cleanup = match complete_browser_launch(
        &state.data_dir,
        &launch_reservation,
        BrowserLaunchEffect {
            page_id: page_id.to_string(),
            engine_provider: engine_provider.to_string(),
            engine_protocol_version: engine_protocol_version.to_string(),
            engine_adapter: engine_adapter.to_string(),
            engine: engine.to_string(),
            provider_cleanup,
            browser_page: browser_page.clone(),
            stream_cleanup: stream_cleanup.clone(),
        },
    )
    .await
    {
        Ok(handle) => handle,
        Err(message) => {
            let outcome = reap_browser_open_effect_after_failure(
                state,
                &launch_reservation,
                page_id,
                &context.principal_id,
                authority.verified_context().launch_id(),
                false,
            )
            .await;
            return Err(
                BrowserOpenFailure::provider("browser", anyhow::anyhow!(message))
                    .with_outcome(outcome),
            );
        }
    };
    if let Err((status, message)) = append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.open.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &open_request_id,
            result: "allowed",
            method: display_mode.as_str(),
            resource: &target,
            page_url: &url,
            origin: request_origin.as_deref(),
            decision: "browser_engine_provider",
        },
    ) {
        let outcome = reap_browser_open_effect_after_failure(
            state,
            &launch_reservation,
            page_id,
            &context.principal_id,
            authority.verified_context().launch_id(),
            true,
        )
        .await;
        return Err(BrowserOpenFailure::text(status, message).with_outcome(outcome));
    }
    Ok(serde_json::json!({
        "schema": "elastos.browser.open-result/v1",
        "url": url,
        "target": target,
        "guarantee_level": guarantee_level.as_str(),
        "stream_session": browser_visible_stream_session(&stream_session),
        "engine_page": browser_page,
        "runtime_cleanup": runtime_cleanup,
    }))
}

async fn consume_browser_vz_did_not_act_settlement(
    state: &GatewayState,
    reservation: &BrowserLaunchReservation,
    stream_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
    engine_response: &serde_json::Value,
) -> Option<serde_json::Value> {
    let adapter = engine_response
        .get("adapter")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() <= 128 && is_safe_runtime_id(value))?;
    let settlement = engine_response.get("launch_settlement_result")?;
    let authority = browser_launch_transport_authority(reservation).await?;
    if validate_browser_vz_did_not_act_settlement(
        settlement,
        &authority,
        reservation,
        stream_id,
        adapter,
    )
    .is_err()
    {
        return None;
    }
    mark_browser_launch_failed(
        reservation,
        settlement
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Browser VZ launch did not act")
            .to_string(),
    )
    .await;
    Some(release_browser_open_resources(state, reservation, stream_cleanup, true).await)
}

fn validate_browser_vz_did_not_act_settlement(
    settlement: &serde_json::Value,
    authority: &serde_json::Value,
    reservation: &BrowserLaunchReservation,
    stream_id: &str,
    adapter: &str,
) -> Result<(), String> {
    let (acted, _) = validate_browser_vz_launch_settlement_binding(settlement, authority)?;
    if settlement.get("state").and_then(serde_json::Value::as_str) != Some("did_not_act")
        || acted
        || authority
            .get("generation")
            .and_then(serde_json::Value::as_str)
            != Some(reservation.generation())
        || authority.get("page_id").and_then(serde_json::Value::as_str)
            != Some(reservation.page_id())
        || authority.get("vm_id").and_then(serde_json::Value::as_str) != Some(reservation.vm_id())
        || authority
            .pointer("/egress/stream_id")
            .and_then(serde_json::Value::as_str)
            != Some(stream_id)
        || reservation.selected_engine_adapter() != Some(adapter)
    {
        return Err("Browser VZ DidNotAct settlement binding is not exact".to_string());
    }
    Ok(())
}

fn validate_browser_vz_launch_settlement_binding(
    settlement: &serde_json::Value,
    authority: &serde_json::Value,
) -> Result<(bool, bool), String> {
    validate_browser_vz_transport_authority(authority)?;
    let object = settlement
        .as_object()
        .ok_or_else(|| "Browser VZ launch settlement must be an object".to_string())?;
    let keys = [
        "schema",
        "state",
        "message",
        "binding_hash",
        "generation",
        "page_id",
        "vm_id",
        "stream_id",
        "media_stream_id",
        "effects",
        "absence",
    ];
    let effects = settlement
        .get("effects")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Browser VZ DidNotAct settlement effects are missing".to_string())?;
    let effect_keys = [
        "session_directory",
        "control_socket",
        "ordinary_stream_bridge",
        "media_stream_bridge",
        "turn_process",
        "supervisor_child",
        "vm",
    ];
    let absence = settlement
        .get("absence")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Browser VZ DidNotAct settlement absence proof is missing".to_string())?;
    let absence_keys = [
        "child_absent",
        "supervisor_child_absent",
        "control_socket_absent",
        "route_absent",
        "turn_listener_absent",
        "turn_relay_ports_absent",
        "ordinary_stream_bridge_absent",
        "media_stream_bridge_absent",
        "session_directory_absent",
        "vm_absent",
    ];
    let message = settlement
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() <= 8_192 && !value.contains('\0'))
        .ok_or_else(|| "Browser VZ launch settlement message is invalid".to_string())?;
    let _ = message;
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || settlement.get("schema").and_then(serde_json::Value::as_str)
            != Some("elastos.browser.vz-launch-settlement/v1")
        || settlement.get("binding_hash") != authority.get("binding_hash")
        || settlement.get("generation") != authority.get("generation")
        || settlement.get("page_id") != authority.get("page_id")
        || settlement.get("vm_id") != authority.get("vm_id")
        || settlement.get("stream_id") != authority.pointer("/egress/stream_id")
        || settlement.get("media_stream_id") != authority.pointer("/media/stream_id")
        || effects.len() != effect_keys.len()
        || effect_keys.iter().any(|key| {
            effects
                .get(*key)
                .and_then(serde_json::Value::as_bool)
                .is_none()
        })
        || absence.len() != absence_keys.len()
        || absence_keys
            .iter()
            .any(|key| absence.get(*key).and_then(serde_json::Value::as_bool) != Some(true))
        || browser_terminal_receipt_contains_transport_secret(settlement)
    {
        return Err("Browser VZ launch settlement binding is not exact".to_string());
    }
    let acted = effect_keys
        .iter()
        .any(|key| effects.get(*key).and_then(serde_json::Value::as_bool) == Some(true));
    let vm_acquired = effects.get("vm").and_then(serde_json::Value::as_bool) == Some(true);
    match settlement.get("state").and_then(serde_json::Value::as_str) {
        Some("did_not_act") if !acted => Ok((acted, vm_acquired)),
        Some("terminal_post_effect_cleanup") if acted => Ok((acted, vm_acquired)),
        _ => Err("Browser VZ launch settlement classification is invalid".to_string()),
    }
}

enum BrowserDispatchedLaunchReconciliation {
    DidNotAct,
    TerminalPostEffectCleanup {
        page_acquired: bool,
        vm_acquired: bool,
    },
    EffectAcquired(Box<BrowserLaunchEffect>),
    CleanupPending,
}

enum BrowserLaunchReconciliationDecision {
    DidNotAct,
    TerminalPostEffectCleanup {
        page_acquired: bool,
        vm_acquired: bool,
    },
    CloseExactEffect(Box<BrowserLaunchEffect>),
    RetainIndeterminate(Option<String>),
}

const BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const BROWSER_LAUNCH_RECONCILIATION_MIN_BACKOFF: Duration = Duration::from_millis(100);
const BROWSER_LAUNCH_RECONCILIATION_MAX_BACKOFF: Duration = Duration::from_secs(30);
const BROWSER_LIFECYCLE_RECONCILIATION_BATCH_LIMIT: usize = 8;

static BROWSER_LIFECYCLE_RECONCILER_WAKES: OnceLock<StdMutex<BTreeMap<String, Weak<Notify>>>> =
    OnceLock::new();

struct BrowserLifecycleReconcilerRegistration {
    scope: String,
    wake: Arc<Notify>,
}

impl Drop for BrowserLifecycleReconcilerRegistration {
    fn drop(&mut self) {
        let Some(registry) = BROWSER_LIFECYCLE_RECONCILER_WAKES.get() else {
            return;
        };
        let Ok(mut registry) = registry.lock() else {
            return;
        };
        let wake = Arc::downgrade(&self.wake);
        if registry
            .get(&self.scope)
            .is_some_and(|registered| Weak::ptr_eq(registered, &wake))
        {
            registry.remove(&self.scope);
        }
    }
}

pub(in crate::api::gateway) struct BrowserLifecycleReconciler {
    shutdown: watch::Sender<bool>,
    wake: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl BrowserLifecycleReconciler {
    pub(in crate::api::gateway) fn cancel(&self) {
        let _ = self.shutdown.send(true);
        self.wake.notify_waiters();
    }

    pub(in crate::api::gateway) async fn join(self) -> Result<(), String> {
        self.task
            .await
            .map_err(|err| format!("Browser launch reconciler task failed: {err}"))
    }
}

pub(in crate::api::gateway) fn start_browser_lifecycle_reconciler(
    state: GatewayState,
) -> Result<BrowserLifecycleReconciler, String> {
    let scope = state.data_dir.to_string_lossy().into_owned();
    let wake = Arc::new(Notify::new());
    let registry = BROWSER_LIFECYCLE_RECONCILER_WAKES.get_or_init(Default::default);
    {
        let mut registry = registry
            .lock()
            .map_err(|_| "Browser launch reconciler registry is unavailable".to_string())?;
        if registry.get(&scope).and_then(Weak::upgrade).is_some() {
            return Err(
                "Browser launch reconciler is already running for this data root".to_string(),
            );
        }
        registry.insert(scope.clone(), Arc::downgrade(&wake));
    }
    let registration = BrowserLifecycleReconcilerRegistration {
        scope,
        wake: wake.clone(),
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task_wake = wake.clone();
    let task = tokio::spawn(async move {
        run_browser_lifecycle_reconciler(state, task_wake, shutdown_rx, registration).await;
    });
    Ok(BrowserLifecycleReconciler {
        shutdown,
        wake,
        task,
    })
}

async fn run_browser_lifecycle_reconciler(
    state: GatewayState,
    wake: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
    _registration: BrowserLifecycleReconcilerRegistration,
) {
    let mut backoff = BROWSER_LAUNCH_RECONCILIATION_MIN_BACKOFF;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let settled = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    release_browser_lifecycle_reconciliation_claims(&state.data_dir).await;
                    break;
                }
                continue;
            }
            settled = retry_pending_browser_lifecycle_obligations(&state) => settled,
        };
        if settled {
            backoff = BROWSER_LAUNCH_RECONCILIATION_MIN_BACKOFF;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = wake.notified() => {
                backoff = BROWSER_LAUNCH_RECONCILIATION_MIN_BACKOFF;
            }
            _ = tokio::time::sleep(backoff) => {
                backoff = backoff
                    .saturating_mul(2)
                    .min(BROWSER_LAUNCH_RECONCILIATION_MAX_BACKOFF);
            }
        }
    }
}

pub(in crate::api::gateway) fn notify_browser_lifecycle_reconciler(data_dir: &FsPath) {
    let Some(registry) = BROWSER_LIFECYCLE_RECONCILER_WAKES.get() else {
        return;
    };
    let scope = data_dir.to_string_lossy().into_owned();
    let wake = registry.lock().ok().and_then(|mut registry| {
        match registry.get(&scope).and_then(Weak::upgrade) {
            Some(wake) => Some(wake),
            None => {
                registry.remove(&scope);
                None
            }
        }
    });
    if let Some(wake) = wake {
        wake.notify_one();
    }
}

fn browser_launch_reconciliation_decision(
    result: Result<BrowserDispatchedLaunchReconciliation, String>,
) -> BrowserLaunchReconciliationDecision {
    match result {
        Ok(BrowserDispatchedLaunchReconciliation::DidNotAct) => {
            BrowserLaunchReconciliationDecision::DidNotAct
        }
        Ok(BrowserDispatchedLaunchReconciliation::TerminalPostEffectCleanup {
            page_acquired,
            vm_acquired,
        }) => BrowserLaunchReconciliationDecision::TerminalPostEffectCleanup {
            page_acquired,
            vm_acquired,
        },
        Ok(BrowserDispatchedLaunchReconciliation::EffectAcquired(effect)) => {
            BrowserLaunchReconciliationDecision::CloseExactEffect(effect)
        }
        Ok(BrowserDispatchedLaunchReconciliation::CleanupPending) => {
            BrowserLaunchReconciliationDecision::RetainIndeterminate(None)
        }
        Err(err) => BrowserLaunchReconciliationDecision::RetainIndeterminate(Some(err)),
    }
}

struct BrowserLaunchReconciliationAttempt<'a> {
    generation: &'a str,
    engine_route_provider: &'a str,
    selected_engine_adapter: Option<&'a str>,
    principal_id: &'a str,
    stream_id: &'a str,
    stream_cleanup: Option<BrowserStreamCleanup>,
    transport_authority: Option<&'a serde_json::Value>,
}

async fn attempt_browser_launch_reconciliation_bounded(
    state: &GatewayState,
    attempt: BrowserLaunchReconciliationAttempt<'_>,
) -> Result<BrowserDispatchedLaunchReconciliation, String> {
    tokio::time::timeout(
        BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
        attempt_browser_launch_reconciliation(state, attempt),
    )
    .await
    .map_err(|_| "browser-engine launch reconciliation timed out".to_string())?
}

async fn reconcile_dispatched_browser_launch_failure(
    state: &GatewayState,
    reservation: &BrowserLaunchReservation,
    principal_id: &str,
    owner_launch_id: &str,
    stream_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
    message: &str,
) -> serde_json::Value {
    mark_browser_launch_failed(reservation, message.to_string()).await;
    let transport_authority = browser_launch_transport_authority(reservation).await;
    let reconciliation = attempt_browser_launch_reconciliation_bounded(
        state,
        BrowserLaunchReconciliationAttempt {
            generation: reservation.generation(),
            engine_route_provider: reservation.engine_route_provider(),
            selected_engine_adapter: reservation.selected_engine_adapter(),
            principal_id,
            stream_id,
            stream_cleanup: stream_cleanup.clone(),
            transport_authority: transport_authority.as_ref(),
        },
    )
    .await;
    match browser_launch_reconciliation_decision(reconciliation) {
        BrowserLaunchReconciliationDecision::DidNotAct => {
            release_browser_open_resources(state, reservation, stream_cleanup, true).await
        }
        BrowserLaunchReconciliationDecision::TerminalPostEffectCleanup {
            page_acquired,
            vm_acquired,
        } => {
            if let Some(authority) = transport_authority.as_ref() {
                if let Err(err) = close_browser_vz_fixed_media_listener(authority).await {
                    tracing::warn!(
                        error = %err,
                        generation = reservation.generation(),
                        "Browser terminal reconciliation could not retire its VZ media listener"
                    );
                    return browser_cleanup_pending_outcome(page_acquired, vm_acquired, true);
                }
            }
            if let Err(err) =
                discard_browser_vz_transport_preparation(&state.data_dir, reservation).await
            {
                tracing::warn!(
                    error = %err,
                    generation = reservation.generation(),
                    "Browser terminal reconciliation could not retire transport authority"
                );
                return browser_cleanup_pending_outcome(page_acquired, vm_acquired, true);
            }
            release_browser_launch(reservation).await;
            if let Err(err) = close_browser_stream_cleanup(state, stream_cleanup).await {
                tracing::warn!(
                    error = %err,
                    "Browser post-dispatch terminal reconciliation could not close its stream"
                );
                browser_cleanup_pending_outcome(page_acquired, vm_acquired, true)
            } else {
                browser_terminal_post_dispatch_outcome(page_acquired, vm_acquired)
            }
        }
        BrowserLaunchReconciliationDecision::CloseExactEffect(effect) => {
            let effect = *effect;
            let page_id = effect.page_id.clone();
            let ownership_persisted = complete_browser_launch(&state.data_dir, reservation, effect)
                .await
                .is_ok();
            reap_browser_open_effect_after_failure(
                state,
                reservation,
                &page_id,
                principal_id,
                owner_launch_id,
                ownership_persisted,
            )
            .await
        }
        BrowserLaunchReconciliationDecision::RetainIndeterminate(reconciliation_error) => {
            if let Some(err) = reconciliation_error {
                tracing::warn!(
                    error = %err,
                    generation = reservation.generation(),
                    stream_id,
                    "Browser dispatched launch reconciliation remains indeterminate"
                );
            }
            if let Err(err) = record_browser_launch_reconciliation_obligation(
                &state.data_dir,
                reservation,
                stream_id,
                stream_cleanup,
            )
            .await
            {
                tracing::error!(
                    error = %err,
                    generation = reservation.generation(),
                    stream_id,
                    "Browser could not durably persist indeterminate launch ownership"
                );
            }
            browser_launch_reconciliation_pending_outcome()
        }
    }
}

async fn attempt_browser_launch_reconciliation(
    state: &GatewayState,
    attempt: BrowserLaunchReconciliationAttempt<'_>,
) -> Result<BrowserDispatchedLaunchReconciliation, String> {
    let BrowserLaunchReconciliationAttempt {
        generation,
        engine_route_provider,
        selected_engine_adapter,
        principal_id,
        stream_id,
        stream_cleanup,
        transport_authority,
    } = attempt;
    let registry = state.provider_registry.as_ref().ok_or_else(|| {
        "browser-engine provider unavailable during launch reconciliation".to_string()
    })?;
    let registration = registry
        .registration_for_uri("elastos://browser-engine/meta/status")
        .await
        .ok_or_else(|| {
            "browser-engine provider status route unavailable during launch reconciliation"
                .to_string()
        })?;
    if registration.provider != engine_route_provider {
        return Err(
            "browser-engine provider binding changed during launch reconciliation".to_string(),
        );
    }
    let call = browser_provider_resource_call(
        "browser-engine",
        "status",
        "elastos://browser-engine/meta/status".to_string(),
        serde_json::json!({
            "principal_id": principal_id,
            "lifecycle_generation": generation,
            "stream_id": stream_id,
            "adapter_id": selected_engine_adapter,
            "transport_authority": transport_authority,
        }),
    )
    .map_err(|(_, message)| message)?;
    let response = browser_provider_resource_response(state, call)
        .await
        .map_err(|(_, message)| message)?;
    if let Some(message) = provider_response_error_message(&response) {
        return Err(message);
    }
    let data = provider_response_data(&response)
        .ok_or_else(|| "browser-engine launch reconciliation response is invalid".to_string())?;
    if data.get("schema").and_then(serde_json::Value::as_str)
        != Some("elastos.browser.engine.launch-reconciliation/v1")
        || data
            .get("lifecycle_generation")
            .and_then(serde_json::Value::as_str)
            != Some(generation)
        || data.get("stream_id").and_then(serde_json::Value::as_str) != Some(stream_id)
        || (transport_authority.is_some() && data.get("transport_authority") != transport_authority)
    {
        return Err("browser-engine launch reconciliation identity is invalid".to_string());
    }
    match data.get("state").and_then(serde_json::Value::as_str) {
        Some("did_not_act")
            if data
                .pointer("/effects/page_acquired")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
                && data
                    .pointer("/effects/vm_acquired")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false) =>
        {
            Ok(BrowserDispatchedLaunchReconciliation::DidNotAct)
        }
        Some("did_not_act") => {
            Err("browser-engine DidNotAct proof omitted exact no-effect evidence".to_string())
        }
        Some("terminal_post_effect_cleanup") => {
            let effects = data.get("effects").ok_or_else(|| {
                "browser-engine terminal reconciliation omitted effects".to_string()
            })?;
            let page_acquired = effects
                .get("page_acquired")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    "browser-engine terminal reconciliation omitted page ownership".to_string()
                })?;
            let vm_acquired = effects
                .get("vm_acquired")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    "browser-engine terminal reconciliation omitted VM ownership".to_string()
                })?;
            if let Some(authority) = transport_authority {
                validate_browser_dispatched_transport_terminal_receipt(
                    data.get("terminal_cleanup_receipt")
                        .unwrap_or(&serde_json::Value::Null),
                    authority,
                    generation,
                    stream_id,
                    selected_engine_adapter,
                    page_acquired,
                    vm_acquired,
                )?;
            }
            Ok(
                BrowserDispatchedLaunchReconciliation::TerminalPostEffectCleanup {
                    page_acquired,
                    vm_acquired,
                },
            )
        }
        Some("effect_acquired") => {
            let effect = data
                .get("effect")
                .filter(|value| value.is_object())
                .ok_or_else(|| {
                    "browser-engine reconciliation omitted its exact effect".to_string()
                })?;
            let page_id = effect
                .get("page_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| is_safe_runtime_id(value))
                .ok_or_else(|| {
                    "browser-engine reconciliation returned an unsafe page_id".to_string()
                })?;
            if transport_authority.is_some_and(|authority| {
                effect.get("transport_authority") != Some(authority)
                    || authority.get("page_id").and_then(serde_json::Value::as_str) != Some(page_id)
                    || validate_browser_vz_transport_effect_receipt(
                        authority,
                        effect
                            .get("transport_receipt")
                            .unwrap_or(&serde_json::Value::Null),
                    )
                    .is_err()
            }) {
                return Err("browser-engine reconciliation transport authority changed".to_string());
            }
            let engine_provider = effect
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .filter(|value| *value == BROWSER_ENGINE_PROVIDER_ID)
                .ok_or_else(|| {
                    "browser-engine reconciliation provider identity changed".to_string()
                })?;
            let engine_protocol_version = effect
                .get("protocol_version")
                .and_then(serde_json::Value::as_str)
                .filter(|value| *value == BROWSER_ENGINE_PROTOCOL_VERSION)
                .ok_or_else(|| "browser-engine reconciliation protocol changed".to_string())?;
            let engine_adapter = effect
                .get("adapter")
                .and_then(serde_json::Value::as_str)
                .filter(|value| is_safe_runtime_id(value))
                .ok_or_else(|| "browser-engine reconciliation adapter is unsafe".to_string())?;
            if selected_engine_adapter.is_some_and(|selected| selected != engine_adapter) {
                return Err(
                    "browser-engine reconciliation selected adapter identity changed".to_string(),
                );
            }
            let engine = effect
                .get("engine")
                .and_then(serde_json::Value::as_str)
                .filter(|value| is_safe_runtime_id(value))
                .ok_or_else(|| "browser-engine reconciliation engine is unsafe".to_string())?;
            if effect.get("stream_id").and_then(serde_json::Value::as_str) != Some(stream_id) {
                return Err("browser-engine reconciliation stream identity changed".to_string());
            }
            let provider_cleanup = browser_provider_cleanup_binding(
                effect,
                generation,
                page_id,
                engine_adapter,
                engine,
                Some(stream_id),
                transport_authority,
            )?;
            Ok(BrowserDispatchedLaunchReconciliation::EffectAcquired(
                Box::new(BrowserLaunchEffect {
                    page_id: page_id.to_string(),
                    engine_provider: engine_provider.to_string(),
                    engine_protocol_version: engine_protocol_version.to_string(),
                    engine_adapter: engine_adapter.to_string(),
                    engine: engine.to_string(),
                    provider_cleanup,
                    browser_page: serde_json::json!({
                        "schema": "elastos.browser.engine.reconciled-effect/v1",
                        "provider": engine_provider,
                        "protocol_version": engine_protocol_version,
                        "page_id": page_id,
                        "adapter": engine_adapter,
                        "engine": engine,
                        "stream_id": stream_id,
                    }),
                    stream_cleanup,
                }),
            ))
        }
        Some("cleanup_pending") => Ok(BrowserDispatchedLaunchReconciliation::CleanupPending),
        _ => Err("browser-engine launch reconciliation state is invalid".to_string()),
    }
}

fn validate_browser_dispatched_transport_terminal_receipt(
    receipt: &serde_json::Value,
    authority: &serde_json::Value,
    generation: &str,
    stream_id: &str,
    selected_engine_adapter: Option<&str>,
    page_acquired: bool,
    vm_acquired: bool,
) -> Result<(), String> {
    if receipt.get("schema").and_then(serde_json::Value::as_str)
        == Some("elastos.browser.vz-launch-settlement/v1")
    {
        let (acted, settlement_vm_acquired) =
            validate_browser_vz_launch_settlement_binding(receipt, authority)?;
        if selected_engine_adapter.is_none()
            || receipt.get("state").and_then(serde_json::Value::as_str)
                != Some("terminal_post_effect_cleanup")
            || !acted
            || page_acquired
            || settlement_vm_acquired != vm_acquired
            || receipt
                .get("generation")
                .and_then(serde_json::Value::as_str)
                != Some(generation)
            || receipt.get("stream_id").and_then(serde_json::Value::as_str) != Some(stream_id)
        {
            return Err("browser-engine terminal VZ launch settlement is not exact".to_string());
        }
        return Ok(());
    }
    let binding = receipt
        .get("binding")
        .ok_or_else(|| "browser-engine terminal transport binding is missing".to_string())?;
    let transport_receipt = binding
        .get("transport_receipt")
        .ok_or_else(|| "browser-engine terminal transport effect receipt is missing".to_string())?;
    validate_browser_vz_transport_effect_receipt(authority, transport_receipt)?;
    let terminal_effects = [
        "page_absent",
        "child_absent",
        "vm_absent",
        "route_absent",
        "socket_absent",
        "transport_session_absent",
        "turn_process_absent",
        "turn_listener_absent",
        "turn_relay_ports_absent",
        "ordinary_vsock_bridge_absent",
        "media_vsock_bridge_absent",
        "bootstrap_vsock_bridge_absent",
        "hibernation_state_absent",
    ];
    if receipt.get("schema").and_then(serde_json::Value::as_str)
        != Some(BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA)
        || receipt.get("page_id") != authority.get("page_id")
        || receipt
            .get("generation")
            .and_then(serde_json::Value::as_str)
            != Some(generation)
        || receipt.get("terminal").and_then(serde_json::Value::as_bool) != Some(true)
        || binding.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA)
        || binding.get("page_id") != authority.get("page_id")
        || binding
            .get("generation")
            .and_then(serde_json::Value::as_str)
            != Some(generation)
        || binding.get("stream_id").and_then(serde_json::Value::as_str) != Some(stream_id)
        || binding.get("principal_id") != authority.get("principal_id")
        || binding.get("adapter").and_then(serde_json::Value::as_str) != selected_engine_adapter
        || binding.get("engine").and_then(serde_json::Value::as_str) != Some("chromium_microvm")
        || binding
            .get("display_mode")
            .and_then(serde_json::Value::as_str)
            != Some("webrtc_remote_display")
        || binding
            .get("guarantee_level")
            .and_then(serde_json::Value::as_str)
            != Some("mechanism_microvm")
        || binding
            .get("isolated_session")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || binding.get("transport_authority") != Some(authority)
        || terminal_effects.iter().any(|effect| {
            receipt
                .pointer(&format!("/effects/{effect}"))
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        })
        || browser_terminal_receipt_contains_transport_secret(receipt)
    {
        return Err("browser-engine terminal transport receipt is not exact".to_string());
    }
    Ok(())
}

fn browser_terminal_receipt_contains_transport_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(browser_terminal_receipt_contains_transport_secret),
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "credential" | "auth_secret" | "transport_secret"
            ) || browser_terminal_receipt_contains_transport_secret(value)
        }),
        _ => false,
    }
}

fn browser_stream_nonce(request_id: &str) -> String {
    let digest = Sha256::digest(request_id.as_bytes());
    format!("open-{}", hex::encode(&digest[..8]))
}

fn browser_remote_exit_id(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
    {
        return Err("remote_exit_id must be a safe identifier up to 128 bytes".to_string());
    }
    Ok(Some(value))
}

fn browser_engine_adapter_id(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if value.len() > 128 || !is_safe_runtime_id(&value) {
        return Err(
            "adapter_id must be a safe Browser Engine identifier up to 128 bytes".to_string(),
        );
    }
    Ok(Some(value))
}

fn browser_provider_cleanup_binding(
    engine_page: &serde_json::Value,
    generation: &str,
    page_id: &str,
    adapter: &str,
    engine: &str,
    stream_id: Option<&str>,
    transport_authority: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let binding = engine_page
        .get("runtime_cleanup")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            "browser-engine provider omitted its Runtime-only cleanup binding".to_string()
        })?;
    let expected_stream_id = stream_id.unwrap_or_default();
    if binding.get("schema").and_then(|value| value.as_str())
        != Some(BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA)
        || binding.get("page_id").and_then(|value| value.as_str()) != Some(page_id)
        || binding.get("generation").and_then(|value| value.as_str()) != Some(generation)
        || binding.get("adapter").and_then(|value| value.as_str()) != Some(adapter)
        || binding.get("engine").and_then(|value| value.as_str()) != Some(engine)
        || binding.get("stream_id").and_then(|value| value.as_str()) != Some(expected_stream_id)
        || transport_authority.is_some()
            && binding.get("transport_authority") != transport_authority
        || transport_authority.is_none() && binding.get("transport_authority").is_some()
        || !browser_provider_cleanup_value_is_safe(&binding, 0)
        || serde_json::to_vec(&binding).map_or(true, |bytes| bytes.len() > 16 * 1024)
    {
        return Err(
            "browser-engine provider returned an invalid Runtime-only cleanup binding".to_string(),
        );
    }
    if let Some(authority) = transport_authority {
        validate_browser_vz_transport_effect_receipt(
            authority,
            binding
                .get("transport_receipt")
                .unwrap_or(&serde_json::Value::Null),
        )?;
    } else if binding.get("transport_receipt").is_some() {
        return Err(
            "browser-engine provider returned an unexpected VZ transport receipt".to_string(),
        );
    }
    Ok(binding)
}

fn browser_provider_cleanup_value_is_safe(value: &serde_json::Value, depth: usize) -> bool {
    if depth > 6 {
        return false;
    }
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
        serde_json::Value::String(text) => {
            text.len() <= 1024
                && !text
                    .chars()
                    .any(|character| character == '\0' || character.is_control())
        }
        serde_json::Value::Array(values) => {
            values.len() <= 32
                && values
                    .iter()
                    .all(|value| browser_provider_cleanup_value_is_safe(value, depth + 1))
        }
        serde_json::Value::Object(values) => {
            values.len() <= 32
                && values.iter().all(|(key, value)| {
                    key.len() <= 128
                        && is_safe_runtime_id(key)
                        && browser_provider_cleanup_value_is_safe(value, depth + 1)
                })
        }
    }
}

async fn cleanup_stale_browser_pages(state: &GatewayState) {
    retry_pending_browser_lifecycle_obligations(state).await;
    for page in take_stale_browser_pages(&state.data_dir).await {
        close_browser_page_record(state, page).await;
    }
}

async fn retry_pending_browser_lifecycle_obligations(state: &GatewayState) -> bool {
    let launch_settled = retry_pending_browser_launch_reconciliations(state).await;
    let engine_settled = retry_pending_browser_engine_cleanups(state).await;
    let stream_settled = retry_pending_browser_stream_cleanups(state).await;
    launch_settled || engine_settled || stream_settled
}

async fn retry_pending_browser_launch_reconciliations(state: &GatewayState) -> bool {
    let mut settled = false;
    for reconciliation in claim_pending_browser_launch_reconciliations(
        &state.data_dir,
        BROWSER_LIFECYCLE_RECONCILIATION_BATCH_LIMIT,
    )
    .await
    {
        let result = if reconciliation.was_dispatched() {
            attempt_browser_launch_reconciliation_bounded(
                state,
                BrowserLaunchReconciliationAttempt {
                    generation: &reconciliation.generation,
                    engine_route_provider: &reconciliation.engine_route_provider,
                    selected_engine_adapter: reconciliation.selected_engine_adapter.as_deref(),
                    principal_id: &reconciliation.principal_id,
                    stream_id: &reconciliation.stream_id,
                    stream_cleanup: reconciliation.stream_cleanup.clone(),
                    transport_authority: reconciliation.transport_authority(),
                },
            )
            .await
        } else {
            Ok(BrowserDispatchedLaunchReconciliation::DidNotAct)
        };
        match browser_launch_reconciliation_decision(result) {
            BrowserLaunchReconciliationDecision::DidNotAct
            | BrowserLaunchReconciliationDecision::TerminalPostEffectCleanup { .. } => {
                if let Some(authority) = reconciliation.transport_authority() {
                    if let Err(err) = close_browser_vz_fixed_media_listener(authority).await {
                        tracing::warn!(
                            error = %err,
                            generation = %reconciliation.generation,
                            "Browser launch reconciliation could not close its VZ media listener"
                        );
                        release_browser_launch_reconciliation_claim(
                            &state.data_dir,
                            &reconciliation,
                        )
                        .await;
                        continue;
                    }
                }
                let stream_result = tokio::time::timeout(
                    BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
                    close_browser_stream_cleanup(state, reconciliation.stream_cleanup.clone()),
                )
                .await
                .map_err(|_| "Browser stream cleanup timed out".to_string())
                .and_then(|result| result);
                if let Err(err) = stream_result {
                    tracing::warn!(
                        error = %err,
                        generation = %reconciliation.generation,
                        "Browser launch reconciliation terminal proof could not close its stream"
                    );
                    release_browser_launch_reconciliation_claim(&state.data_dir, &reconciliation)
                        .await;
                    continue;
                }
                if let Err(err) = forget_browser_launch_reconciliation_obligation(
                    &state.data_dir,
                    &reconciliation,
                )
                .await
                {
                    tracing::warn!(
                        error = %err,
                        generation = %reconciliation.generation,
                        "Browser launch reconciliation terminal release could not be committed"
                    );
                    release_browser_launch_reconciliation_claim(&state.data_dir, &reconciliation)
                        .await;
                } else {
                    settled = true;
                }
            }
            BrowserLaunchReconciliationDecision::CloseExactEffect(effect) => {
                let effect = *effect;
                let cleanup = BrowserEngineCleanup {
                    cleanup_id: reconciliation.cleanup_id.clone(),
                    page_id: effect.page_id,
                    principal_id: reconciliation.principal_id.clone(),
                    owner_launch_id: reconciliation.owner_launch_id.clone(),
                    browser_instance: reconciliation.browser_instance.clone(),
                    generation: reconciliation.generation.clone(),
                    engine_route_provider: reconciliation.engine_route_provider.clone(),
                    engine_provider: effect.engine_provider,
                    engine_protocol_version: effect.engine_protocol_version,
                    engine_adapter: effect.engine_adapter,
                    engine: effect.engine,
                    stream_id: reconciliation.stream_id.clone(),
                    transport_authority: reconciliation.transport_authority().cloned(),
                    provider_cleanup: effect.provider_cleanup,
                };
                if let Err(err) = promote_browser_launch_reconciliation_effect(
                    &state.data_dir,
                    &reconciliation,
                    cleanup.clone(),
                )
                .await
                {
                    tracing::warn!(
                        error = %err,
                        generation = %reconciliation.generation,
                        "Browser could not promote reconciled launch ownership into exact cleanup"
                    );
                    release_browser_launch_reconciliation_claim(&state.data_dir, &reconciliation)
                        .await;
                    continue;
                }
                let engine_result = tokio::time::timeout(
                    BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
                    attempt_browser_engine_cleanup(state, &cleanup),
                )
                .await
                .map_err(|_| "Browser engine cleanup timed out".to_string())
                .and_then(|result| result);
                let stream_result = tokio::time::timeout(
                    BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
                    close_browser_stream_cleanup(state, reconciliation.stream_cleanup.clone()),
                )
                .await
                .map_err(|_| "Browser stream cleanup timed out".to_string())
                .and_then(|result| result);
                if engine_result.is_ok() && stream_result.is_ok() {
                    if let Err(err) =
                        forget_browser_engine_cleanup_obligation(&state.data_dir, &cleanup).await
                    {
                        release_browser_engine_cleanup_claim(&state.data_dir, &cleanup).await;
                        tracing::warn!(
                            error = %err,
                            page_id = %cleanup.page_id,
                            "Browser reconciled cleanup terminal release could not be committed"
                        );
                    } else {
                        settled = true;
                    }
                } else {
                    release_browser_engine_cleanup_claim(&state.data_dir, &cleanup).await;
                    tracing::warn!(
                        page_id = %cleanup.page_id,
                        engine_error = ?engine_result.err(),
                        stream_error = ?stream_result.err(),
                        "Browser reconciled cleanup remains pending"
                    );
                }
            }
            BrowserLaunchReconciliationDecision::RetainIndeterminate(error) => {
                if let Some(error) = error {
                    tracing::warn!(
                        error,
                        generation = %reconciliation.generation,
                        "Browser launch reconciliation retry remains indeterminate"
                    );
                }
                release_browser_launch_reconciliation_claim(&state.data_dir, &reconciliation).await;
            }
        }
    }
    settled
}

async fn retry_pending_browser_engine_cleanups(state: &GatewayState) -> bool {
    let mut settled = false;
    for cleanup in claim_pending_browser_engine_cleanups(
        &state.data_dir,
        BROWSER_LIFECYCLE_RECONCILIATION_BATCH_LIMIT,
    )
    .await
    {
        let engine_result = tokio::time::timeout(
            BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
            attempt_browser_engine_cleanup(state, &cleanup),
        )
        .await
        .map_err(|_| "Browser engine cleanup timed out".to_string())
        .and_then(|result| result);
        let stream_cleanup =
            browser_pending_stream_cleanup_for_engine(&state.data_dir, &cleanup).await;
        let stream_result = tokio::time::timeout(
            BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
            close_browser_stream_cleanup(state, stream_cleanup),
        )
        .await
        .map_err(|_| "Browser stream cleanup timed out".to_string())
        .and_then(|result| result);
        match (engine_result, stream_result) {
            (Ok(_), Ok(())) => {
                if let Err(err) =
                    forget_browser_engine_cleanup_obligation(&state.data_dir, &cleanup).await
                {
                    release_browser_engine_cleanup_claim(&state.data_dir, &cleanup).await;
                    tracing::warn!(
                        page_id = %cleanup.page_id,
                        error = %err,
                        "Browser terminal cleanup could not commit durable owner release"
                    );
                } else {
                    settled = true;
                }
            }
            (engine_result, stream_result) => {
                release_browser_engine_cleanup_claim(&state.data_dir, &cleanup).await;
                tracing::warn!(
                    page_id = %cleanup.page_id,
                    engine_error = ?engine_result.err(),
                    stream_error = ?stream_result.err(),
                    "Browser pending engine cleanup retry failed"
                );
            }
        }
    }
    settled
}

async fn retry_pending_browser_stream_cleanups(state: &GatewayState) -> bool {
    let mut settled = false;
    for cleanup in claim_pending_browser_stream_cleanups(
        &state.data_dir,
        BROWSER_LIFECYCLE_RECONCILIATION_BATCH_LIMIT,
    )
    .await
    {
        let result = tokio::time::timeout(
            BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT,
            close_browser_stream_cleanup(state, Some(cleanup)),
        )
        .await
        .map_err(|_| "Browser stream cleanup timed out".to_string())
        .and_then(|result| result);
        if let Err(err) = result {
            tracing::warn!(
                error = %err,
                "Browser pending stream cleanup retry failed"
            );
        } else {
            settled = true;
        }
    }
    settled
}

async fn close_browser_page_record(state: &GatewayState, page: BrowserPageCleanup) {
    let engine_cleanup = page.engine_cleanup;
    forget_browser_open_job_for_owner(
        &state.data_dir,
        &engine_cleanup.principal_id,
        &engine_cleanup.owner_launch_id,
    )
    .await;
    if let Err(err) =
        record_browser_engine_cleanup_obligation(&state.data_dir, engine_cleanup.clone()).await
    {
        tracing::warn!(
            page_id = %engine_cleanup.page_id,
            error = %err,
            "Browser stale page cleanup obligation could not be persisted; attempting exact deterministic cleanup"
        );
    }
    let engine_result = attempt_browser_engine_cleanup(state, &engine_cleanup).await;
    let stream_result = close_browser_stream_cleanup(state, page.stream_cleanup).await;
    match (&engine_result, &stream_result) {
        (Ok(_), Ok(())) => {
            if let Err(err) =
                forget_browser_engine_cleanup_obligation(&state.data_dir, &engine_cleanup).await
            {
                release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
                tracing::warn!(
                    page_id = %engine_cleanup.page_id,
                    error = %err,
                    "Browser stale page terminal release could not be committed"
                );
            }
        }
        _ => {
            release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
            tracing::warn!(
                page_id = %engine_cleanup.page_id,
                engine_error = ?engine_result.as_ref().err(),
                stream_error = ?stream_result.as_ref().err(),
                "Browser stale page engine cleanup failed"
            );
        }
    }
}

async fn attempt_browser_engine_cleanup(
    state: &GatewayState,
    cleanup: &BrowserEngineCleanup,
) -> Result<serde_json::Value, String> {
    require_browser_engine_provider_binding(state, cleanup).await?;
    let call = browser_provider_resource_call(
        "browser-engine",
        "close_page",
        "elastos://browser-engine/close_page".to_string(),
        serde_json::json!({
            "page_id": cleanup.page_id,
            "principal_id": cleanup.principal_id,
            "runtime_cleanup": cleanup.provider_cleanup,
        }),
    )
    .map_err(|(_, message)| message)?;
    let response = browser_provider_resource_response(state, call)
        .await
        .map_err(|(_, message)| message)?;
    if let Some(message) = provider_response_error_message(&response) {
        return Err(message);
    }
    let data = provider_response_data(&response).ok_or_else(|| {
        "browser-engine provider returned an invalid close-page response".to_string()
    })?;
    let receipt = browser_terminal_close_receipt(cleanup, data)?;
    if let Some(authority) = cleanup.transport_authority.as_ref() {
        close_browser_vz_fixed_media_listener(authority).await?;
    }
    Ok(receipt)
}

async fn require_browser_engine_provider_binding(
    state: &GatewayState,
    cleanup: &BrowserEngineCleanup,
) -> Result<(), String> {
    let registry = state.provider_registry.as_ref().ok_or_else(|| {
        "browser-engine provider unavailable while cleanup is pending".to_string()
    })?;
    let registration = registry
        .registration_for_uri("elastos://browser-engine/close_page")
        .await
        .ok_or_else(|| {
            "browser-engine provider route unavailable while cleanup is pending".to_string()
        })?;
    if registration.provider != cleanup.engine_route_provider {
        return Err(
            "browser-engine provider binding changed while exact cleanup is pending".to_string(),
        );
    }
    let status_call = browser_provider_resource_call(
        "browser-engine",
        "status",
        "elastos://browser-engine/meta/status".to_string(),
        serde_json::json!({
            "principal_id": cleanup.principal_id,
        }),
    )
    .map_err(|(_, message)| message)?;
    let status_response = browser_provider_resource_response(state, status_call)
        .await
        .map_err(|(_, message)| message)?;
    if let Some(message) = provider_response_error_message(&status_response) {
        return Err(message);
    }
    let status = provider_response_data(&status_response)
        .ok_or_else(|| "browser-engine provider returned an invalid status binding".to_string())?;
    if status.get("provider").and_then(|value| value.as_str())
        != Some(cleanup.engine_provider.as_str())
        || status
            .get("protocol_version")
            .and_then(|value| value.as_str())
            != Some(cleanup.engine_protocol_version.as_str())
    {
        return Err(
            "browser-engine provider identity or protocol changed while exact cleanup is pending"
                .to_string(),
        );
    }
    Ok(())
}

async fn release_browser_open_resources(
    state: &GatewayState,
    reservation: &BrowserLaunchReservation,
    stream_cleanup: Option<BrowserStreamCleanup>,
    provider_dispatched: bool,
) -> serde_json::Value {
    if let Some(authority) = browser_launch_transport_authority(reservation).await {
        if let Err(err) = close_browser_vz_fixed_media_listener(&authority).await {
            tracing::warn!(
                error = %err,
                generation = reservation.generation(),
                "Browser VZ prepared media listener could not be retired"
            );
            return browser_cleanup_pending_outcome(false, false, true);
        }
    }
    if let Err(err) = discard_browser_vz_transport_preparation(&state.data_dir, reservation).await {
        tracing::warn!(
            error = %err,
            generation = reservation.generation(),
            "Browser VZ prepared transport authority could not be retired"
        );
        return browser_cleanup_pending_outcome(false, false, true);
    }
    release_browser_launch(reservation).await;
    if let Err(err) = close_browser_stream_cleanup(state, stream_cleanup).await {
        tracing::warn!(
            error = %err,
            "Browser open resource cleanup failed"
        );
        return browser_cleanup_pending_outcome(false, false, true);
    }
    if provider_dispatched {
        browser_terminal_post_dispatch_outcome(false, false)
    } else {
        browser_terminal_pre_effect_outcome()
    }
}

async fn reap_browser_open_effect_after_failure(
    state: &GatewayState,
    reservation: &BrowserLaunchReservation,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
    ownership_persisted: bool,
) -> serde_json::Value {
    let page_cleanup = match browser_page_cleanup_for_principal(
        &state.data_dir,
        page_id,
        principal_id,
        owner_launch_id,
        reservation.cleanup_id(),
    )
    .await
    {
        Ok(Some(cleanup)) => cleanup,
        Ok(None) => {
            tracing::warn!(
                page_id,
                "Browser open failure could not recover its exact cleanup binding"
            );
            return browser_cleanup_pending_outcome(true, true, true);
        }
        Err(err) => {
            tracing::warn!(
                page_id,
                error = %err,
                "Browser open failure could not load its exact cleanup binding"
            );
            return browser_cleanup_pending_outcome(true, true, true);
        }
    };
    let engine_cleanup = page_cleanup.engine_cleanup;
    let obligation_persisted =
        match record_browser_engine_cleanup_obligation(&state.data_dir, engine_cleanup.clone())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(
                    page_id,
                    error = %err,
                    "Browser open failure cleanup could not persist its obligation"
                );
                false
            }
        };
    let engine_result = attempt_browser_engine_cleanup(state, &engine_cleanup).await;
    if !ownership_persisted && !obligation_persisted && engine_result.is_err() {
        tracing::warn!(
            page_id,
            error = ?engine_result.as_ref().err(),
            "Browser open failure remains actively owned because neither terminal cleanup nor durable transfer succeeded"
        );
        return browser_cleanup_pending_outcome(true, true, true);
    }
    let stream_result = release_browser_page_and_stream_for_principal(
        state,
        page_id,
        principal_id,
        owner_launch_id,
    )
    .await;
    if engine_result.is_ok() && stream_result.is_ok() {
        if let Err(err) =
            forget_browser_engine_cleanup_obligation(&state.data_dir, &engine_cleanup).await
        {
            release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
            tracing::warn!(
                page_id,
                error = %err,
                "Browser open failure terminal cleanup could not release its durable obligation"
            );
            return browser_cleanup_pending_outcome(false, false, false);
        }
        browser_terminal_post_effect_outcome()
    } else {
        release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
        tracing::warn!(
            page_id,
            engine_error = ?engine_result.as_ref().err(),
            stream_error = ?stream_result.as_ref().err(),
            "Browser open failure transferred exact cleanup into a retryable obligation"
        );
        browser_cleanup_pending_outcome(
            engine_result.is_err(),
            engine_result.is_err(),
            stream_result.is_err(),
        )
    }
}

async fn release_browser_page_and_stream_for_principal(
    state: &GatewayState,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> Result<(), String> {
    let stream_cleanup = browser_page_stream_cleanup_for_principal(
        &state.data_dir,
        page_id,
        principal_id,
        owner_launch_id,
    )
    .await;
    let cleanup_result = close_browser_stream_cleanup(state, stream_cleanup).await;
    let _ =
        release_browser_page_for_principal(&state.data_dir, page_id, principal_id, owner_launch_id)
            .await;
    forget_browser_open_job_for_owner(&state.data_dir, principal_id, owner_launch_id).await;
    cleanup_result
}

async fn close_browser_stream_cleanup(
    state: &GatewayState,
    stream_cleanup: Option<BrowserStreamCleanup>,
) -> Result<(), String> {
    let Some(cleanup) = stream_cleanup else {
        return Ok(());
    };
    let cleanup_for_failure = cleanup.clone();
    let listener_result =
        close_browser_runtime_stream_listener(&state.data_dir, &cleanup.stream_id).await;
    let provider_result = match state.provider_registry.as_ref() {
        Some(registry) => match browser_provider_resource_call(
            "exit",
            "close_stream",
            "elastos://exit/close_stream".to_string(),
            serde_json::json!({
                "stream_id": cleanup.stream_id,
                "principal_id": cleanup.principal_id,
            }),
        ) {
            Ok(call) => match registry.send_raw(call.scheme, &call.request).await {
                Ok(response)
                    if response.get("status").and_then(|value| value.as_str()) == Some("ok") =>
                {
                    Ok(())
                }
                Ok(response) => Err(provider_response_error_message(&response).unwrap_or_else(
                    || "exit provider returned an invalid close_stream response".into(),
                )),
                Err(err) => Err(format!("exit provider close_stream failed: {err}")),
            },
            Err((_status, message)) => Err(message),
        },
        None => Err("exit provider unavailable while closing Browser stream".to_string()),
    };
    match (listener_result, provider_result) {
        (Ok(()), Ok(())) => {
            forget_browser_stream_cleanup_failure(&state.data_dir, &cleanup_for_failure).await
        }
        (listener_result, provider_result) => {
            record_browser_stream_cleanup_failure(&state.data_dir, cleanup_for_failure).await?;
            Err(format!(
                "Browser stream cleanup remains pending: listener={}; provider={}",
                listener_result
                    .err()
                    .unwrap_or_else(|| "terminal".to_string()),
                provider_result
                    .err()
                    .unwrap_or_else(|| "terminal".to_string()),
            ))
        }
    }
}

pub(super) async fn browser_app_page_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    let call = match browser_provider_resource_call(
        "browser-engine",
        "page_status",
        "elastos://browser-engine/page/status".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "principal_id": principal_id.clone(),
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => return (status, message).into_response(),
    };
    let response = match browser_provider_resource_response(&state, call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id)
                .await;
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid page-status response"),
        ),
    }
}

pub(super) async fn browser_app_page_diagnostics(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    let call = match browser_provider_resource_call(
        "browser-engine",
        "diagnostics",
        "elastos://browser-engine/page/diagnostics".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "principal_id": principal_id.clone(),
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => return (status, message).into_response(),
    };
    let response = match browser_provider_resource_response(&state, call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id)
                .await;
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid diagnostics response"),
        ),
    }
}

pub(super) async fn browser_app_page_heartbeat(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    Json(serde_json::json!({
        "schema": "elastos.browser.page-heartbeat/v1",
        "page_id": page_id,
        "principal_id": principal_id,
        "ok": true,
    }))
    .into_response()
}

pub(super) async fn browser_app_page_input(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    Json(input): Json<BrowserInputRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    let event = input.event;
    let browser_command = event
        .get("type")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value == "browser_command");
    let command = event
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let navigation_url = event.get("url").and_then(|value| value.as_str());
    if browser_command && matches!(command, "navigate" | "back" | "forward" | "reload") {
        let _ = mark_browser_page_navigating(
            &state.data_dir,
            &page_id,
            &principal_id,
            &owner_launch_id,
            navigation_url,
        )
        .await;
    }
    let call = match browser_provider_resource_call(
        "browser-engine",
        "input",
        "elastos://browser-engine/page/input".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "event": event,
            "principal_id": principal_id.clone(),
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => return (status, message).into_response(),
    };
    let response = match browser_provider_resource_response(&state, call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            let _ = mark_browser_page_failed(
                &state.data_dir,
                &page_id,
                &principal_id,
                &owner_launch_id,
                message.clone(),
            )
            .await;
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        let _ = mark_browser_page_failed(
            &state.data_dir,
            &page_id,
            &principal_id,
            &owner_launch_id,
            message.clone(),
        )
        .await;
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            let _ = mark_browser_page_active(
                &state.data_dir,
                &page_id,
                &principal_id,
                &owner_launch_id,
            )
            .await;
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id)
                .await;
            Json(data).into_response()
        }
        None => {
            let _ = mark_browser_page_failed(
                &state.data_dir,
                &page_id,
                &principal_id,
                &owner_launch_id,
                "browser-engine provider returned an invalid input response",
            )
            .await;
            gateway_provider_error_response(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider returned an invalid input response"),
            )
        }
    }
}

pub(super) async fn browser_app_page_close(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    close_request: Option<Json<BrowserPageCloseRequest>>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    let cleanup_id = match close_request {
        Some(Json(request)) => {
            if request.schema != "elastos.browser.close-request/v2"
                || request.cleanup_id.len() > 128
                || !is_safe_runtime_id(&request.cleanup_id)
            {
                return (StatusCode::BAD_REQUEST, "invalid Browser cleanup handle").into_response();
            }
            request.cleanup_id
        }
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Browser close requires an opaque Runtime cleanup handle",
            )
                .into_response()
        }
    };
    let page_cleanup = match browser_page_cleanup_for_principal(
        &state.data_dir,
        &page_id,
        &principal_id,
        &owner_launch_id,
        &cleanup_id,
    )
    .await
    {
        Ok(Some(cleanup)) => cleanup,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "browser session is not active").into_response()
        }
        Err(message) => return (StatusCode::SERVICE_UNAVAILABLE, message).into_response(),
    };
    let cleanup_owner_launch_id = page_cleanup.engine_cleanup.owner_launch_id.clone();
    if page_cleanup.active_session {
        let _ = mark_browser_page_retiring(
            &state.data_dir,
            &page_id,
            &principal_id,
            &cleanup_owner_launch_id,
        )
        .await;
    }
    let engine_cleanup = page_cleanup.engine_cleanup.clone();
    if let Err(message) =
        record_browser_engine_cleanup_obligation(&state.data_dir, engine_cleanup.clone()).await
    {
        return (StatusCode::SERVICE_UNAVAILABLE, message).into_response();
    }
    let engine_result = attempt_browser_engine_cleanup(&state, &engine_cleanup).await;
    let stream_result = if page_cleanup.active_session {
        release_browser_page_and_stream_for_principal(
            &state,
            &page_id,
            &principal_id,
            &cleanup_owner_launch_id,
        )
        .await
    } else {
        close_browser_stream_cleanup(&state, page_cleanup.stream_cleanup).await
    };
    let terminal = if engine_result.is_ok() && stream_result.is_ok() {
        forget_browser_engine_cleanup_obligation(&state.data_dir, &engine_cleanup).await
    } else {
        release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
        Ok(())
    };
    if let Err(message) = terminal {
        release_browser_engine_cleanup_claim(&state.data_dir, &engine_cleanup).await;
        return (StatusCode::SERVICE_UNAVAILABLE, message).into_response();
    }
    if let Err(cleanup_err) = stream_result {
        return (StatusCode::SERVICE_UNAVAILABLE, cleanup_err).into_response();
    }
    match engine_result {
        Ok(receipt) => Json(browser_public_terminal_close_receipt(
            &engine_cleanup,
            &receipt,
        ))
        .into_response(),
        Err(message) => gateway_provider_error_response("browser-engine", anyhow::anyhow!(message)),
    }
}

fn browser_public_terminal_close_receipt(
    cleanup: &BrowserEngineCleanup,
    receipt: &serde_json::Value,
) -> serde_json::Value {
    let mut public_receipt = serde_json::json!({
        "schema": "elastos.browser.close-result/v1",
        "page_id": cleanup.page_id,
        "closed": true,
        "cleanup_id": cleanup.cleanup_id,
        "terminal_effects": {
            "page_absent": receipt["effects"]["page_absent"],
            "child_absent": receipt["effects"]["child_absent"],
            "vm_absent": receipt["effects"]["vm_absent"],
            "route_absent": receipt["effects"]["route_absent"],
            "socket_absent": receipt["effects"]["socket_absent"]
        },
        "cleanup": {
            "schema": "elastos.browser.runtime-session-cleanup/v1",
            "ok": true,
            "action": "released_exact_runtime_browser_ownership"
        }
    });
    if let (Some(authority), Some(transport_receipt)) = (
        cleanup.provider_cleanup.get("transport_authority"),
        cleanup.provider_cleanup.get("transport_receipt"),
    ) {
        if let Ok(proof) = browser_vz_public_transport_proof(authority, transport_receipt) {
            public_receipt["transport_proof"] = proof;
            for effect in [
                "transport_session_absent",
                "turn_process_absent",
                "turn_listener_absent",
                "turn_relay_ports_absent",
                "ordinary_vsock_bridge_absent",
                "media_vsock_bridge_absent",
                "bootstrap_vsock_bridge_absent",
                "hibernation_state_absent",
            ] {
                public_receipt["terminal_effects"][effect] = receipt["effects"][effect].clone();
            }
        }
    }
    public_receipt
}

pub(super) fn browser_terminal_close_receipt(
    cleanup: &BrowserEngineCleanup,
    data: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let expected_binding = &cleanup.provider_cleanup;
    let transport_effects_terminal = expected_binding.get("transport_authority").is_none()
        || [
            "transport_session_absent",
            "turn_process_absent",
            "turn_listener_absent",
            "turn_relay_ports_absent",
            "ordinary_vsock_bridge_absent",
            "media_vsock_bridge_absent",
            "bootstrap_vsock_bridge_absent",
            "hibernation_state_absent",
        ]
        .iter()
        .all(|effect| {
            data.pointer(&format!("/effects/{effect}"))
                .and_then(|value| value.as_bool())
                == Some(true)
        });
    let terminal = data.get("schema").and_then(|value| value.as_str())
        == Some(BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA)
        && data.get("page_id").and_then(|value| value.as_str()) == Some(cleanup.page_id.as_str())
        && data.get("generation").and_then(|value| value.as_str())
            == Some(cleanup.generation.as_str())
        && data.get("binding") == Some(expected_binding)
        && data.get("terminal").and_then(|value| value.as_bool()) == Some(true)
        && data
            .pointer("/effects/page_absent")
            .and_then(|value| value.as_bool())
            == Some(true)
        && data
            .pointer("/effects/child_absent")
            .and_then(|value| value.as_bool())
            == Some(true)
        && data
            .pointer("/effects/vm_absent")
            .and_then(|value| value.as_bool())
            == Some(true)
        && data
            .pointer("/effects/route_absent")
            .and_then(|value| value.as_bool())
            == Some(true)
        && data
            .pointer("/effects/socket_absent")
            .and_then(|value| value.as_bool())
            == Some(true)
        && transport_effects_terminal;
    if !terminal {
        return Err(
            "browser-engine provider did not prove exact terminal cleanup ownership".to_string(),
        );
    }
    Ok(data)
}

pub(super) async fn browser_app_page_webrtc(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    Json(input): Json<BrowserWebrtcSignalRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let context = authority.home_launch_context();
    let owner_launch_id = authority.verified_context().launch_id().to_string();
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    let signal_type = input.signal_type.clone();
    let channel = match input.channel.as_deref() {
        Some("audio") => "audio",
        Some("video") | None => "video",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Browser WebRTC channel is unsupported",
            )
                .into_response()
        }
    }
    .to_string();
    let mut signal = match browser_webrtc_signal_value(input) {
        Ok(signal) => signal,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    let transport_authority = match touch_browser_page_transport_authority(
        &state.data_dir,
        &page_id,
        &principal_id,
        &owner_launch_id,
    )
    .await
    {
        Some(authority) => authority,
        None => {
            return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
        }
    };
    if let Some(authority) = transport_authority.as_ref() {
        if let Err(message) = normalize_browser_vz_webrtc_signal(authority, &mut signal) {
            return gateway_provider_error_response("browser", anyhow::anyhow!(message));
        }
    }
    let call = match browser_provider_resource_call(
        "browser-engine",
        "webrtc_signal",
        "elastos://browser-engine/page/webrtc_signal".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "signal": signal,
            "channel": channel,
            "principal_id": principal_id.clone(),
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => return (status, message).into_response(),
    };
    let response = match browser_provider_resource_response(&state, call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    let data = match provider_response_data(&response) {
        Some(data) => data,
        None => {
            return gateway_provider_error_response(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider returned an invalid WebRTC response"),
            )
        }
    };
    match validate_browser_webrtc_response(&signal_type, data) {
        Ok(data) => {
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id, &owner_launch_id)
                .await;
            Json(data).into_response()
        }
        Err(err) => gateway_provider_error_response("browser-engine", err),
    }
}

#[cfg(test)]
mod browser_open_outcome_tests {
    use super::*;

    #[test]
    fn failed_open_serializes_cleanup_pending_separately_from_pre_effect_failure() {
        let pending = BrowserOpenFailure::text(
            StatusCode::SERVICE_UNAVAILABLE,
            "Browser cleanup remains owned by Runtime",
        )
        .with_outcome(browser_cleanup_pending_outcome(true, true, false))
        .status_value();
        let pre_effect = BrowserOpenFailure::text(
            StatusCode::SERVICE_UNAVAILABLE,
            "Browser VM was not acquired",
        )
        .status_value();

        assert_eq!(pending["outcome"]["state"], "cleanup_pending");
        assert_eq!(pending["outcome"]["effects"]["page_acquired"], true);
        assert_eq!(
            pre_effect["outcome"]["state"],
            "terminal_pre_effect_failure"
        );
        assert_eq!(pre_effect["outcome"]["effects"]["page_acquired"], false);
        assert_eq!(pre_effect["outcome"]["effects"]["vm_acquired"], false);
    }

    #[test]
    fn dispatched_runtime_failures_remain_indeterminate_without_typed_settlement() {
        assert!(matches!(
            browser_launch_reconciliation_decision(Ok(
                BrowserDispatchedLaunchReconciliation::CleanupPending
            )),
            BrowserLaunchReconciliationDecision::RetainIndeterminate(None)
        ));
        assert!(matches!(
            browser_launch_reconciliation_decision(Err(
                "injected Runtime reconciliation timeout".to_string()
            )),
            BrowserLaunchReconciliationDecision::RetainIndeterminate(Some(message))
                if message == "injected Runtime reconciliation timeout"
        ));
        assert!(matches!(
            browser_launch_reconciliation_decision(Ok(
                BrowserDispatchedLaunchReconciliation::DidNotAct
            )),
            BrowserLaunchReconciliationDecision::DidNotAct
        ));
    }
}
