//! Browser wallet bridge gateway helpers.

use super::*;
use crate::api::auth_gateway;

#[path = "gateway_browser_wallet_bridge.rs"]
mod gateway_browser_wallet_bridge;
#[path = "gateway_browser_wallet_reads.rs"]
mod gateway_browser_wallet_reads;

use gateway_browser_wallet_bridge::browser_wallet_account_is_signable_evm;
pub(in crate::api::gateway) use gateway_browser_wallet_bridge::{
    browser_chain_namespace_network, browser_wallet_bridge_payload, is_browser_wallet_intent,
};
use gateway_browser_wallet_reads::browser_wallet_read;

fn browser_wallet_cors_origin(headers: &HeaderMap) -> HeaderValue {
    headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|origin| origin.starts_with("https://") || origin.starts_with("http://"))
        .and_then(|origin| HeaderValue::from_str(origin).ok())
        .unwrap_or_else(|| HeaderValue::from_static("*"))
}

fn browser_wallet_cors_response(headers: &HeaderMap, mut response: Response) -> Response {
    use axum::http::header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        ACCESS_CONTROL_MAX_AGE, VARY,
    };
    let response_headers = response.headers_mut();
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        browser_wallet_cors_origin(headers),
    );
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-elastos-home-token"),
    );
    response_headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    response_headers.insert(VARY, HeaderValue::from_static("Origin"));
    response
}

pub(in crate::api::gateway) async fn browser_app_wallet_cors_preflight(
    headers: HeaderMap,
) -> Response {
    browser_wallet_cors_response(&headers, StatusCode::NO_CONTENT.into_response())
}

pub(in crate::api::gateway) async fn browser_app_wallet_bridge(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let response = Json(
        browser_wallet_bridge_payload(
            &state,
            &context,
            home_launch_token_header(&headers).as_deref(),
            browser_request_origin(&headers).as_deref(),
        )
        .await,
    )
    .into_response();
    browser_wallet_cors_response(&headers, response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserWalletSignatureRequest {
    pub(in crate::api::gateway) method: String,
    #[serde(default)]
    pub(in crate::api::gateway) params: serde_json::Value,
    pub(in crate::api::gateway) account_id: String,
    pub(in crate::api::gateway) chain_namespace: String,
    pub(in crate::api::gateway) address: String,
    pub(in crate::api::gateway) page_url: String,
    #[serde(default)]
    pub(in crate::api::gateway) origin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserWalletTransactionRequest {
    pub(in crate::api::gateway) method: String,
    #[serde(default)]
    pub(in crate::api::gateway) params: serde_json::Value,
    pub(in crate::api::gateway) account_id: String,
    pub(in crate::api::gateway) chain_namespace: String,
    pub(in crate::api::gateway) address: String,
    pub(in crate::api::gateway) page_url: String,
    #[serde(default)]
    pub(in crate::api::gateway) origin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserWalletReadRequest {
    pub(in crate::api::gateway) method: String,
    #[serde(default)]
    pub(in crate::api::gateway) params: serde_json::Value,
    pub(in crate::api::gateway) chain_namespace: String,
    #[serde(default)]
    pub(in crate::api::gateway) address: Option<String>,
    pub(in crate::api::gateway) page_url: String,
    #[serde(default)]
    pub(in crate::api::gateway) origin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserWalletBroadcastRequest {
    pub(in crate::api::gateway) request_id: String,
}

pub(in crate::api::gateway) async fn browser_app_wallet_request_signature(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletSignatureRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let response = match create_browser_wallet_signature_request(&state, &context, input).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

pub(in crate::api::gateway) async fn browser_app_wallet_request_transaction(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletTransactionRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let response = match create_browser_wallet_transaction_request(&state, &context, input).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

pub(in crate::api::gateway) async fn browser_app_wallet_read(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletReadRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let response = match browser_wallet_read(&state, &context, input).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

pub(in crate::api::gateway) async fn browser_app_wallet_broadcast_transaction(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletBroadcastRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    if !is_safe_runtime_id(&input.request_id) {
        return browser_wallet_cors_response(
            &headers,
            (
                StatusCode::BAD_REQUEST,
                "invalid browser wallet approval id",
            )
                .into_response(),
        );
    }
    let response =
        match browser_wallet_broadcast_transaction(&state, &context, &input.request_id).await {
            Ok(payload) => Json(payload).into_response(),
            Err((status, message)) => (status, message).into_response(),
        };
    browser_wallet_cors_response(&headers, response)
}

pub(in crate::api::gateway) async fn browser_app_wallet_approval_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, BROWSER_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    if !is_safe_runtime_id(&request_id) {
        return browser_wallet_cors_response(
            &headers,
            (
                StatusCode::BAD_REQUEST,
                "invalid browser wallet approval id",
            )
                .into_response(),
        );
    }
    let response = match browser_wallet_approval_status(&state, &context, &request_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

async fn create_browser_wallet_transaction_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    input: BrowserWalletTransactionRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let account_id = input.account_id.clone();
    let chain_namespace = input.chain_namespace.clone();
    let address = input.address.clone();
    let page_url = input.page_url.clone();
    let _origin = input.origin.as_deref();
    let method = input.method.trim().to_string();
    if method != "eth_sendTransaction" {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser wallet bridge supports eth_sendTransaction transaction approvals only"
                .to_string(),
        ));
    }
    if browser_url_to_stream_target(&page_url).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid browser page URL".to_string(),
        ));
    }
    let params = input.params.as_array().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser wallet transaction params must be an array".to_string(),
        )
    })?;
    let tx = params
        .first()
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "eth_sendTransaction requires a transaction object".to_string(),
            )
        })?;
    let requested_from = tx
        .get("from")
        .and_then(|value| value.as_str())
        .unwrap_or(address.as_str());
    if !requested_from.eq_ignore_ascii_case(&address) {
        return Err((
            StatusCode::BAD_REQUEST,
            "transaction from address does not match selected Browser wallet account".to_string(),
        ));
    }
    let Some(to) = tx.get("to").and_then(|value| value.as_str()) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "eth_sendTransaction requires a to address".to_string(),
        ));
    };
    let value = tx
        .get("value")
        .and_then(|value| value.as_str())
        .unwrap_or("0x0");
    let data = tx
        .get("data")
        .and_then(|value| value.as_str())
        .unwrap_or("0x");
    if data.len() > 256 * 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "transaction data is too large for Browser wallet approval".to_string(),
        ));
    }
    let Some(network) = browser_chain_namespace_network(&chain_namespace) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser transaction approvals require a supported eip155 chain".to_string(),
        ));
    };
    let accounts = system_wallet_accounts_summary(state, &context.principal_id).await;
    let Some(account) = accounts.accounts.iter().find(|account| {
        account.account_id == account_id
            && account.chain_namespace.starts_with("eip155:")
            && chain_namespace.starts_with("eip155:")
            && account.address.eq_ignore_ascii_case(&address)
    }) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser wallet transaction account is not linked to this Runtime principal"
                .to_string(),
        ));
    };
    if !account.chain_namespace.starts_with("eip155:") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser transactions require an EVM wallet account".to_string(),
        ));
    }
    let chain_prepare_resource = format!("elastos://chain/{network}/prepare_transaction");
    let chain_broadcast_resource = format!("elastos://chain/{network}/broadcast_transaction");
    let prepare_call = browser_provider_resource_call(
        "chain",
        "prepare_transaction",
        chain_prepare_resource,
        serde_json::json!({
            "network": network,
            "from": account.address.clone(),
            "to": to,
            "value": value,
            "data": data,
        }),
    )?;
    let prepare_response = browser_provider_resource_response(state, prepare_call).await?;
    if let Some(message) = provider_response_error_message(&prepare_response) {
        return Err((StatusCode::BAD_REQUEST, message));
    }
    let intent = provider_response_data(&prepare_response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned an invalid transaction intent".to_string(),
        )
    })?;
    if intent.get("schema").and_then(|value| value.as_str())
        != Some("elastos.chain.unsigned_transaction_intent/v1")
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned an unsupported transaction intent".to_string(),
        ));
    }
    let wallet_sign_resource = format!(
        "elastos://wallet/{}/sign/transaction_intent",
        chain_namespace
    );
    let wallet_call = browser_provider_resource_call(
        "wallet",
        "request_signature",
        wallet_sign_resource,
        serde_json::json!({
            "principal_id": context.principal_id,
            "account_id": account.account_id.clone(),
            "chain_namespace": chain_namespace,
            "intent": "transaction_intent",
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": chain_broadcast_resource,
            "reason": format!("Browser page requests {method} on {network}"),
            "payload": intent
        }),
    )?;
    let response = browser_provider_resource_response(state, wallet_call).await?;
    let data = provider_response_data(&response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "wallet-provider returned an invalid approval response".to_string(),
        )
    })?;
    Ok(serde_json::json!({
        "schema": "elastos.browser.wallet-approval-result/v1",
        "requires_approval": true,
        "approval_request": data.get("approval_request").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

pub(in crate::api::gateway) struct BrowserEffectAuditInput<'a> {
    pub(in crate::api::gateway) event_type: &'a str,
    pub(in crate::api::gateway) principal_id: &'a str,
    pub(in crate::api::gateway) session_id: &'a str,
    pub(in crate::api::gateway) request_id: &'a str,
    pub(in crate::api::gateway) result: &'a str,
    pub(in crate::api::gateway) method: &'a str,
    pub(in crate::api::gateway) resource: &'a str,
    pub(in crate::api::gateway) page_url: &'a str,
    pub(in crate::api::gateway) origin: Option<&'a str>,
    pub(in crate::api::gateway) decision: &'a str,
}

pub(in crate::api::gateway) fn browser_effect_request_id(prefix: &str, method: &str) -> String {
    let safe_method: String = method
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{prefix}:{safe_method}:{timestamp}")
}

pub(in crate::api::gateway) fn append_browser_effect_audit_or_500(
    data_dir: &std::path::Path,
    input: BrowserEffectAuditInput<'_>,
) -> Result<(), (StatusCode, String)> {
    append_browser_effect_audit(data_dir, input).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Browser effect audit failed: {err}"),
        )
    })
}

fn append_browser_effect_audit(
    data_dir: &std::path::Path,
    input: BrowserEffectAuditInput<'_>,
) -> anyhow::Result<()> {
    let now = now_ts();
    crate::auth::append_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!(
                "audit:browser-effect:{}:{}:{now}",
                input.event_type, input.request_id
            ),
            event_type: input.event_type.to_string(),
            principal_id: Some(input.principal_id.to_string()),
            proof_binding_id: None,
            session_id: Some(input.session_id.to_string()),
            challenge_id: Some(input.request_id.to_string()),
            capsule_id: Some(BROWSER_CAPSULE_ID.to_string()),
            result: input.result.to_string(),
            reason: format!(
                "method={} resource={} page_url={} origin={} decision={}",
                input.method,
                input.resource,
                input.page_url,
                input.origin.unwrap_or(""),
                input.decision
            ),
            occurred_at: now,
            signer_did: None,
            signature: None,
        },
    )
}

fn provider_response_data_or_bad_request(
    response: &serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, String)> {
    if let Some(message) = provider_response_error_message(response) {
        return Err((StatusCode::BAD_REQUEST, message));
    }
    provider_response_data(response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned an invalid read response".to_string(),
        )
    })
}

async fn create_browser_wallet_signature_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    input: BrowserWalletSignatureRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let account_id = input.account_id.clone();
    let chain_namespace = input.chain_namespace.clone();
    let address = input.address.clone();
    let page_url = input.page_url.clone();
    let origin = input.origin.clone();
    let method = input.method.trim();
    let is_personal = method == "personal_sign" || method == "eth_sign";
    let is_typed_data = is_browser_typed_data_sign_method(method);
    if !is_personal && !is_typed_data {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser wallet bridge supports personal_sign, eth_sign, and eth_signTypedData approval requests only".to_string(),
        ));
    }
    let params = input.params.as_array().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser wallet request params must be an array".to_string(),
        )
    })?;
    let (
        intent,
        resource_action,
        reason,
        payload_params,
        message,
        typed_data,
        typed_data_canonical,
        requested_address,
    ) = if is_typed_data {
        let (requested_address, typed_data, canonical) =
            browser_typed_data_signature_parts(params, &address)?;
        (
            "browser_typed_data_sign",
            "browser_typed_data_sign",
            format!("Browser page requests {method}"),
            serde_json::json!([requested_address.clone(), canonical.clone()]),
            None,
            Some(typed_data),
            Some(canonical),
            requested_address,
        )
    } else {
        let message = params
            .first()
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "personal_sign requires a message parameter".to_string(),
                )
            })?;
        if message.is_empty() || message.len() > 8 * 1024 || message.chars().any(char::is_control) {
            return Err((
                StatusCode::BAD_REQUEST,
                "personal_sign message size is invalid".to_string(),
            ));
        }
        let requested_address = params
            .get(1)
            .and_then(|value| value.as_str())
            .unwrap_or(address.as_str())
            .to_string();
        (
            "browser_personal_sign",
            "browser_personal_sign",
            format!("Browser page requests {method}"),
            serde_json::json!([message, requested_address.clone()]),
            Some(message.to_string()),
            None,
            None,
            requested_address,
        )
    };
    if !requested_address.eq_ignore_ascii_case(&address) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser signature address does not match selected Browser wallet account".to_string(),
        ));
    }
    if browser_url_to_stream_target(&page_url).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid browser page URL".to_string(),
        ));
    }
    let accounts = system_wallet_accounts_summary(state, &context.principal_id).await;
    let Some(account) = accounts.accounts.iter().find(|account| {
        account.account_id == account_id
            && account.chain_namespace.starts_with("eip155:")
            && chain_namespace.starts_with("eip155:")
            && account.address.eq_ignore_ascii_case(&address)
    }) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser wallet request account is not linked to this Runtime principal".to_string(),
        ));
    };
    if !account.chain_namespace.starts_with("eip155:") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser signatures require an EVM wallet account".to_string(),
        ));
    }
    let mut payload = serde_json::json!({
        "schema": "elastos.browser.wallet-signature-request/v1",
        "method": method,
        "params": payload_params,
        "address": account.address.clone(),
        "account_id": account.account_id.clone(),
        "chain_namespace": chain_namespace,
        "page_url": page_url,
        "origin": origin,
        "principal_id": context.principal_id,
        "session_id": context.session_id,
        "requires_wallet_approval": true
    });
    if let Some(message) = message {
        payload["message"] = serde_json::Value::String(message);
    }
    if let Some(typed_data) = typed_data {
        payload["typed_data"] = typed_data;
    }
    if let Some(canonical) = typed_data_canonical {
        payload["typed_data_canonical"] = serde_json::Value::String(canonical);
    }
    let response = auth_gateway::wallet_provider_data(
        state,
        serde_json::json!({
            "op": "request_signature",
            "principal_id": context.principal_id,
            "account_id": account_id,
            "chain_namespace": chain_namespace,
            "intent": intent,
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": format!("elastos://wallet/{}/sign/{resource_action}", chain_namespace),
            "reason": reason,
            "payload": payload
        }),
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
    let data = provider_response_data(&response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "wallet-provider returned an invalid approval response".to_string(),
        )
    })?;
    Ok(serde_json::json!({
        "schema": "elastos.browser.wallet-approval-result/v1",
        "requires_approval": true,
        "approval_request": data.get("approval_request").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

fn is_browser_typed_data_sign_method(method: &str) -> bool {
    matches!(
        method,
        "eth_signTypedData" | "eth_signTypedData_v3" | "eth_signTypedData_v4"
    )
}

fn browser_typed_data_signature_parts(
    params: &[serde_json::Value],
    selected_address: &str,
) -> Result<(String, serde_json::Value, String), (StatusCode, String)> {
    if params.len() < 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            "eth_signTypedData requires address and typed-data parameters".to_string(),
        ));
    }
    let first = params.first().and_then(|value| value.as_str());
    let second = params.get(1).and_then(|value| value.as_str());
    let (requested_address, typed_data_value) =
        if first.is_some_and(|value| value.eq_ignore_ascii_case(selected_address)) {
            (first.unwrap().to_string(), params.get(1).cloned())
        } else if second.is_some_and(|value| value.eq_ignore_ascii_case(selected_address)) {
            (second.unwrap().to_string(), params.first().cloned())
        } else {
            (selected_address.to_string(), params.get(1).cloned())
        };
    let Some(typed_data_value) = typed_data_value else {
        return Err((
            StatusCode::BAD_REQUEST,
            "eth_signTypedData missing typed-data payload".to_string(),
        ));
    };
    let typed_data = if let Some(raw) = typed_data_value.as_str() {
        if raw.is_empty() || raw.len() > 32 * 1024 {
            return Err((
                StatusCode::BAD_REQUEST,
                "eth_signTypedData payload size is invalid".to_string(),
            ));
        }
        serde_json::from_str(raw).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "eth_signTypedData payload must be JSON".to_string(),
            )
        })?
    } else {
        typed_data_value
    };
    let canonical = serde_json::to_string(&typed_data).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "eth_signTypedData payload is not serializable".to_string(),
        )
    })?;
    if canonical.is_empty() || canonical.len() > 32 * 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "eth_signTypedData payload size is invalid".to_string(),
        ));
    }
    Ok((requested_address, typed_data, canonical))
}

async fn browser_wallet_approval_status(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let response = auth_gateway::wallet_provider_data(
        state,
        serde_json::json!({
            "op": "approval_requests",
            "principal_id": context.principal_id,
            "include_resolved": true,
        }),
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
    let approvals = response
        .get("approval_requests")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider returned an invalid approval list".to_string(),
            )
        })?;
    let request = approvals
        .iter()
        .find(|request| {
            request.get("request_id").and_then(|value| value.as_str()) == Some(request_id)
                && is_browser_wallet_intent(request.get("intent").and_then(|value| value.as_str()))
                && request.get("capsule_id").and_then(|value| value.as_str())
                    == Some(BROWSER_CAPSULE_ID)
        })
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "browser wallet approval request not found".to_string(),
            )
        })?;
    let status = request
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let mut payload = serde_json::json!({
        "schema": "elastos.browser.wallet-approval-status/v1",
        "request_id": request_id,
        "status": status,
    });
    if status == "completed" {
        let result = request.get("signed_result").ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "completed browser wallet approval is missing signed result".to_string(),
            )
        })?;
        if matches!(
            request.get("intent").and_then(|value| value.as_str()),
            Some("browser_personal_sign") | Some("browser_typed_data_sign")
        ) {
            let signature = result
                .get("signature")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "completed browser wallet approval is missing signature".to_string(),
                    )
                })?;
            payload["signature"] = serde_json::Value::String(signature.to_string());
        } else if request.get("intent").and_then(|value| value.as_str())
            == Some("transaction_intent")
        {
            if let Some(signed_transaction) = result
                .get("signed_transaction")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                payload["signed_transaction"] =
                    serde_json::Value::String(signed_transaction.to_string());
            }
            if let Some(hash) = result.get("transaction_hash").cloned() {
                payload["transaction_hash"] = hash;
            }
            if payload.get("signed_transaction").is_none()
                && payload.get("transaction_hash").is_none()
            {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "completed browser wallet approval is missing transaction result".to_string(),
                ));
            }
        }
        payload["signed_result"] = result.clone();
    }
    Ok(payload)
}

async fn browser_wallet_broadcast_transaction(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let status = browser_wallet_approval_status(state, context, request_id).await?;
    if status.get("status").and_then(|value| value.as_str()) != Some("completed") {
        return Err((
            StatusCode::BAD_REQUEST,
            "browser transaction approval is not completed".to_string(),
        ));
    }
    if let Some(transaction_hash) = status
        .get("transaction_hash")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(serde_json::json!({
            "schema": "elastos.browser.transaction-broadcast/v1",
            "request_id": request_id,
            "transaction_hash": transaction_hash,
            "already_recorded": true,
        }));
    }
    let signed_transaction = status
        .get("signed_transaction")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "completed browser wallet approval is missing signed transaction".to_string(),
            )
        })?;
    let chain_namespace = status
        .get("signed_result")
        .and_then(|value| value.get("chain_namespace"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "completed browser wallet approval is missing chain namespace".to_string(),
            )
        })?;
    let Some(network) = browser_chain_namespace_network(chain_namespace) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser transaction approval uses an unsupported eip155 chain".to_string(),
        ));
    };
    let broadcast_call = browser_provider_resource_call(
        "chain",
        "broadcast_transaction",
        format!("elastos://chain/{network}/broadcast_transaction"),
        serde_json::json!({
            "network": network,
            "signed_transaction": signed_transaction,
        }),
    )?;
    let response = browser_provider_resource_response(state, broadcast_call).await?;
    if let Some(message) = provider_response_error_message(&response) {
        return Err((StatusCode::BAD_REQUEST, message));
    }
    let receipt = provider_response_data(&response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned an invalid broadcast receipt".to_string(),
        )
    })?;
    let transaction_hash = receipt
        .get("transaction_hash")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "chain provider broadcast receipt is missing transaction hash".to_string(),
            )
        })?;
    let recorded = auth_gateway::wallet_provider_data(
        state,
        serde_json::json!({
            "op": "record_transaction_hash",
            "principal_id": context.principal_id,
            "request_id": request_id,
            "transaction_hash": transaction_hash,
        }),
    )
    .await
    .is_ok();
    Ok(serde_json::json!({
        "schema": "elastos.browser.transaction-broadcast/v1",
        "request_id": request_id,
        "transaction_hash": transaction_hash,
        "recorded": recorded,
        "receipt": receipt,
    }))
}
