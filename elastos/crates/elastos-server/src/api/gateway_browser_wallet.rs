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
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let context = authority.home_launch_context();
    let response = Json(
        browser_wallet_bridge_payload(
            &state,
            &context,
            &authority,
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

const BROWSER_PENDING_TRANSACTION_BROADCAST_SCHEMA: &str =
    "elastos.browser.pending-transaction-broadcast/v1";

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct BrowserPendingTransactionBroadcast {
    schema: String,
    principal_id: String,
    request_id: String,
    chain_namespace: String,
    network: String,
    transaction_hash: String,
    receipt: serde_json::Value,
    created_at: u64,
}

pub(in crate::api::gateway) async fn browser_app_wallet_request_signature(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletSignatureRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let context = authority.home_launch_context();
    let response =
        match create_browser_wallet_signature_request(&state, &context, &authority, input).await {
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
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let context = authority.home_launch_context();
    let response = match create_browser_wallet_transaction_request(
        &state, &context, &authority, input,
    )
    .await
    {
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
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let context = authority.home_launch_context();
    let response = match browser_wallet_read(&state, &context, &authority, input).await {
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
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => {
                return browser_wallet_cors_response(
                    &headers,
                    gateway_provider_error_response("browser", err),
                );
            }
        };
    let context = authority.home_launch_context();
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
        match browser_wallet_broadcast_transaction(&state, &context, &authority, &input.request_id)
            .await
        {
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
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(authority) => authority,
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
    let response = match browser_wallet_approval_status(&state, &authority, &request_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

async fn create_browser_wallet_transaction_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    input: BrowserWalletTransactionRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let account_id = input.account_id.clone();
    let chain_namespace = input.chain_namespace.clone();
    let address = input.address.clone();
    let page_url = input.page_url.clone();
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
    let accounts = system_wallet_accounts_summary(state, authority).await;
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
    let mut intent = provider_response_data(&prepare_response).ok_or_else(|| {
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
    if let Some(intent_object) = intent.as_object_mut() {
        intent_object.insert("method".to_string(), serde_json::json!(method.clone()));
        intent_object.insert("page_url".to_string(), serde_json::json!(page_url));
        intent_object.insert(
            "origin".to_string(),
            serde_json::json!(input.origin.clone()),
        );
        intent_object.insert(
            "principal_id".to_string(),
            serde_json::json!(context.principal_id.clone()),
        );
        intent_object.insert(
            "session_id".to_string(),
            serde_json::json!(context.session_id.clone()),
        );
    }
    let data = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::RequestApproval {
            account_id: account.account_id.clone(),
            chain_namespace,
            intent: "transaction_intent".to_string(),
            resource: chain_broadcast_resource,
            reason: format!("Browser page requests {method} on {network}"),
            payload: intent,
            expires_at: now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
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
    authority: &RuntimeWalletAuthority,
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
    let accounts = system_wallet_accounts_summary(state, authority).await;
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
    let data = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: chain_namespace.clone(),
            intent: intent.to_string(),
            resource: format!("elastos://wallet/{chain_namespace}/sign/{resource_action}"),
            reason,
            payload,
            expires_at: now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
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
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let response = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ListApprovals {
            include_resolved: true,
        },
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
            let has_signed_transaction = if let Some(signed_transaction) = result
                .get("signed_transaction")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                payload["signed_transaction"] =
                    serde_json::Value::String(signed_transaction.to_string());
                true
            } else {
                false
            };
            let broadcast_recorded = result.get("broadcast_recorded_at").is_some();
            if let Some(hash) = result.get("transaction_hash").cloned() {
                if !has_signed_transaction || broadcast_recorded {
                    payload["transaction_hash"] = hash;
                }
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

fn browser_pending_transaction_broadcast_uri(
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> String {
    format!(
        "{}/.AppData/ElastOS/Browser/pending-transaction-broadcasts/{request_id}.json",
        crate::auth::principal_localhost_root(&context.principal_id)
    )
}

fn browser_pending_transaction_broadcast_path(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> Result<(String, std::path::PathBuf), (StatusCode, String)> {
    let uri = browser_pending_transaction_broadcast_uri(context, request_id);
    let path = rooted_localhost_fs_path(&state.data_dir, &uri).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid Browser pending broadcast storage path".to_string(),
        )
    })?;
    Ok((uri, path))
}

fn browser_pending_transaction_missing(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn read_browser_pending_transaction_broadcast(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> Result<Option<BrowserPendingTransactionBroadcast>, (StatusCode, String)> {
    let (uri, path) = browser_pending_transaction_broadcast_path(state, context, request_id)?;
    if !path.is_file() {
        return Ok(None);
    }
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    let bytes = match crate::auth::read_principal_root_object(
        &state.data_dir,
        &context.principal_id,
        &localhost_root,
        &uri,
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if browser_pending_transaction_missing(&err) => return Ok(None),
        Err(err) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read Browser pending transaction broadcast: {err}"),
            ));
        }
    };
    let pending: BrowserPendingTransactionBroadcast =
        serde_json::from_slice(&bytes).map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("invalid Browser pending transaction broadcast: {err}"),
            )
        })?;
    if pending.schema != BROWSER_PENDING_TRANSACTION_BROADCAST_SCHEMA
        || pending.principal_id != context.principal_id
        || pending.request_id != request_id
        || pending.transaction_hash.trim().is_empty()
    {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Browser pending transaction broadcast failed validation".to_string(),
        ));
    }
    Ok(Some(pending))
}

fn write_browser_pending_transaction_broadcast(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    pending: &BrowserPendingTransactionBroadcast,
) -> Result<(), (StatusCode, String)> {
    let (uri, path) =
        browser_pending_transaction_broadcast_path(state, context, &pending.request_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to create Browser pending broadcast storage: {err}"),
            )
        })?;
    }
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    let bytes = serde_json::to_vec_pretty(pending).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode Browser pending transaction broadcast: {err}"),
        )
    })?;
    crate::auth::write_principal_root_object(
        &state.data_dir,
        &context.principal_id,
        &localhost_root,
        &uri,
        &path,
        &bytes,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save Browser pending transaction broadcast: {err}"),
        )
    })
}

fn remove_browser_pending_transaction_broadcast(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) {
    let Ok((_uri, path)) = browser_pending_transaction_broadcast_path(state, context, request_id)
    else {
        return;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(
            path = %path.display(),
            error = %err,
            "failed to remove Browser pending transaction broadcast"
        ),
    }
}

async fn record_browser_transaction_hash(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
    transaction_hash: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    auth_gateway::wallet_provider_data(
        state,
        serde_json::json!({
            "op": "record_transaction_hash",
            "principal_id": context.principal_id,
            "request_id": request_id,
            "transaction_hash": transaction_hash,
        }),
    )
    .await
    .map_err(|err| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "chain broadcast succeeded but wallet-provider could not record transaction hash {transaction_hash}; retry Browser broadcast to record without rebroadcasting: {err}"
            ),
        )
    })
}

async fn browser_wallet_broadcast_transaction(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let status = browser_wallet_approval_status(state, authority, request_id).await?;
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
        remove_browser_pending_transaction_broadcast(state, context, request_id);
        return Ok(serde_json::json!({
            "schema": "elastos.browser.transaction-broadcast/v1",
            "request_id": request_id,
            "transaction_hash": transaction_hash,
            "already_recorded": true,
        }));
    }
    if let Some(pending) = read_browser_pending_transaction_broadcast(state, context, request_id)? {
        if let Some(status_chain_namespace) = status
            .get("signed_result")
            .and_then(|value| value.get("chain_namespace"))
            .and_then(|value| value.as_str())
        {
            if status_chain_namespace != pending.chain_namespace {
                return Err((
                    StatusCode::CONFLICT,
                    "Browser pending transaction broadcast does not match approval chain"
                        .to_string(),
                ));
            }
        }
        let recorded =
            record_browser_transaction_hash(state, context, request_id, &pending.transaction_hash)
                .await?;
        remove_browser_pending_transaction_broadcast(state, context, request_id);
        return Ok(serde_json::json!({
            "schema": "elastos.browser.transaction-broadcast/v1",
            "request_id": request_id,
            "transaction_hash": pending.transaction_hash,
            "recorded": true,
            "recovered_pending_broadcast": true,
            "receipt": pending.receipt,
            "approval_request": recorded.get("approval_request").cloned(),
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
    let pending = BrowserPendingTransactionBroadcast {
        schema: BROWSER_PENDING_TRANSACTION_BROADCAST_SCHEMA.to_string(),
        principal_id: context.principal_id.clone(),
        request_id: request_id.to_string(),
        chain_namespace: chain_namespace.to_string(),
        network: network.to_string(),
        transaction_hash: transaction_hash.to_string(),
        receipt: receipt.clone(),
        created_at: crate::auth::now_ts(),
    };
    write_browser_pending_transaction_broadcast(state, context, &pending)?;
    let recorded =
        record_browser_transaction_hash(state, context, request_id, transaction_hash).await?;
    remove_browser_pending_transaction_broadcast(state, context, request_id);
    Ok(serde_json::json!({
        "schema": "elastos.browser.transaction-broadcast/v1",
        "request_id": request_id,
        "transaction_hash": transaction_hash,
        "recorded": true,
        "receipt": receipt,
        "approval_request": recorded.get("approval_request").cloned(),
    }))
}
