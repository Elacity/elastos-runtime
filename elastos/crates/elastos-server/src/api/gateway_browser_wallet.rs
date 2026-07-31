//! Browser wallet bridge gateway helpers.

use super::*;
#[path = "gateway_browser_wallet_bridge.rs"]
mod gateway_browser_wallet_bridge;
#[path = "gateway_browser_wallet_reads.rs"]
mod gateway_browser_wallet_reads;

use gateway_browser_wallet_bridge::browser_wallet_account_is_signable_evm;
pub(in crate::api::gateway) use gateway_browser_wallet_bridge::{
    browser_chain_namespace_network, browser_wallet_bridge_payload, is_browser_wallet_intent,
};
use gateway_browser_wallet_bridge::{
    browser_projected_evm_accounts, BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES,
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
pub(in crate::api::gateway) struct BrowserWalletAccountAccessRequest {
    pub(in crate::api::gateway) chain_namespace: String,
    pub(in crate::api::gateway) page_url: String,
    pub(in crate::api::gateway) origin: String,
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

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserWalletApprovalStatusQuery {
    #[serde(default)]
    pub(in crate::api::gateway) page_url: Option<String>,
    #[serde(default)]
    pub(in crate::api::gateway) origin: Option<String>,
}

const BROWSER_ACCOUNT_ACCESS_TTL_SECS: u64 = 12 * 60 * 60;

fn validate_browser_wallet_page_origin(
    page_url: &str,
    origin: &str,
) -> Result<String, (StatusCode, String)> {
    let invalid_page = || {
        (
            StatusCode::BAD_REQUEST,
            "invalid browser page URL".to_string(),
        )
    };
    let invalid_origin = || {
        (
            StatusCode::BAD_REQUEST,
            "Browser wallet request origin must be an exact HTTP(S) origin".to_string(),
        )
    };
    let page = url::Url::parse(page_url).map_err(|_| invalid_page())?;
    if !matches!(page.scheme(), "http" | "https") || page.host_str().is_none() {
        return Err(invalid_page());
    }
    let parsed = url::Url::parse(origin).map_err(|_| invalid_origin())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(invalid_origin());
    }
    let canonical = parsed.origin().ascii_serialization();
    if origin != canonical {
        return Err(invalid_origin());
    }
    if canonical != page.origin().ascii_serialization() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser wallet request origin does not match page URL origin".to_string(),
        ));
    }
    Ok(canonical)
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

pub(in crate::api::gateway) async fn browser_app_wallet_request_accounts(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<BrowserWalletAccountAccessRequest>,
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
    let response =
        match create_browser_wallet_account_access_request(&state, &authority, input).await {
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
        match browser_wallet_broadcast_transaction(&state, &authority, &input.request_id).await {
            Ok(payload) => Json(payload).into_response(),
            Err((status, message)) => (status, message).into_response(),
        };
    browser_wallet_cors_response(&headers, response)
}

pub(in crate::api::gateway) async fn browser_app_wallet_approval_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Query(query): Query<BrowserWalletApprovalStatusQuery>,
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
    let response = match browser_wallet_approval_status(
        &state,
        &authority,
        &request_id,
        query.page_url.as_deref(),
        query.origin.as_deref(),
    )
    .await
    {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    };
    browser_wallet_cors_response(&headers, response)
}

async fn create_browser_wallet_account_access_request(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    input: BrowserWalletAccountAccessRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let context = authority.verified_context();
    let proof_binding_id = context.proof_binding_id().ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "Browser account access requires proof-bound launch authority".to_string(),
        )
    })?;
    let origin = validate_browser_wallet_page_origin(&input.page_url, &input.origin)?;
    if browser_chain_namespace_network(&input.chain_namespace).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser account access requires a Runtime-supported eip155 chain".to_string(),
        ));
    }
    let summary = system_wallet_accounts_summary(state, authority).await;
    let account = browser_projected_evm_accounts(&summary)
        .into_iter()
        .find(|account| {
            account.chain_namespace == input.chain_namespace
                && browser_wallet_account_is_signable_evm(account)
                && is_managed_wallet_proof_type(&account.proof_type)
                && account.connector_id.is_none()
        })
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "Browser account access requires a Runtime-managed EVM account for the selected chain"
                    .to_string(),
            )
        })?;
    let chain_namespaces = BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES
        .iter()
        .map(|namespace| (*namespace).to_string())
        .collect::<Vec<_>>();
    let grant_expires_at = now_ts().saturating_add(BROWSER_ACCOUNT_ACCESS_TTL_SECS);
    let payload = serde_json::json!({
        "schema": "elastos.browser.account-access-request/v1",
        "permission": "eth_accounts",
        "principal_id": context.principal_id(),
        "session_id": context.session_id(),
        "launch_id": context.launch_id(),
        "proof_binding_id": proof_binding_id,
        "origin": origin,
        "page_url": input.page_url,
        "account_id": account.account_id,
        "requested_chain_namespace": input.chain_namespace,
        "chain_namespaces": chain_namespaces,
        "address": account.address,
        "grant_expires_at": grant_expires_at,
        "requires_wallet_approval": true,
    });
    let data = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::RequestApproval {
            account_id: account.account_id.clone(),
            chain_namespace: input.chain_namespace.clone(),
            intent: "browser_account_access".to_string(),
            resource: format!(
                "elastos://wallet/account/{}/permission/eth_accounts",
                account.account_id
            ),
            reason: format!(
                "Browser origin {origin} requests access on Runtime-supported EVM networks: {}",
                BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES.join(", ")
            ),
            payload,
            expires_at: now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
    let approval = data
        .get("approval_request")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider did not create an account-access approval request".to_string(),
            )
        })?;
    let request_id = approval
        .get("request_id")
        .and_then(|value| value.as_str())
        .filter(|value| is_safe_runtime_id(value))
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider returned an invalid account-access approval id".to_string(),
            )
        })?;
    append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.wallet.account_access.requested",
            principal_id: context.principal_id(),
            session_id: context.session_id(),
            request_id,
            result: "pending",
            method: "eth_requestAccounts",
            resource: &input.chain_namespace,
            page_url: &input.page_url,
            origin: Some(&origin),
            decision: "wallet_review_required_before_disclosure",
        },
    )?;
    Ok(serde_json::json!({
        "schema": "elastos.browser.account-access-request-result/v1",
        "approval_request": {
            "request_id": request_id,
            "status": approval.get("status").cloned().unwrap_or(serde_json::json!("pending")),
            "expires_at": approval.get("expires_at").cloned().unwrap_or(serde_json::Value::Null),
        },
        "requires_approval": true,
    }))
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
    let stable_request = serde_json::json!({
        "method": method.clone(),
        "params": input.params.clone(),
        "account_id": account_id.clone(),
        "chain_namespace": chain_namespace.clone(),
        "address": address.to_ascii_lowercase(),
        "page_url": page_url.clone(),
        "origin": input.origin.clone(),
    });
    let request_sha256 = runtime_transaction_request_sha256(&stable_request)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let effect_id =
        runtime_transaction_effect_id(BROWSER_TRANSACTION_SOURCE, authority, &stable_request)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let approval = ensure_runtime_transaction_approval(
        state,
        authority,
        RuntimeTransactionRequest {
            source: BROWSER_TRANSACTION_SOURCE,
            effect_id,
            request_sha256,
            account_id: account.account_id.clone(),
            address: account.address.clone(),
            chain_namespace,
            network: network.to_string(),
            to: to.to_string(),
            value: value.to_string(),
            data: data.to_string(),
            approval_reason: format!("Browser page requests {method} on {network}"),
            metadata: serde_json::json!({
                "method": method,
                "page_url": page_url,
                "origin": input.origin,
                "principal_id": context.principal_id.clone(),
                "session_id": context.session_id.clone(),
            }),
        },
    )
    .await?;
    Ok(serde_json::json!({
        "schema": "elastos.browser.wallet-approval-result/v1",
        "requires_approval": true,
        "approval_request": approval.approval_request,
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

fn completed_browser_wallet_result_error(detail: &str) -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        format!("completed browser wallet approval {detail}"),
    )
}

fn completed_browser_wallet_result_string<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, (StatusCode, String)> {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| completed_browser_wallet_result_error(&format!("is missing {field}")))
}

fn browser_account_access_request_payload<'a>(
    request: &'a serde_json::Value,
    authority: &RuntimeWalletAuthority,
    page_url: Option<&str>,
    origin: Option<&str>,
) -> Result<&'a serde_json::Value, (StatusCode, String)> {
    let context = authority.verified_context();
    let page_url = page_url.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser account-access status requires the exact page URL".to_string(),
        )
    })?;
    let origin = origin.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser account-access status requires the exact page origin".to_string(),
        )
    })?;
    let canonical_origin = validate_browser_wallet_page_origin(page_url, origin)?;
    if !is_managed_wallet_proof_type(
        request
            .get("proof_type")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    ) || request
        .get("connector_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(completed_browser_wallet_result_error(
            "is not backed by a Runtime-managed signer",
        ));
    }
    for (field, expected) in [
        ("principal_id", context.principal_id()),
        ("session_id", context.session_id()),
        ("launch_id", context.launch_id()),
        ("requested_by_actor", BROWSER_CAPSULE_ID),
    ] {
        if request.get(field).and_then(|value| value.as_str()) != Some(expected) {
            return Err((
                StatusCode::NOT_FOUND,
                "browser wallet approval request not found".to_string(),
            ));
        }
    }
    let payload = request
        .get("payload")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            completed_browser_wallet_result_error("is missing account-access request payload")
        })?;
    if completed_browser_wallet_result_string(payload, "schema")?
        != "elastos.browser.account-access-request/v1"
        || completed_browser_wallet_result_string(payload, "permission")? != "eth_accounts"
        || payload
            .get("requires_wallet_approval")
            .and_then(|value| value.as_bool())
            != Some(true)
    {
        return Err(completed_browser_wallet_result_error(
            "has an invalid account-access request payload",
        ));
    }
    for (field, expected) in [
        ("principal_id", context.principal_id()),
        ("session_id", context.session_id()),
        ("launch_id", context.launch_id()),
        ("page_url", page_url),
        ("origin", canonical_origin.as_str()),
    ] {
        if completed_browser_wallet_result_string(payload, field)? != expected {
            return Err((
                StatusCode::NOT_FOUND,
                "browser wallet approval request not found".to_string(),
            ));
        }
    }
    let proof_binding_id = context.proof_binding_id().ok_or_else(|| {
        completed_browser_wallet_result_error("has no proof-bound Browser authority")
    })?;
    if completed_browser_wallet_result_string(payload, "proof_binding_id")? != proof_binding_id {
        return Err((
            StatusCode::NOT_FOUND,
            "browser wallet approval request not found".to_string(),
        ));
    }
    let account_id = completed_browser_wallet_result_string(request, "account_id")?;
    let chain_namespace = completed_browser_wallet_result_string(request, "chain_namespace")?;
    let address = completed_browser_wallet_result_string(request, "address")?;
    if completed_browser_wallet_result_string(payload, "account_id")? != account_id
        || completed_browser_wallet_result_string(payload, "requested_chain_namespace")?
            != chain_namespace
        || !completed_browser_wallet_result_string(payload, "address")?
            .eq_ignore_ascii_case(address)
        || !BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES.contains(&chain_namespace)
    {
        return Err(completed_browser_wallet_result_error(
            "has substituted account-access account authority",
        ));
    }
    let namespaces = payload
        .get("chain_namespaces")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            completed_browser_wallet_result_error("is missing account-access network set")
        })?;
    if namespaces.len() != BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES.len()
        || !namespaces
            .iter()
            .zip(BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES)
            .all(|(actual, expected)| actual.as_str() == Some(*expected))
    {
        return Err(completed_browser_wallet_result_error(
            "has an invalid account-access network set",
        ));
    }
    let now = now_ts();
    if !payload
        .get("grant_expires_at")
        .and_then(|value| value.as_u64())
        .is_some_and(|expires_at| {
            expires_at > now && expires_at <= now.saturating_add(BROWSER_ACCOUNT_ACCESS_TTL_SECS)
        })
    {
        return Err(completed_browser_wallet_result_error(
            "has an invalid or expired account-access grant",
        ));
    }
    Ok(payload)
}

fn validate_completed_browser_account_access_result(
    request_id: &str,
    request: &serde_json::Value,
    request_payload: &serde_json::Value,
    result: &serde_json::Value,
    authority: &RuntimeWalletAuthority,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let context = authority.verified_context();
    if completed_browser_wallet_result_string(result, "schema")?
        != "elastos.browser.account-access-result/v1"
        || completed_browser_wallet_result_string(result, "request_id")? != request_id
        || completed_browser_wallet_result_string(result, "permission")? != "eth_accounts"
    {
        return Err(completed_browser_wallet_result_error(
            "has an invalid account-access result envelope",
        ));
    }
    for (field, expected) in [
        ("principal_id", context.principal_id()),
        ("session_id", context.session_id()),
        ("launch_id", context.launch_id()),
    ] {
        if completed_browser_wallet_result_string(result, field)? != expected
            || completed_browser_wallet_result_string(request_payload, field)? != expected
        {
            return Err(completed_browser_wallet_result_error(&format!(
                "account-access result does not match its {field} authority"
            )));
        }
    }
    let proof_binding_id = context.proof_binding_id().ok_or_else(|| {
        completed_browser_wallet_result_error("has no proof-bound Browser authority")
    })?;
    if completed_browser_wallet_result_string(result, "proof_binding_id")? != proof_binding_id
        || completed_browser_wallet_result_string(request_payload, "proof_binding_id")?
            != proof_binding_id
    {
        return Err(completed_browser_wallet_result_error(
            "does not match its Browser proof binding",
        ));
    }
    for field in [
        "origin",
        "page_url",
        "account_id",
        "requested_chain_namespace",
    ] {
        if completed_browser_wallet_result_string(result, field)?
            != completed_browser_wallet_result_string(request_payload, field)?
        {
            return Err(completed_browser_wallet_result_error(&format!(
                "account-access result does not match its request {field}"
            )));
        }
    }
    if completed_browser_wallet_result_string(result, "account_id")?
        != completed_browser_wallet_result_string(request, "account_id")?
        || completed_browser_wallet_result_string(result, "requested_chain_namespace")?
            != completed_browser_wallet_result_string(request, "chain_namespace")?
        || !completed_browser_wallet_result_string(result, "address")?
            .eq_ignore_ascii_case(completed_browser_wallet_result_string(request, "address")?)
        || !completed_browser_wallet_result_string(result, "address")?.eq_ignore_ascii_case(
            completed_browser_wallet_result_string(request_payload, "address")?,
        )
        || completed_browser_wallet_result_string(result, "payload_hash")?
            != completed_browser_wallet_result_string(request, "payload_hash")?
    {
        return Err(completed_browser_wallet_result_error(
            "account-access result does not match its request account authority",
        ));
    }
    let result_namespaces = result
        .get("chain_namespaces")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            completed_browser_wallet_result_error("is missing account-access result network set")
        })?;
    let request_namespaces = request_payload
        .get("chain_namespaces")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            completed_browser_wallet_result_error("is missing account-access request network set")
        })?;
    if result_namespaces != request_namespaces {
        return Err(completed_browser_wallet_result_error(
            "account-access result does not match its request network set",
        ));
    }
    let grant_expires_at = result
        .get("grant_expires_at")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            completed_browser_wallet_result_error("is missing account-access grant expiry")
        })?;
    if request_payload
        .get("grant_expires_at")
        .and_then(|value| value.as_u64())
        != Some(grant_expires_at)
        || grant_expires_at <= now_ts()
    {
        return Err(completed_browser_wallet_result_error(
            "has an invalid or expired account-access grant",
        ));
    }
    Ok(serde_json::json!({
        "permission": "eth_accounts",
        "principal_id": context.principal_id(),
        "session_id": context.session_id(),
        "launch_id": context.launch_id(),
        "origin": completed_browser_wallet_result_string(result, "origin")?,
        "account_id": completed_browser_wallet_result_string(result, "account_id")?,
        "requested_chain_namespace": completed_browser_wallet_result_string(result, "requested_chain_namespace")?,
        "chain_namespaces": result_namespaces,
        "address": completed_browser_wallet_result_string(result, "address")?,
        "grant_expires_at": grant_expires_at,
    }))
}

async fn browser_wallet_approval_status(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    page_url: Option<&str>,
    origin: Option<&str>,
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
                && request
                    .get("requested_by_actor")
                    .or_else(|| request.get("capsule_id"))
                    .and_then(|value| value.as_str())
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
    let intent = request.get("intent").and_then(|value| value.as_str());
    let account_access_payload = if intent == Some("browser_account_access") {
        Some(browser_account_access_request_payload(
            request, authority, page_url, origin,
        )?)
    } else {
        None
    };
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
        if intent == Some("browser_account_access") {
            payload["account_access"] = validate_completed_browser_account_access_result(
                request_id,
                request,
                account_access_payload.expect("account access payload validated above"),
                result,
                authority,
            )?;
        } else if matches!(
            intent,
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
            let broadcast_recorded = request.get("validated_chain_outcome").is_some();
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
        if intent != Some("browser_account_access") {
            payload["signed_result"] = result.clone();
        }
    }
    Ok(payload)
}

async fn browser_wallet_broadcast_transaction(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let completion = complete_runtime_transaction_effect(
        state,
        authority,
        RuntimeTransactionLookup::ApprovalId(request_id),
        None,
    )
    .await?;
    Ok(serde_json::json!({
        "schema": "elastos.browser.transaction-broadcast/v1",
        "request_id": completion.approval_request_id,
        "transaction_hash": completion.transaction_hash,
        "recorded": !completion.externally_completed,
        "already_recorded": completion.already_confirmed,
        "receipt": completion.receipt,
        "approval_request": completion.approval_request,
        "completion_status": if completion.completion_pending { "pending" } else { "complete" },
        "completion_error": completion.completion_error,
    }))
}
