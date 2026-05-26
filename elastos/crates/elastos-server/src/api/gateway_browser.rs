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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserOpenRequest {
    pub(super) url: String,
    #[serde(default)]
    pub(super) reason: Option<String>,
    #[serde(default)]
    pub(super) viewport: Option<BrowserViewportRequest>,
    #[serde(default = "default_browser_display_mode")]
    pub(super) display_mode: BrowserDisplayMode,
}

#[derive(Deserialize)]
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
    DiagnosticFrame,
}

impl BrowserDisplayMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::WebrtcRemoteDisplay => "webrtc_remote_display",
            Self::NativeSurface => "native_surface",
            Self::DiagnosticFrame => "diagnostic_frame",
        }
    }
}

fn default_browser_display_mode() -> BrowserDisplayMode {
    BrowserDisplayMode::WebrtcRemoteDisplay
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
    pub(super) sdp: Option<String>,
    #[serde(default)]
    pub(super) candidate: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserFrameQuery {
    #[serde(default)]
    pub(super) since: Option<u64>,
    #[serde(default)]
    pub(super) wait_ms: Option<u64>,
}

pub(super) struct BrowserProviderResourceCall {
    pub(super) scheme: &'static str,
    pub(super) resource: String,
    pub(super) request: serde_json::Value,
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
    let (url, target) = match browser_url_to_stream_target(&input.url) {
        Ok(value) => value,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "browser",
                anyhow::anyhow!("browser providers unavailable"),
            )
        }
    };
    cleanup_stale_browser_pages(&state).await;
    let reason = input
        .reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "open browser page".to_string());
    let request_origin = browser_request_origin(&headers);
    let open_request_id = browser_effect_request_id("open", &url);
    if let Err((status, message)) = append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.open.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &open_request_id,
            result: "requested",
            method: input.display_mode.as_str(),
            resource: &target,
            page_url: &url,
            origin: request_origin.as_deref(),
            decision: "runtime_net_exit_policy",
        },
    ) {
        return (status, message).into_response();
    }
    let viewport = match input.viewport {
        Some(viewport) => match browser_viewport_value(viewport) {
            Ok(value) => Some(value),
            Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
        },
        None => None,
    };
    let launch_reservation =
        match reserve_browser_launch(&state.data_dir, &context.principal_id).await {
            Ok(reservation) => reservation,
            Err((status, message)) => return (status, message).into_response(),
        };
    let stream_request = serde_json::json!({
        "op": "stream",
        "target": target,
        "principal_id": context.principal_id,
        "reason": reason,
    });
    let stream_session =
        match browser_reserve_stream_session(registry.as_ref(), &stream_request).await {
            Ok(receipt) => receipt,
            Err((provider, err)) => {
                release_browser_launch(&launch_reservation).await;
                return gateway_provider_error_response(provider, err);
            }
        };
    let stream_session =
        match browser_attach_runtime_stream_path(&state.data_dir, stream_session).await {
            Ok(receipt) => receipt,
            Err(err) => {
                release_browser_launch(&launch_reservation).await;
                return gateway_provider_error_response("browser", err);
            }
        };
    let engine_stream_session = browser_engine_stream_session(&stream_session);
    let wallet = browser_wallet_bridge_payload(
        &state,
        &context,
        home_launch_token_header(&headers).as_deref(),
        request_origin.as_deref(),
    )
    .await;
    let engine_call = match browser_provider_resource_call(
        "browser-engine",
        "launch",
        "elastos://browser-engine/launch".to_string(),
        serde_json::json!({
            "url": url,
            "stream_session": engine_stream_session,
            "principal_id": context.principal_id,
            "reason": reason,
            "wallet": wallet,
            "viewport": viewport,
            "display_mode": input.display_mode,
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => {
            release_browser_launch(&launch_reservation).await;
            return (status, message).into_response();
        }
    };
    let engine_response = match browser_provider_resource_response(&state, engine_call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            release_browser_launch(&launch_reservation).await;
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
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
            release_browser_launch(&launch_reservation).await;
            return gateway_provider_error_response(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider unavailable: {}", message),
            );
        }
        release_browser_launch(&launch_reservation).await;
        return gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!(message.to_string()),
        );
    }
    if let Some(message) = provider_response_error_message(&engine_response) {
        release_browser_launch(&launch_reservation).await;
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    let engine_page = match provider_response_data(&engine_response)
        .map(|page| validate_browser_engine_page(page, input.display_mode))
        .transpose()
    {
        Ok(Some(data)) => data,
        Ok(None) => {
            release_browser_launch(&launch_reservation).await;
            return gateway_provider_error_response(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider returned an invalid launch response"),
            );
        }
        Err(err) => {
            release_browser_launch(&launch_reservation).await;
            return gateway_provider_error_response("browser-engine", err);
        }
    };
    let Some(page_id) = engine_page.get("page_id").and_then(|value| value.as_str()) else {
        release_browser_launch(&launch_reservation).await;
        return gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned page without page_id"),
        );
    };
    complete_browser_launch(&launch_reservation, page_id).await;
    if let Err((status, message)) = append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.open.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &open_request_id,
            result: "allowed",
            method: input.display_mode.as_str(),
            resource: &target,
            page_url: &url,
            origin: request_origin.as_deref(),
            decision: "browser_engine_provider",
        },
    ) {
        release_browser_launch(&launch_reservation).await;
        return (status, message).into_response();
    }
    Json(serde_json::json!({
        "schema": "elastos.browser.open-result/v1",
        "url": url,
        "target": target,
        "stream_session": browser_visible_stream_session(&stream_session),
        "engine_page": engine_page,
    }))
    .into_response()
}

async fn cleanup_stale_browser_pages(state: &GatewayState) {
    for page in take_stale_browser_pages(&state.data_dir).await {
        let call = match browser_provider_resource_call(
            "browser-engine",
            "close_page",
            "elastos://browser-engine/close_page".to_string(),
            serde_json::json!({
                "page_id": page.page_id,
                "principal_id": page.principal_id,
                "reason": "stale_browser_session_janitor",
            }),
        ) {
            Ok(call) => call,
            Err(_) => continue,
        };
        let _ = browser_provider_resource_response(state, call).await;
    }
}

pub(super) async fn browser_app_page_screenshot(
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
    let call = match browser_provider_resource_call(
        "browser-engine",
        "screenshot",
        "elastos://browser-engine/page/screenshot".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "principal_id": context.principal_id,
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
                anyhow::anyhow!("browser-engine provider returned an invalid screenshot response"),
            )
        }
    };
    let _ = touch_browser_page(&state.data_dir, &page_id).await;
    match browser_screenshot_bytes(data) {
        Ok((content_type, bytes)) => (
            [(CONTENT_TYPE, HeaderValue::from_static(content_type))],
            bytes,
        )
            .into_response(),
        Err(err) => gateway_provider_error_response("browser-engine", err),
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
    let call = match browser_provider_resource_call(
        "browser-engine",
        "page_status",
        "elastos://browser-engine/page/status".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "principal_id": context.principal_id,
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
            let _ = touch_browser_page(&state.data_dir, &page_id).await;
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid page-status response"),
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
    if !touch_browser_page(&state.data_dir, &page_id).await {
        return (StatusCode::NOT_FOUND, "browser session is not active").into_response();
    }
    Json(serde_json::json!({
        "schema": "elastos.browser.page-heartbeat/v1",
        "page_id": page_id,
        "principal_id": context.principal_id,
        "ok": true,
    }))
    .into_response()
}

pub(super) async fn browser_app_page_frame(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    Query(query): Query<BrowserFrameQuery>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response("browser", err),
        };
    if !is_safe_runtime_id(&page_id) {
        return (StatusCode::BAD_REQUEST, "invalid browser page id").into_response();
    }
    let wait_ms = query.wait_ms.unwrap_or(1200).min(5000);
    let call = match browser_provider_resource_call(
        "browser-engine",
        "frame",
        "elastos://browser-engine/page/frame".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "since": query.since.unwrap_or(0),
            "wait_ms": wait_ms,
            "principal_id": context.principal_id,
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
            let _ = touch_browser_page(&state.data_dir, &page_id).await;
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid frame response"),
        ),
    }
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
    let call = match browser_provider_resource_call(
        "browser-engine",
        "input",
        "elastos://browser-engine/page/input".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "event": input.event,
            "principal_id": context.principal_id,
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
            let _ = touch_browser_page(&state.data_dir, &page_id).await;
            Json(data).into_response()
        }
        None => gateway_provider_error_response(
            "browser-engine",
            anyhow::anyhow!("browser-engine provider returned an invalid input response"),
        ),
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
    let call = match browser_provider_resource_call(
        "browser-engine",
        "close_page",
        "elastos://browser-engine/close_page".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "principal_id": context.principal_id,
        }),
    ) {
        Ok(call) => call,
        Err((status, message)) => return (status, message).into_response(),
    };
    let response = match browser_provider_resource_response(&state, call).await {
        Ok(value) => value,
        Err((_status, message)) => {
            release_browser_page(&state.data_dir, &page_id).await;
            return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
        }
    };
    if let Some(message) = provider_response_error_message(&response) {
        release_browser_page(&state.data_dir, &page_id).await;
        return gateway_provider_error_response("browser-engine", anyhow::anyhow!(message));
    }
    match provider_response_data(&response) {
        Some(data) => {
            release_browser_page(&state.data_dir, &page_id).await;
            Json(data).into_response()
        }
        None => {
            release_browser_page(&state.data_dir, &page_id).await;
            gateway_provider_error_response(
                "browser-engine",
                anyhow::anyhow!("browser-engine provider returned an invalid close-page response"),
            )
        }
    }
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
    let signal_type = input.signal_type.clone();
    let signal = match browser_webrtc_signal_value(input) {
        Ok(signal) => signal,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    let call = match browser_provider_resource_call(
        "browser-engine",
        "webrtc_signal",
        "elastos://browser-engine/page/webrtc_signal".to_string(),
        serde_json::json!({
            "page_id": page_id,
            "signal": signal,
            "principal_id": context.principal_id,
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
            let _ = touch_browser_page(&state.data_dir, &page_id).await;
            Json(data).into_response()
        }
        Err(err) => gateway_provider_error_response("browser-engine", err),
    }
}
