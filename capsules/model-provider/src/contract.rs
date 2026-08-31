use crate::config::BridgeProviderConfig;
use anyhow::Result;
pub use elastos_model_contract::{
    model_input_hash, validate_input_hash, validate_run_id, RuntimeAccessBinding,
    RuntimeCreateBinding,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROVIDER_ID: &str = "model-provider";
pub const PROVIDER_PROTOCOL_VERSION: &str = "elastos.model-provider/v1";
pub const OFFERS_LIST_SCHEMA: &str = "elastos.model.offers-list/v1";
pub const RUN_SCHEMA: &str = "elastos.model.run/v1";
pub const RUN_EVENTS_SCHEMA: &str = "elastos.model.run-events/v1";
pub const RUN_EVENT_SCHEMA: &str = "elastos.model.run-event/v1";
pub const MODEL_POLICY_SCHEMA: &str = "elastos.model.policy/v1";
pub const RUN_OUTPUT_TEXT_SCHEMA: &str = "elastos.model.output.text/v1";
pub const RUN_OUTPUT_OBJECT_SCHEMA: &str = "elastos.model.output.object/v1";
pub const RUN_OUTPUT_CONTENT_SCHEMA: &str = "elastos.model.output.content/v1";
pub const MAX_EVENT_SEQUENCE: u64 = 9_007_199_254_740_991;

#[derive(Debug)]
pub struct ProviderEnvelope {
    pub operation: ProviderOperation,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOperation {
    Init,
    Status,
    Shutdown,
    OffersList,
    RunsCreate,
    RunsGet,
    RunsEvents,
    RunsCancel,
    Unsupported(String),
}

pub fn parse_request(line: &str) -> std::result::Result<ProviderEnvelope, ProviderFault> {
    let value = serde_json::from_str::<Value>(line)
        .map_err(|_| ProviderFault::invalid_request("invalid provider request json"))?;
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderFault::invalid_request("provider request missing op"))?;
    let operation = match op {
        "init" => ProviderOperation::Init,
        "status" => ProviderOperation::Status,
        "shutdown" => ProviderOperation::Shutdown,
        "offers_list" => ProviderOperation::OffersList,
        "runs_create" => ProviderOperation::RunsCreate,
        "runs_get" => ProviderOperation::RunsGet,
        "runs_events" => ProviderOperation::RunsEvents,
        "runs_cancel" => ProviderOperation::RunsCancel,
        other => ProviderOperation::Unsupported(other.to_string()),
    };
    Ok(ProviderEnvelope { operation, value })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitRequest {
    pub op: String,
    pub config: BridgeProviderConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffersListRequest {
    pub op: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsCreateRequest {
    pub op: String,
    pub offer_id: String,
    pub operation: String,
    pub input: Value,
    pub runtime_binding: RuntimeCreateBinding,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsGetRequest {
    pub op: String,
    pub run_id: String,
    pub runtime_binding: RuntimeAccessBinding,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsEventsRequest {
    pub op: String,
    pub run_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    pub runtime_binding: RuntimeAccessBinding,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsCancelRequest {
    pub op: String,
    pub run_id: String,
    pub runtime_binding: RuntimeAccessBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Prepared,
    Running,
    Reconciling,
    Completed,
    Failed,
    Cancelled,
    SettlementUnknown,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::SettlementUnknown
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    SelectionUnavailable,
    CredentialsUnavailable,
    AuthenticationRejected,
    RateLimited,
    ContextRejected,
    BackendTimeout,
    TransportInterrupted,
    BackendFailed,
    ResponseMalformed,
    Cancelled,
    SettlementUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunError {
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEvent {
    pub schema: String,
    pub sequence: u64,
    pub kind: String,
    pub data: Value,
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfferPolicySummary {
    pub schema: String,
    pub concurrency_limit: u32,
    pub input_bytes_limit: u64,
    pub inline_output_bytes_limit: u64,
    pub event_bytes_limit: u64,
    pub runtime_ms_limit: u64,
    pub retention_secs: u64,
    pub cancel_settlement_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfferSummary {
    pub id: String,
    pub title: String,
    pub operation: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub stream_output: bool,
    pub policy: OfferPolicySummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunTerminalOutcome {
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    pub schema: String,
    pub run_id: String,
    pub offer_id: String,
    pub operation: String,
    pub status: RunStatus,
    pub sequence_cursor: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminalOutcome>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventsPage {
    pub schema: String,
    pub run_id: String,
    pub next_cursor: u64,
    pub has_more: bool,
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResponse {
    pub provider: String,
    pub protocol_version: String,
    pub offers_ready: usize,
}

#[derive(Debug, Clone)]
pub struct ProviderFault {
    code: &'static str,
    message: &'static str,
    detail: Option<String>,
}

impl ProviderFault {
    pub fn invalid_request(message: &'static str) -> Self {
        Self {
            code: "invalid_request",
            message,
            detail: None,
        }
    }

    pub fn invalid_frame(limit: usize) -> Self {
        Self {
            code: "invalid_frame",
            message: "model request exceeds provider frame limit",
            detail: Some(format!("request frame exceeds {limit} bytes")),
        }
    }

    pub fn unsupported_operation(op: &str) -> Self {
        Self {
            code: "unsupported_operation",
            message: "unsupported model provider operation",
            detail: Some(format!("unsupported operation: {op}")),
        }
    }

    pub fn not_initialized() -> Self {
        Self {
            code: "not_initialized",
            message: "model provider is not initialized",
            detail: None,
        }
    }

    pub fn unauthorized_run_access() -> Self {
        Self {
            code: "run_not_found",
            message: "model run is not available for the current caller",
            detail: None,
        }
    }

    pub fn selection_unavailable(detail: impl Into<String>) -> Self {
        Self {
            code: "selection_unavailable",
            message: "model offer is not available",
            detail: Some(detail.into()),
        }
    }

    pub fn policy_limit(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code: "policy_limit",
            message,
            detail: Some(detail.into()),
        }
    }

    pub fn corrupt_journal(detail: impl Into<String>) -> Self {
        Self {
            code: "journal_corrupt",
            message: "model run journal is unavailable",
            detail: Some(detail.into()),
        }
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            code: "internal_error",
            message: "model provider failed",
            detail: Some(detail.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub fn log(&self) {
        if let Some(detail) = &self.detail {
            eprintln!("[model-provider] {}: {}", self.code, detail);
        }
    }
}

pub fn ok_response(data: Value) -> Value {
    serde_json::json!({
        "status": "ok",
        "data": data,
    })
}

pub fn error_response(code: &str, message: &str) -> Value {
    serde_json::json!({
        "status": "error",
        "code": code,
        "message": message,
    })
}

pub fn validate_trimmed(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.trim() != value {
        anyhow::bail!("{label} must be a trimmed non-empty string");
    }
    Ok(())
}

pub fn validate_bounded_trimmed(value: &str, label: &str, max_bytes: usize) -> Result<()> {
    validate_trimmed(value, label)?;
    if value.len() > max_bytes {
        anyhow::bail!("{label} exceeds {max_bytes} bytes");
    }
    Ok(())
}

pub fn hex_hash(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
