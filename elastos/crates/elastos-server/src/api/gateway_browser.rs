//! Browser gateway helper contracts.
//!
//! Keep Browser-specific provider envelope handling here so the public gateway
//! module stays focused on HTTP route registration and response shaping.

use super::*;
#[path = "gateway_browser_engine.rs"]
mod gateway_browser_engine;
#[path = "gateway_browser_response.rs"]
mod gateway_browser_response;
#[path = "gateway_browser_sessions.rs"]
mod gateway_browser_sessions;
#[path = "gateway_browser_stream.rs"]
mod gateway_browser_stream;
#[path = "gateway_browser_validation.rs"]
mod gateway_browser_validation;
#[path = "gateway_browser_wallet.rs"]
mod gateway_browser_wallet;

pub(in crate::api::gateway) use gateway_browser_engine::*;
pub(in crate::api::gateway) use gateway_browser_response::*;
pub(in crate::api::gateway) use gateway_browser_sessions::*;
pub(in crate::api::gateway) use gateway_browser_stream::*;
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

#[derive(Debug)]
struct BrowserOpenFailure {
    status: StatusCode,
    body: BrowserOpenFailureBody,
}

#[derive(Debug)]
enum BrowserOpenFailureBody {
    Text(String),
    Json(serde_json::Value),
}

impl BrowserOpenFailure {
    fn text(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: BrowserOpenFailureBody::Text(message.into()),
        }
    }

    fn json(status: StatusCode, body: serde_json::Value) -> Self {
        Self {
            status,
            body: BrowserOpenFailureBody::Json(body),
        }
    }

    fn provider(scheme: &str, err: anyhow::Error) -> Self {
        let (status, message) = gateway_provider_error_tuple(scheme, err);
        Self::text(status, message)
    }

    fn into_response(self) -> Response {
        match self.body {
            BrowserOpenFailureBody::Text(message) => (self.status, message).into_response(),
            BrowserOpenFailureBody::Json(body) => (self.status, Json(body)).into_response(),
        }
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
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };
    let engine_adapter =
        browser_engine_summary(state.provider_registry.as_ref(), &context.principal_id).await;
    let net = browser_net_summary(state.provider_registry.as_ref(), &context.principal_id).await;
    let wallet_accounts = system_wallet_accounts_summary(&state, &context.principal_id).await;
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
        sessions: browser_gateway_session_status(&state.data_dir, &context.principal_id).await,
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    let home_token = home_launch_token_header(&headers);
    let request_origin = browser_request_origin(&headers);
    if input.async_open {
        let job = create_browser_open_job(&state.data_dir, &context.principal_id).await;
        let open_id = job.id.clone();
        let state_for_task = state.clone();
        tokio::spawn(async move {
            match execute_browser_open(&state_for_task, context, input, home_token, request_origin)
                .await
            {
                Ok(result) => complete_browser_open_job(&job, result).await,
                Err(error) => fail_browser_open_job(&job, error.status_value()).await,
            }
        });
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
    match execute_browser_open(&state, context, input, home_token, request_origin).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => error.into_response(),
    }
}

pub(super) async fn browser_app_open_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(open_id): Path<String>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&open_id) {
        return (StatusCode::BAD_REQUEST, "invalid Browser open id").into_response();
    }
    let Some(snapshot) =
        browser_open_job_for_principal(&state.data_dir, &open_id, &context.principal_id).await
    else {
        return (StatusCode::NOT_FOUND, "Browser open job not found").into_response();
    };
    Json(browser_open_status_value(&open_id, snapshot)).into_response()
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
    input: BrowserOpenRequest,
    home_token: Option<String>,
    request_origin: Option<String>,
) -> Result<serde_json::Value, BrowserOpenFailure> {
    let BrowserOpenRequest {
        url: requested_url,
        reason,
        remote_exit_id,
        adapter_id,
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
    let adapter_id = match browser_engine_adapter_id(adapter_id) {
        Ok(value) => value,
        Err(message) => return Err(BrowserOpenFailure::text(StatusCode::BAD_REQUEST, message)),
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
        return Err(BrowserOpenFailure::text(status, message));
    }
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
    let profile_key = profile
        .get("profile_key")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let lifecycle = BrowserLaunchLifecycle {
        url: url.clone(),
        exit_id: browser_lifecycle_exit_id(remote_exit_id.as_deref()),
        profile_key_hash: browser_lifecycle_hash(profile_key),
        vm_key_hash: browser_lifecycle_vm_key_hash(&[
            profile_key,
            remote_exit_id.as_deref().unwrap_or("local-runtime"),
            adapter_id.as_deref().unwrap_or("default-adapter"),
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
                release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                    .await;
                return Err(BrowserOpenFailure::provider("browser", err));
            }
        };
    let engine_stream_session = browser_engine_stream_session(&stream_session);
    let wallet = browser_wallet_bridge_payload(
        state,
        &context,
        home_token.as_deref(),
        request_origin.as_deref(),
    )
    .await;
    let mut engine_launch_request = serde_json::json!({
        "url": url.clone(),
        "stream_session": engine_stream_session,
        "principal_id": context.principal_id.clone(),
        "reason": reason.clone(),
        "profile": profile,
        "wallet": wallet,
        "viewport": viewport,
        "display_mode": display_mode,
        "guarantee_level": guarantee_level,
    });
    if let Some(adapter_id) = adapter_id.as_deref() {
        engine_launch_request["adapter_id"] = serde_json::json!(adapter_id);
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
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::text(status, message));
        }
    };
    let engine_response = match browser_provider_resource_response(state, engine_call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            mark_browser_launch_failed(&launch_reservation, message.clone()).await;
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!(message),
            ));
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
        if matches!(
            code,
            "engine_unavailable" | "byte_transport_unavailable" | "display_session_unavailable"
        ) {
            mark_browser_launch_failed(&launch_reservation, message.to_string()).await;
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider unavailable: {}", message),
            ));
        }
        if code == "browser_capacity_unavailable" {
            mark_browser_launch_failed(&launch_reservation, message.to_string()).await;
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::json(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({
                    "schema": "elastos.browser.open-error/v1",
                    "ok": false,
                    "code": code,
                    "message": message,
                }),
            ));
        }
        mark_browser_launch_failed(&launch_reservation, message.to_string()).await;
        release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone()).await;
        return Err(BrowserOpenFailure::provider(
            "browser-engine",
            anyhow::anyhow!(message.to_string()),
        ));
    }
    if let Some(message) = provider_response_error_message(&engine_response) {
        mark_browser_launch_failed(&launch_reservation, message.clone()).await;
        release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone()).await;
        return Err(BrowserOpenFailure::provider(
            "browser-engine",
            anyhow::anyhow!(message),
        ));
    }
    let engine_page = match provider_response_data(&engine_response)
        .map(|page| validate_browser_engine_page(page, display_mode, guarantee_level))
        .transpose()
    {
        Ok(Some(data)) => data,
        Ok(None) => {
            mark_browser_launch_failed(
                &launch_reservation,
                "browser-engine provider returned an invalid launch response",
            )
            .await;
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::provider(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider returned an invalid launch response"),
            ));
        }
        Err(err) => {
            mark_browser_launch_failed(&launch_reservation, err.to_string()).await;
            release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone())
                .await;
            return Err(BrowserOpenFailure::provider("browser-engine", err));
        }
    };
    let Some(page_id) = engine_page.get("page_id").and_then(|value| value.as_str()) else {
        mark_browser_launch_failed(
            &launch_reservation,
            "browser-engine provider returned page without page_id",
        )
        .await;
        release_browser_open_resources(state, &launch_reservation, stream_cleanup.clone()).await;
        return Err(BrowserOpenFailure::provider(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned page without page_id"),
        ));
    };
    complete_browser_launch(&launch_reservation, page_id, stream_cleanup.clone()).await;
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
        release_browser_open_resources(state, &launch_reservation, stream_cleanup).await;
        return Err(BrowserOpenFailure::text(status, message));
    }
    Ok(serde_json::json!({
        "schema": "elastos.browser.open-result/v1",
        "url": url,
        "target": target,
        "guarantee_level": guarantee_level.as_str(),
        "stream_session": browser_visible_stream_session(&stream_session),
        "engine_page": engine_page,
    }))
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

async fn cleanup_stale_browser_pages(state: &GatewayState) {
    retry_pending_browser_stream_cleanups(state).await;
    for page in take_stale_browser_pages(&state.data_dir).await {
        close_browser_page_record(state, page, "stale_browser_session_janitor").await;
    }
}

async fn retry_pending_browser_stream_cleanups(state: &GatewayState) {
    for cleanup in take_pending_browser_stream_cleanups(&state.data_dir).await {
        if let Err(err) = close_browser_stream_cleanup(state, Some(cleanup)).await {
            tracing::warn!(
                error = %err,
                "Browser pending stream cleanup retry failed"
            );
        }
    }
}

async fn close_browser_page_record(state: &GatewayState, page: BrowserPageCleanup, reason: &str) {
    if let Ok(call) = browser_provider_resource_call(
        "browser-engine",
        "close_page",
        "elastos://browser-engine/close_page".to_string(),
        serde_json::json!({
            "page_id": page.page_id.clone(),
            "principal_id": page.principal_id,
            "reason": reason,
        }),
    ) {
        let _ = browser_provider_resource_response(state, call).await;
    }
    if let Err(err) = close_browser_stream_cleanup(state, page.stream_cleanup).await {
        tracing::warn!(
            error = %err,
            "Browser stale page stream cleanup failed"
        );
    }
}

async fn release_browser_open_resources(
    state: &GatewayState,
    reservation: &BrowserLaunchReservation,
    stream_cleanup: Option<BrowserStreamCleanup>,
) {
    release_browser_launch(reservation).await;
    if let Err(err) = close_browser_stream_cleanup(state, stream_cleanup).await {
        tracing::warn!(
            error = %err,
            "Browser open resource cleanup failed"
        );
    }
}

async fn release_browser_page_and_stream_for_principal(
    state: &GatewayState,
    page_id: &str,
    principal_id: &str,
) -> Result<(), String> {
    let stream_cleanup =
        browser_page_stream_cleanup_for_principal(&state.data_dir, page_id, principal_id).await;
    close_browser_stream_cleanup(state, stream_cleanup).await?;
    let _ = release_browser_page_for_principal(&state.data_dir, page_id, principal_id).await;
    Ok(())
}

async fn close_browser_stream_cleanup(
    state: &GatewayState,
    stream_cleanup: Option<BrowserStreamCleanup>,
) -> Result<(), String> {
    let Some(cleanup) = stream_cleanup else {
        return Ok(());
    };
    let Some(registry) = state.provider_registry.as_ref() else {
        record_browser_stream_cleanup_failure(&state.data_dir, cleanup).await;
        return Err("exit provider unavailable while closing Browser stream".to_string());
    };
    let cleanup_for_failure = cleanup.clone();
    let call = match browser_provider_resource_call(
        "exit",
        "close_stream",
        "elastos://exit/close_stream".to_string(),
        serde_json::json!({
            "stream_id": cleanup.stream_id,
            "principal_id": cleanup.principal_id,
        }),
    ) {
        Ok(call) => call,
        Err((_status, message)) => {
            record_browser_stream_cleanup_failure(&state.data_dir, cleanup_for_failure).await;
            return Err(message);
        }
    };
    match registry.send_raw(call.scheme, &call.request).await {
        Ok(response) if response.get("status").and_then(|value| value.as_str()) == Some("ok") => {
            forget_browser_stream_cleanup_failure(
                &state.data_dir,
                call.request
                    .get("stream_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default(),
            )
            .await;
            Ok(())
        }
        Ok(response) => {
            let message = provider_response_error_message(&response).unwrap_or_else(|| {
                "exit provider returned an invalid close_stream response".into()
            });
            record_browser_stream_cleanup_failure(&state.data_dir, cleanup_for_failure).await;
            Err(message)
        }
        Err(err) => {
            record_browser_stream_cleanup_failure(&state.data_dir, cleanup_for_failure).await;
            Err(format!("exit provider close_stream failed: {err}"))
        }
    }
}

pub(super) async fn browser_app_page_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
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
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id).await;
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
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
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id).await;
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
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
        let _ =
            mark_browser_page_navigating(&state.data_dir, &page_id, &principal_id, navigation_url)
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
            let _ =
                mark_browser_page_failed(&state.data_dir, &page_id, &principal_id, message.clone())
                    .await;
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        let _ = mark_browser_page_failed(&state.data_dir, &page_id, &principal_id, message.clone())
            .await;
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            let _ = mark_browser_page_active(&state.data_dir, &page_id, &principal_id).await;
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id).await;
            Json(data).into_response()
        }
        None => {
            let _ = mark_browser_page_failed(
                &state.data_dir,
                &page_id,
                &principal_id,
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
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let principal_id = context.principal_id;
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    let _ = mark_browser_page_retiring(&state.data_dir, &page_id, &principal_id).await;
    let call = match browser_provider_resource_call(
        "browser-engine",
        "close_page",
        "elastos://browser-engine/close_page".to_string(),
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
            if let Some(receipt) = browser_close_reconciled_receipt(&page_id, &message) {
                if let Err(err) =
                    release_browser_page_and_stream_for_principal(&state, &page_id, &principal_id)
                        .await
                {
                    return (StatusCode::SERVICE_UNAVAILABLE, err).into_response();
                }
                return Json(receipt).into_response();
            }
            let _ =
                mark_browser_page_failed(&state.data_dir, &page_id, &principal_id, message.clone())
                    .await;
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        if let Some(receipt) = browser_close_reconciled_receipt(&page_id, &message) {
            if let Err(err) =
                release_browser_page_and_stream_for_principal(&state, &page_id, &principal_id).await
            {
                return (StatusCode::SERVICE_UNAVAILABLE, err).into_response();
            }
            return Json(receipt).into_response();
        }
        let _ = mark_browser_page_failed(&state.data_dir, &page_id, &principal_id, message.clone())
            .await;
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            if let Err(err) =
                release_browser_page_and_stream_for_principal(&state, &page_id, &principal_id).await
            {
                return (StatusCode::SERVICE_UNAVAILABLE, err).into_response();
            }
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid close-page response"),
        ),
    }
}

pub(super) fn browser_close_reconciled_receipt(
    page_id: &str,
    message: &str,
) -> Option<serde_json::Value> {
    if !message.contains("engine_process_unavailable")
        || !message.contains("no page-scoped engine control session")
    {
        return None;
    }
    Some(serde_json::json!({
        "schema": "elastos.browser.close-result/v1",
        "page_id": page_id,
        "closed": true,
        "already_closed": true,
        "reconciled": true,
        "control_error": message,
        "cleanup": {
            "schema": "elastos.browser.runtime-session-cleanup/v1",
            "ok": true,
            "action": "released_runtime_browser_session_after_missing_engine_control"
        }
    }))
}

pub(super) async fn browser_app_page_webrtc(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    Json(input): Json<BrowserWebrtcSignalRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
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
    let signal = match browser_webrtc_signal_value(input) {
        Ok(signal) => signal,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    if !touch_browser_page(&state.data_dir, &page_id, &principal_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
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
            let _ = touch_browser_page(&state.data_dir, &page_id, &principal_id).await;
            Json(data).into_response()
        }
        Err(err) => gateway_provider_error_response("browser-engine", err),
    }
}
