#[cfg(test)]
use std::cell::RefCell;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use super::*;

fn hosted_setup_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn discard_staged_setup(data_dir: &std::path::Path, id: &str) -> anyhow::Result<()> {
    let _guard = hosted_setup_gate().lock().await;
    crate::api::model_provider_config::discard_staged_hosted_key(data_dir, id)
}

const HOSTED_EXTERNAL_HTTPS_PAUSED: &str =
    "Hosted external HTTPS is paused until Runtime network authority is available.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_PENDING: &str =
    "Approve this hosted connection in Inbox, then check the key again.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_DENIED: &str =
    "The hosted connection request was denied. Try again after the decision window.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_ACCESS_CLOSED: &str = "Hosted access was denied or ended. Review Inbox.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_ENDED: &str =
    "Hosted HTTPS was ended on this Home. Start a new connection check in Inbox.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_ROUTE_BLOCKED: &str =
    "Runtime blocked this hosted route. Review the connection configuration.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_CHECK_FAILED: &str =
    "This Home could not check the hosted connection. Try again.";
#[cfg(any(test, target_os = "macos"))]
const HOSTED_CONNECTION_NETWORK_FAILED: &str =
    "Hosted HTTPS could not reach the host. Check the network and try again.";

#[cfg(any(test, target_os = "macos"))]
fn hosted_transport_error(err: anyhow::Error) -> anyhow::Error {
    let message = match err.to_string().as_str() {
        "hosted egress requires an Inbox decision" => HOSTED_CONNECTION_PENDING,
        "hosted connection request was denied" => HOSTED_CONNECTION_DENIED,
        "hosted egress request was denied or ended" => HOSTED_ACCESS_CLOSED,
        "owner ended hosted HTTPS" => HOSTED_CONNECTION_ENDED,
        "hosted validation destination unavailable"
        | "hosted validation redirect denied"
        | "hosted destination unavailable" => HOSTED_CONNECTION_ROUTE_BLOCKED,
        _ if err.downcast_ref::<reqwest::Error>().is_some()
            || err.downcast_ref::<std::io::Error>().is_some()
            || err.downcast_ref::<tokio::time::error::Elapsed>().is_some() =>
        {
            HOSTED_CONNECTION_NETWORK_FAILED
        }
        _ => HOSTED_CONNECTION_CHECK_FAILED,
    };
    ai_provider_request_error(message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AiProviderRequestError(&'static str);

impl std::fmt::Display for AiProviderRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for AiProviderRequestError {}

fn ai_provider_request_error(message: &'static str) -> anyhow::Error {
    AiProviderRequestError(message).into()
}

pub(super) fn ai_provider_request_message(err: &anyhow::Error) -> Option<&'static str> {
    err.downcast_ref::<AiProviderRequestError>()
        .map(|error| error.0)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AiProviderValidateRequest {
    provider: String,
    api_key: String,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AiProviderSaveRequest {
    provider: String,
    api_key: String,
    model: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AiProviderShareRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    enabled: bool,
    #[serde(default)]
    terms_ack: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AiProviderDeleteRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DiscoveredModel {
    id: String,
    #[serde(skip_serializing)]
    expected_response_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    privacy: Option<String>,
}

#[derive(Debug, Serialize)]
struct AiProviderValidateResponse {
    valid: bool,
    models: Vec<DiscoveredModel>,
}

#[cfg(test)]
thread_local! {
    static OPENROUTER_MODELS_DOUBLE: RefCell<Option<OpenRouterModelsDouble>> = const { RefCell::new(None) };
    static VENICE_AUTH_DOUBLE: RefCell<Option<VeniceAuthDouble>> = const { RefCell::new(None) };
    static VENICE_MODELS_DOUBLE: RefCell<Option<Vec<DiscoveredModel>>> = const { RefCell::new(None) };
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) enum OpenRouterModelsDouble {
    InvalidKey,
    Models(Vec<String>),
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) enum VeniceAuthDouble {
    InvalidKey,
    Denied,
    Permitted,
}

#[cfg(test)]
pub(crate) fn install_openrouter_models_double(double: OpenRouterModelsDouble) {
    OPENROUTER_MODELS_DOUBLE.with(|slot| *slot.borrow_mut() = Some(double));
}

#[cfg(test)]
pub(crate) fn install_venice_validate_double(
    auth: VeniceAuthDouble,
    models: Option<Vec<(String, Option<String>)>>,
) {
    VENICE_AUTH_DOUBLE.with(|slot| *slot.borrow_mut() = Some(auth));
    VENICE_MODELS_DOUBLE.with(|slot| {
        *slot.borrow_mut() = models.map(|models| {
            models
                .into_iter()
                .map(|(id, privacy)| DiscoveredModel {
                    id,
                    privacy,
                    expected_response_model: None,
                })
                .collect()
        });
    });
}

#[cfg(test)]
pub(crate) fn clear_hosted_ai_validate_doubles() {
    OPENROUTER_MODELS_DOUBLE.with(|slot| *slot.borrow_mut() = None);
    VENICE_AUTH_DOUBLE.with(|slot| *slot.borrow_mut() = None);
    VENICE_MODELS_DOUBLE.with(|slot| *slot.borrow_mut() = None);
}

fn require_system_admin(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let context = require_home_launch_token_context(data_dir, headers, SYSTEM_CAPSULE_ID)?;
    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        return Err(anyhow::anyhow!("admin passkey required"));
    };
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if !crate::auth::is_admin(&principal) {
        return Err(anyhow::anyhow!("admin passkey required"));
    }
    Ok(context)
}

fn parse_provider(value: &str) -> anyhow::Result<crate::api::HostedAiProvider> {
    crate::api::HostedAiProvider::parse(value)
        .map_err(|_| ai_provider_request_error("invalid AI provider"))
}

fn invalid_key(provider: crate::api::HostedAiProvider) -> anyhow::Error {
    match provider {
        crate::api::HostedAiProvider::OpenRouter => {
            ai_provider_request_error("invalid OpenRouter key")
        }
        crate::api::HostedAiProvider::Venice => ai_provider_request_error("invalid Venice key"),
    }
}

fn invalid_model(provider: crate::api::HostedAiProvider) -> anyhow::Error {
    match provider {
        crate::api::HostedAiProvider::OpenRouter => {
            ai_provider_request_error("invalid OpenRouter model")
        }
        crate::api::HostedAiProvider::Venice => ai_provider_request_error("invalid Venice model"),
    }
}

fn normalize_secret(provider: crate::api::HostedAiProvider, value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid_key(provider));
    }
    Ok(trimmed.to_string())
}

fn normalize_model(provider: crate::api::HostedAiProvider, value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid_model(provider));
    }
    Ok(trimmed.to_string())
}

fn parse_openrouter_models_body(bytes: &[u8]) -> anyhow::Result<Vec<DiscoveredModel>> {
    let payload: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ai_provider_request_error("invalid OpenRouter key"))?;
    let Some(data) = payload.get("data").and_then(serde_json::Value::as_array) else {
        return Err(ai_provider_request_error("invalid OpenRouter key"));
    };
    let mut models = Vec::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let id = id.trim();
        // Discovery admits only the operations implemented by this provider.
        let outputs = entry
            .pointer("/architecture/output_modalities")
            .and_then(serde_json::Value::as_array);
        let decision = outputs
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("decisions")));
        let text =
            outputs.is_some_and(|items| items.iter().any(|item| item.as_str() == Some("text")));
        let pinned_jev = id.starts_with("typesafe/jev-");
        let canonical = entry
            .get("canonical_slug")
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                !value.is_empty()
                    && value.trim() == *value
                    && value.len() <= 256
                    && !value.starts_with('~')
            });
        if !id.is_empty()
            && !id.starts_with('~')
            && ((decision && pinned_jev && canonical.is_some())
                || (text && !decision && !pinned_jev))
        {
            models.push(DiscoveredModel {
                id: id.to_string(),
                expected_response_model: if decision {
                    canonical.map(ToOwned::to_owned)
                } else {
                    None
                },
                privacy: None,
            });
        }
    }
    Ok(models)
}

fn parse_venice_rate_limits_body(bytes: &[u8]) -> anyhow::Result<bool> {
    let payload: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ai_provider_request_error("invalid Venice key"))?;
    Ok(payload
        .get("data")
        .and_then(|data| data.get("accessPermitted"))
        .and_then(serde_json::Value::as_bool)
        == Some(true))
}

fn parse_venice_models_body(bytes: &[u8]) -> anyhow::Result<Vec<DiscoveredModel>> {
    let payload: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ai_provider_request_error("invalid Venice key"))?;
    let Some(data) = payload.get("data").and_then(serde_json::Value::as_array) else {
        return Err(ai_provider_request_error("invalid Venice key"));
    };
    let mut models = Vec::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        let privacy = entry
            .get("model_spec")
            .and_then(|spec| spec.get("privacy"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        models.push(DiscoveredModel {
            id: id.to_string(),
            privacy,
            expected_response_model: None,
        });
    }
    Ok(models)
}

async fn fetch_openrouter_models(
    data_dir: &std::path::Path,
    api_key: &str,
    owner_proof_binding_id: &str,
    connection_id: &str,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    #[cfg(test)]
    let _ = owner_proof_binding_id;
    #[cfg(test)]
    {
        let _ = (data_dir, connection_id);
        let _ = api_key;
        OPENROUTER_MODELS_DOUBLE.with(|slot| match slot.borrow().as_ref() {
            Some(OpenRouterModelsDouble::InvalidKey) => {
                Err(ai_provider_request_error("invalid OpenRouter key"))
            }
            Some(OpenRouterModelsDouble::Models(models)) => Ok(models
                .iter()
                .map(|id| DiscoveredModel {
                    id: id.clone(),
                    expected_response_model: id.starts_with("typesafe/jev-").then(|| id.clone()),
                    privacy: None,
                })
                .collect()),
            None => Err(anyhow::anyhow!("OpenRouter validation transport unset")),
        })
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let (status, bytes) = crate::api::model_provider_egress::fetch_validation_for_connection(
            data_dir,
            crate::api::model_provider_egress::ValidationEndpoint::OpenRouterModels,
            api_key,
            owner_proof_binding_id,
            connection_id,
        )
        .await
        .map_err(hosted_transport_error)?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ai_provider_request_error("invalid OpenRouter key"));
        }
        if !status.is_success() {
            return Err(anyhow::anyhow!("OpenRouter validation unavailable"));
        }
        parse_openrouter_models_body(&bytes)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = (data_dir, api_key, owner_proof_binding_id, connection_id);
        Err(ai_provider_request_error(HOSTED_EXTERNAL_HTTPS_PAUSED))
    }
}

async fn fetch_venice_access(
    data_dir: &std::path::Path,
    api_key: &str,
    owner_proof_binding_id: &str,
    connection_id: &str,
) -> anyhow::Result<bool> {
    #[cfg(test)]
    let _ = owner_proof_binding_id;
    #[cfg(test)]
    {
        let _ = (data_dir, connection_id);
        let _ = api_key;
        VENICE_AUTH_DOUBLE.with(|slot| match slot.borrow().as_ref() {
            Some(VeniceAuthDouble::InvalidKey) => {
                Err(ai_provider_request_error("invalid Venice key"))
            }
            Some(VeniceAuthDouble::Denied) => Ok(false),
            Some(VeniceAuthDouble::Permitted) => Ok(true),
            None => Err(anyhow::anyhow!("Venice validation transport unset")),
        })
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let (status, bytes) = crate::api::model_provider_egress::fetch_validation_for_connection(
            data_dir,
            crate::api::model_provider_egress::ValidationEndpoint::VeniceAccess,
            api_key,
            owner_proof_binding_id,
            connection_id,
        )
        .await
        .map_err(hosted_transport_error)?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ai_provider_request_error("invalid Venice key"));
        }
        if !status.is_success() {
            return Err(anyhow::anyhow!("Venice validation unavailable"));
        }
        parse_venice_rate_limits_body(&bytes)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = (data_dir, api_key, owner_proof_binding_id, connection_id);
        Err(ai_provider_request_error(HOSTED_EXTERNAL_HTTPS_PAUSED))
    }
}

async fn fetch_venice_models(
    data_dir: &std::path::Path,
    api_key: &str,
    owner_proof_binding_id: &str,
    connection_id: &str,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    #[cfg(test)]
    let _ = owner_proof_binding_id;
    #[cfg(test)]
    {
        let _ = (data_dir, connection_id);
        let _ = api_key;
        VENICE_MODELS_DOUBLE.with(|slot| match slot.borrow().as_ref() {
            Some(models) => Ok(models.clone()),
            None => Err(anyhow::anyhow!("Venice models transport unset")),
        })
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let (status, bytes) = crate::api::model_provider_egress::fetch_validation_for_connection(
            data_dir,
            crate::api::model_provider_egress::ValidationEndpoint::VeniceModels,
            api_key,
            owner_proof_binding_id,
            connection_id,
        )
        .await
        .map_err(hosted_transport_error)?;
        if !status.is_success() {
            return Err(anyhow::anyhow!("Venice validation unavailable"));
        }
        parse_venice_models_body(&bytes)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = (data_dir, api_key, owner_proof_binding_id, connection_id);
        Err(ai_provider_request_error(HOSTED_EXTERNAL_HTTPS_PAUSED))
    }
}

async fn validate_hosted_key(
    data_dir: &std::path::Path,
    provider: crate::api::HostedAiProvider,
    api_key: &str,
    owner_proof_binding_id: &str,
    connection_id: &str,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    let api_key = normalize_secret(provider, api_key)?;
    match provider {
        crate::api::HostedAiProvider::OpenRouter => {
            fetch_openrouter_models(data_dir, &api_key, owner_proof_binding_id, connection_id).await
        }
        crate::api::HostedAiProvider::Venice => {
            if !fetch_venice_access(data_dir, &api_key, owner_proof_binding_id, connection_id)
                .await?
            {
                return Err(invalid_key(provider));
            }
            fetch_venice_models(data_dir, &api_key, owner_proof_binding_id, connection_id).await
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ApprovalLensRequest {
    id: String,
}

pub(super) async fn system_approval_lens_select(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<ApprovalLensRequest>,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    match crate::api::model_provider_config::select_approval_lens(&state.data_dir, &req.id) {
        Ok(()) => Json(serde_json::json!({ "offer_id": req.id })).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_approval_lens_revoke(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<ApprovalLensRequest>,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    #[cfg(unix)]
    let ended_https =
        match crate::api::model_provider_egress_decision::end_offer(&state.data_dir, &req.id) {
            Ok(count) => count,
            Err(err) => return system_error_response(err),
        };
    #[cfg(not(unix))]
    let ended_https = 0;
    #[cfg(target_os = "macos")]
    let ended_operator = match crate::api::model_provider_egress::end_operator_hosted_access(
        &state.data_dir,
        &req.id,
    ) {
        Ok(ended) => ended,
        Err(err) => return system_error_response(err),
    };
    #[cfg(not(target_os = "macos"))]
    let ended_operator = false;
    let jev_active = crate::jev_approval_lens::approved_connection(&state.data_dir, &req.id);
    if jev_active {
        if let Err(err) =
            crate::jev_approval_lens::end_connection_approval(&state.data_dir, &req.id)
        {
            return system_error_response(err);
        }
    }
    if ended_https == 0 && !ended_operator && !jev_active {
        return system_error_response(anyhow::anyhow!("hosted connection has no active approval"));
    }
    Json(serde_json::json!({ "offer_id": req.id, "approval": "ended" })).into_response()
}

pub(super) async fn system_ai_provider_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    match crate::api::ai_provider_status(&state.data_dir) {
        Ok(status) => {
            let mut result = serde_json::to_value(status).expect("serializable provider status");
            #[cfg(target_os = "macos")]
            let operator_enabled =
                crate::api::model_provider_egress::operator_hosted_access_enabled(&state.data_dir);
            #[cfg(not(target_os = "macos"))]
            let operator_enabled = false;
            result["hosted_external_https"] = serde_json::json!(if operator_enabled {
                "operator_ready"
            } else if cfg!(target_os = "macos") {
                "consent_required"
            } else {
                "paused"
            });
            result["hosted_external_https_reason"] = serde_json::json!(if operator_enabled {
                "Owner-authorized hosted HTTPS is active for configured providers."
            } else if cfg!(target_os = "macos") {
                "Approve each hosted connection in Inbox before its key check or model request."
            } else {
                HOSTED_EXTERNAL_HTTPS_PAUSED
            });
            if let Some(connections) = result
                .get_mut("connections")
                .and_then(serde_json::Value::as_array_mut)
            {
                for connection in connections {
                    connection["egress_state"] = serde_json::json!("paused");
                    let Some(id) = connection
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                    else {
                        continue;
                    };
                    #[cfg(target_os = "macos")]
                    if crate::api::model_provider_egress::operator_hosted_offer_ready(
                        &state.data_dir,
                        &id,
                    ) {
                        connection["egress_state"] = serde_json::json!("operator_ready");
                    }
                    #[cfg(target_os = "macos")]
                    match crate::api::model_provider_egress::saved_connection_state(
                        &state.data_dir,
                        &id,
                    ) {
                        Ok(egress_state) => {
                            connection["egress_approval_state"] = serde_json::json!(egress_state);
                            if egress_state == "approved" {
                                connection["egress_state"] = serde_json::json!("ready");
                            }
                        }
                        Err(_) => {
                            connection["egress_approval_state"] = serde_json::json!("unavailable")
                        }
                    }
                    if crate::jev_approval_lens::approved_connection(&state.data_dir, &id) {
                        connection["approval_state"] = serde_json::json!("approved");
                    }
                }
            }
            #[cfg(unix)]
            {
                let saved = result["connections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|connection| connection["id"].as_str())
                    .collect::<std::collections::HashSet<_>>();
                let staged_connections =
                    match crate::api::model_provider_config::staged_hosted_connections(
                        &state.data_dir,
                    ) {
                        Ok(connections) => connections,
                        Err(err) => return system_error_response(err),
                    };
                let staged = staged_connections
                    .into_iter()
                    .map(|(id, provider)| {
                        let has_saved_model = saved.contains(id.as_str());
                        serde_json::json!({
                            "id": id,
                            "provider": provider,
                            "status": "needs_key_check",
                            "has_saved_model": has_saved_model,
                        })
                    })
                    .collect::<Vec<_>>();
                result["staged_connections"] = serde_json::json!(staged);
            }
            #[cfg(unix)]
            {
                result["validation_egress_approval_state"] = serde_json::json!({});
                for provider in ["openrouter", "venice"] {
                    let offer_id = format!("validation:{provider}");
                    let state_value = crate::api::model_provider_egress_decision::offer_state(
                        &state.data_dir,
                        &offer_id,
                    )
                    .unwrap_or("unavailable");
                    result["validation_egress_approval_state"][provider] =
                        serde_json::json!(state_value);
                }
            }
            match crate::api::model_provider_config::approval_lens_offer_id(&state.data_dir) {
                Ok(id) => {
                    result["approval_lens_offer_id"] = serde_json::json!(id);
                }
                Err(_) => {
                    result["approval_lens_error"] = serde_json::json!(true);
                }
            }
            Json(result).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_ai_provider_validate(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AiProviderValidateRequest>,
) -> Response {
    let admin = match require_system_admin(&state.data_dir, &headers) {
        Ok(admin) => admin,
        Err(err) => return system_error_response(err),
    };
    if !cfg!(test) && !cfg!(target_os = "macos") {
        return system_error_response(ai_provider_request_error(HOSTED_EXTERNAL_HTTPS_PAUSED));
    }
    let provider = match parse_provider(&req.provider) {
        Ok(provider) => provider,
        Err(err) => return system_error_response(err),
    };
    let connection_id = req
        .id
        .clone()
        .unwrap_or_else(|| format!("model:hosted-{:032x}", rand::random::<u128>()));
    let _setup = hosted_setup_gate().lock().await;
    let api_key = match crate::api::model_provider_config::hosted_key_for_save(
        &state.data_dir,
        provider,
        Some(&connection_id),
        &req.api_key,
    ) {
        Ok(key) => key,
        Err(err) => return system_error_response(err),
    };
    let api_key = match normalize_secret(provider, &api_key) {
        Ok(key) => key,
        Err(err) => return system_error_response(err),
    };
    if let Err(err) = crate::api::model_provider_config::stage_hosted_key(
        &state.data_dir,
        provider,
        &connection_id,
        &api_key,
    ) {
        return system_error_response(err);
    }
    match validate_hosted_key(
        &state.data_dir,
        provider,
        &api_key,
        admin.proof_binding_id.as_deref().unwrap_or_default(),
        &connection_id,
    )
    .await
    {
        Ok(models) => Json(AiProviderValidateResponse {
            valid: true,
            models,
        })
        .into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_ai_provider_save(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AiProviderSaveRequest>,
) -> Response {
    let admin = match require_system_admin(&state.data_dir, &headers) {
        Ok(admin) => admin,
        Err(err) => return system_error_response(err),
    };
    if !cfg!(test) && !cfg!(target_os = "macos") {
        return system_error_response(ai_provider_request_error(HOSTED_EXTERNAL_HTTPS_PAUSED));
    }
    let provider = match parse_provider(&req.provider) {
        Ok(provider) => provider,
        Err(err) => return system_error_response(err),
    };
    let connection_id = req
        .id
        .clone()
        .unwrap_or_else(|| format!("model:hosted-{:032x}", rand::random::<u128>()));
    let _setup = hosted_setup_gate().lock().await;
    let api_key = match if req.api_key.trim().is_empty() {
        crate::api::model_provider_config::hosted_key_for_save(
            &state.data_dir,
            provider,
            Some(&connection_id),
            &req.api_key,
        )
    } else {
        normalize_secret(provider, &req.api_key)
    } {
        Ok(api_key) => api_key,
        Err(err) => return system_error_response(err),
    };
    let model = match normalize_model(provider, &req.model) {
        Ok(model) => model,
        Err(err) => return system_error_response(err),
    };
    if let Err(err) = crate::api::model_provider_config::stage_hosted_key(
        &state.data_dir,
        provider,
        &connection_id,
        &api_key,
    ) {
        return system_error_response(err);
    }
    let models = match validate_hosted_key(
        &state.data_dir,
        provider,
        &api_key,
        admin.proof_binding_id.as_deref().unwrap_or_default(),
        &connection_id,
    )
    .await
    {
        Ok(models) => models,
        Err(err) => return system_error_response(err),
    };
    let Some(selected) = models.iter().find(|entry| entry.id == model) else {
        return system_error_response(invalid_model(provider));
    };
    let name = req.name.clone().unwrap_or_default();
    let _model_share_guard = super::gateway_model_service::model_share_gate()
        .write()
        .await;
    match crate::api::save_hosted_offer(
        &state.data_dir,
        state.provider_registry.as_deref(),
        crate::api::model_provider_config::HostedOfferSave {
            provider,
            api_key: &api_key,
            model: &model,
            expected_response_model: selected.expected_response_model.as_deref(),
            privacy: selected.privacy.as_deref(),
            name: &name,
            instance_id: Some(&connection_id),
        },
    )
    .await
    {
        Ok(status) => Json(status).into_response(),
        Err(err) => system_error_response(err),
    }
}

fn resolve_hosted_instance_id(
    data_dir: &std::path::Path,
    id: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(id) = id.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(id.to_string());
    }
    let provider = parse_provider(provider.unwrap_or_default())?;
    let status = crate::api::ai_provider_status(data_dir)?;
    let matches = status
        .connections
        .into_iter()
        .filter(|connection| connection.provider == provider.as_str())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [connection] => Ok(connection.id.clone()),
        [] => Err(anyhow::anyhow!("hosted connection is required")),
        _ => Err(anyhow::anyhow!("hosted instance id is required")),
    }
}

pub(super) async fn system_ai_provider_delete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AiProviderDeleteRequest>,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    let offer_id = match resolve_hosted_instance_id(
        &state.data_dir,
        req.id.as_deref(),
        req.provider.as_deref(),
    ) {
        Ok(offer_id) => offer_id,
        Err(err) => return system_error_response(err),
    };
    #[cfg(unix)]
    if let Err(err) =
        crate::api::model_provider_egress_decision::end_offer(&state.data_dir, &offer_id)
    {
        return system_error_response(err);
    }
    let _model_share_guard = super::gateway_model_service::model_share_gate()
        .write()
        .await;
    match crate::api::remove_hosted_offer(
        &state.data_dir,
        state.provider_registry.as_deref(),
        &offer_id,
    )
    .await
    {
        Ok(status) => Json(status).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_ai_provider_discard_staged(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AiProviderDeleteRequest>,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    let Some(id) = req.id.as_deref() else {
        return system_error_response(anyhow::anyhow!("staged hosted connection id required"));
    };
    match discard_staged_setup(&state.data_dir, id).await {
        Ok(()) => Json(serde_json::json!({"discarded": true})).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_ai_provider_share(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AiProviderShareRequest>,
) -> Response {
    if let Err(err) = require_system_admin(&state.data_dir, &headers) {
        return system_error_response(err);
    }
    let offer_id = match resolve_hosted_instance_id(
        &state.data_dir,
        req.id.as_deref(),
        req.provider.as_deref(),
    ) {
        Ok(offer_id) => offer_id,
        Err(err) => return system_error_response(err),
    };
    let _model_share_guard = super::gateway_model_service::model_share_gate()
        .write()
        .await;
    match crate::api::set_hosted_offer_share(
        &state.data_dir,
        &offer_id,
        req.enabled,
        req.terms_ack.as_deref(),
    ) {
        Ok(status) => Json(status).into_response(),
        Err(err) => system_error_response(err),
    }
}

#[cfg(test)]
mod parse_tests {
    #[tokio::test]
    async fn hosted_validation_errors_keep_decision_and_network_outcomes_without_details() {
        let mapped = |detail| {
            let err = super::hosted_transport_error(anyhow::anyhow!("{detail}"));
            super::ai_provider_request_message(&err)
                .unwrap()
                .to_string()
        };
        assert_eq!(
            mapped("hosted egress requires an Inbox decision"),
            super::HOSTED_CONNECTION_PENDING
        );
        assert_eq!(
            mapped("hosted connection request was denied"),
            super::HOSTED_CONNECTION_DENIED
        );
        assert_eq!(
            mapped("hosted egress request was denied or ended"),
            super::HOSTED_ACCESS_CLOSED
        );
        assert_eq!(
            mapped("owner ended hosted HTTPS"),
            super::HOSTED_CONNECTION_ENDED
        );
        assert_eq!(
            mapped("hosted validation destination unavailable"),
            super::HOSTED_CONNECTION_ROUTE_BLOCKED
        );
        assert_eq!(
            mapped("private URL and key"),
            super::HOSTED_CONNECTION_CHECK_FAILED
        );
        let network = super::hosted_transport_error(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "private host").into(),
        );
        assert_eq!(
            super::ai_provider_request_message(&network),
            Some(super::HOSTED_CONNECTION_NETWORK_FAILED)
        );
        assert!(!network.to_string().contains("private host"));
        let elapsed = tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err();
        let timeout = super::hosted_transport_error(elapsed.into());
        assert_eq!(
            super::ai_provider_request_message(&timeout),
            Some(super::HOSTED_CONNECTION_NETWORK_FAILED)
        );
    }

    #[tokio::test]
    async fn staged_discard_waits_for_inflight_setup_and_ends_its_pending_decision() {
        use crate::api::model_provider_egress_decision::{ConnectionScope, EgressScope};

        let dir = tempfile::tempdir().unwrap();
        let id = "model:hosted-0123456789abcdef0123456789abcdef";
        let guard = super::hosted_setup_gate().lock().await;
        crate::api::model_provider_config::stage_hosted_key(
            dir.path(),
            crate::api::HostedAiProvider::OpenRouter,
            id,
            "fixture-key",
        )
        .unwrap();
        let mut discard = Box::pin(super::discard_staged_setup(dir.path(), id));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut discard)
                .await
                .is_err()
        );
        let scope = EgressScope {
            offer_id: id.into(),
            effect: "validate_models".into(),
            method: "GET".into(),
            url: "https://example.com/models".into(),
            origin: "https://example.com".into(),
            recipient: "example.com".into(),
            payer: "this Home".into(),
            provider: "OpenRouter".into(),
            purpose: "Load hosted model choices".into(),
            configuration_id: "a".repeat(64),
        };
        let connection = ConnectionScope {
            version: 1,
            offer_id: id.into(),
            provider: "OpenRouter".into(),
            origin: "https://example.com".into(),
            revision: "b".repeat(64),
        };
        let pending = crate::api::model_provider_egress_decision::request_connection(
            dir.path(),
            &scope,
            &connection,
            None,
        )
        .unwrap();
        drop(guard);
        discard.await.unwrap();
        assert!(
            crate::api::model_provider_config::staged_hosted_connections(dir.path())
                .unwrap()
                .is_empty()
        );
        let history = serde_json::to_value(
            crate::api::model_provider_egress_decision::inbox_history(dir.path()).unwrap(),
        )
        .unwrap();
        assert_eq!(history[0]["id"], pending);
        assert_eq!(history[0]["status"], "ended");
    }

    #[test]
    fn decision_catalog_binds_exact_canonical_identity_and_requires_metadata() {
        let models = super::parse_openrouter_models_body(br#"{"data":[
            {"id":"typesafe/jev-1.13","canonical_slug":"typesafe/jev-1.13-20260917","architecture":{"output_modalities":["decisions"]}},
            {"id":"typesafe/jev-1.12","architecture":{"output_modalities":["decisions"]}},
            {"id":"typesafe/jev-1.14","canonical_slug":"~typesafe/jev-latest","architecture":{"output_modalities":["decisions"]}}
        ]}"#).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "typesafe/jev-1.13");
        assert_eq!(
            models[0].expected_response_model.as_deref(),
            Some("typesafe/jev-1.13-20260917")
        );
    }

    #[test]
    fn parse_openrouter_models_body_reads_ids() {
        assert_eq!(
            super::parse_openrouter_models_body(
                br#"{"data":[
                    {"id":"fixture/model","architecture":{"output_modalities":["text"]}},
                    {"id":"typesafe/jev-1.13","canonical_slug":"typesafe/jev-1.13-20260917","architecture":{"output_modalities":["decisions"]}},
                    {"id":"~typesafe/jev-latest","architecture":{"output_modalities":["decisions"]}},
                    {"id":"other/decision","architecture":{"output_modalities":["decisions"]}},
                    {"id":"image/only","architecture":{"output_modalities":["image"]}},
                    {"id":"audio/only","architecture":{"output_modalities":["audio"]}},
                    {"id":"embedding/only","architecture":{"output_modalities":["embeddings"]}},
                    {"id":"unknown/modalities"},
                    {"id":"  ","architecture":{"output_modalities":["text"]}}
                ]}"#
            )
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>(),
            vec!["fixture/model".to_string(), "typesafe/jev-1.13".to_string()]
        );
    }

    #[test]
    fn parse_venice_rate_limits_requires_access_permitted() {
        assert!(
            super::parse_venice_rate_limits_body(br#"{"data":{"accessPermitted":true}}"#).unwrap()
        );
        assert!(
            !super::parse_venice_rate_limits_body(br#"{"data":{"accessPermitted":false}}"#)
                .unwrap()
        );
    }

    #[test]
    fn parse_venice_models_body_keeps_privacy() {
        let models = super::parse_venice_models_body(
            br#"{"data":[{"id":"fixture/model","model_spec":{"privacy":"private"}},{"id":"  "}]}"#,
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "fixture/model");
        assert_eq!(models[0].privacy.as_deref(), Some("private"));
    }
}
