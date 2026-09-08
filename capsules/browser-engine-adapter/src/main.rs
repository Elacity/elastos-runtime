//! ElastOS browser-engine-adapter Capsule
//!
//! Internal contract between the Browser UI capsule and a real browser engine
//! host adapter. The provider never gives browser capsules raw host network,
//! wallet, or browser-engine authority; configured host adapters attach only
//! through Runtime-owned stream and display sessions.

use elastos_common::browser_protocol::{
    browser_display_generation_valid, browser_display_request_id_valid,
    validate_browser_inspection_result, BrowserDisplayAttachment, BrowserDisplayError,
    BrowserDisplayMode, BrowserEngineAdapterCapabilities, BrowserEngineReadiness,
    BrowserEngineReadinessReason, BrowserGuaranteeLevel, BrowserInspectionError,
    BrowserInspectionRequest, BrowserProfileDescriptor, BrowserViewport as ViewportRequest,
    BROWSER_DISPLAY_ATTACH_REQUEST_SCHEMA, BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
    BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA, BROWSER_ENGINE_PROTOCOL_VERSION,
    BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_READINESS_SCHEMA,
    BROWSER_INSPECTION_MAX_RESPONSE_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};

mod display;
mod ids;
mod supervisor;
mod transport;
mod validation;

use display::*;
use ids::*;
use supervisor::*;
use transport::*;
use validation::*;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA: &str =
    "elastos.browser.engine.launch-reconciliation/v1";
const BROWSER_ENGINE_RECONCILIATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(1);
const MAX_BROWSER_ENGINE_RECONCILIATION_RESPONSE_BYTES: usize = 64 * 1024;
const BROWSER_SUPERVISOR_CLEANUP_RESULT_SCHEMA: &str =
    "elastos.browser.supervisor-cleanup-result/v2";
const MAX_SUPERVISOR_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Readiness {
        adapter_id: String,
        #[serde(default)]
        principal_id: Option<String>,
    },
    Status {
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        lifecycle_generation: Option<String>,
        #[serde(default)]
        stream_id: Option<String>,
        #[serde(default)]
        adapter_id: Option<String>,
        #[serde(default)]
        transport_authority: Option<Value>,
    },
    Launch {
        url: String,
        stream_session: Box<StreamSessionReceipt>,
        lifecycle_generation: String,
        profile: BrowserProfileDescriptor,
        #[serde(default)]
        adapter_id: Option<String>,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        wallet: Value,
        #[serde(default)]
        viewport: Option<ViewportRequest>,
        display_mode: BrowserDisplayMode,
        guarantee_level: BrowserGuaranteeLevel,
        #[serde(default)]
        page_id: Option<String>,
        #[serde(default)]
        vm_id: Option<String>,
        #[serde(default)]
        transport_authority: Option<Value>,
        #[serde(default)]
        transport_secret: Option<Value>,
    },
    AttachStream {
        page_id: String,
        stream_session: StreamSessionReceipt,
        #[serde(default)]
        principal_id: Option<String>,
    },
    ClosePage {
        page_id: String,
        runtime_cleanup: EngineCleanupBinding,
        #[serde(default)]
        principal_id: Option<String>,
    },
    PageStatus {
        page_id: String,
        #[serde(default)]
        principal_id: Option<String>,
    },
    Diagnostics {
        page_id: String,
        #[serde(default)]
        principal_id: Option<String>,
    },
    Inspect {
        page_id: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        request: Option<BrowserInspectionRequest>,
    },
    Input {
        page_id: String,
        event: Value,
        #[serde(default)]
        principal_id: Option<String>,
    },
    WebrtcSignal {
        page_id: String,
        signal: Value,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        principal_id: Option<String>,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        adapter: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        launch_settlement_result: Option<Value>,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
            adapter: None,
            launch_settlement_result: None,
        }
    }

    fn supervisor_error(adapter: &AdapterConfig, error: SupervisorLaunchError) -> Self {
        Self::Error {
            code: error.code,
            message: error.message,
            adapter: Some(adapter.id.clone()),
            launch_settlement_result: error.launch_settlement_result,
        }
    }
}

struct BrowserEngineAdapter {
    adapters: Vec<AdapterConfig>,
    page_control_sessions: BTreeMap<String, PageControlSession>,
    max_active_sessions: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct EngineCleanupBinding {
    schema: String,
    page_id: String,
    generation: String,
    stream_id: String,
    adapter: String,
    engine: AdapterKind,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
    #[serde(default)]
    principal_id: Option<String>,
    control_socket_path: String,
    #[serde(default)]
    shutdown_socket_path: Option<String>,
    isolated_session: bool,
    #[serde(default)]
    isolation: Option<SupervisorIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_service: Option<ControlServiceIdentity>,
    #[serde(default)]
    process: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport_authority: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport_receipt: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ControlServiceIdentity {
    schema: String,
    service_id: String,
    control_socket_path: String,
    #[serde(default)]
    config_fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableControlLaunchIdentity {
    adapter: String,
    engine: AdapterKind,
    lifecycle_generation: String,
    stream_id: String,
    principal_id: Option<String>,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
    #[serde(default)]
    page_id: Option<String>,
    #[serde(default)]
    vm_id: Option<String>,
    #[serde(default)]
    transport_authority: Option<Value>,
}

#[derive(Debug, Clone)]
struct PageControlSession {
    display_attachment: BrowserDisplayAttachment,
    generation: String,
    stream_id: String,
    socket_path: String,
    shutdown_socket_path: Option<String>,
    adapter_id: String,
    principal_id: Option<String>,
    engine: AdapterKind,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
    isolated_session: bool,
    isolation_session_dir: Option<String>,
    isolation_kind: Option<String>,
    control_service: Option<ControlServiceIdentity>,
    process: Option<Value>,
    transport_authority: Option<Value>,
    transport_receipt: Option<Value>,
}

fn engine_cleanup_binding(page_id: &str, session: &PageControlSession) -> EngineCleanupBinding {
    EngineCleanupBinding {
        schema: BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA.to_string(),
        page_id: page_id.to_string(),
        generation: session.generation.clone(),
        stream_id: session.stream_id.clone(),
        adapter: session.adapter_id.clone(),
        engine: session.engine,
        display_mode: session.display_mode,
        guarantee_level: session.guarantee_level,
        principal_id: session.principal_id.clone(),
        control_socket_path: session.socket_path.clone(),
        shutdown_socket_path: session.shutdown_socket_path.clone(),
        isolated_session: session.isolated_session,
        isolation: session
            .isolation_session_dir
            .as_ref()
            .zip(session.isolation_kind.as_ref())
            .map(|(session_dir, kind)| SupervisorIsolation {
                schema: "elastos.browser.engine.isolation/v1".to_string(),
                kind: kind.clone(),
                session_dir: session_dir.clone(),
            }),
        control_service: session.control_service.clone(),
        process: session.process.clone(),
        transport_authority: session.transport_authority.clone(),
        transport_receipt: session.transport_receipt.clone(),
    }
}

fn page_control_session_from_cleanup(
    binding: &EngineCleanupBinding,
) -> Result<PageControlSession, String> {
    validate_engine_cleanup_binding(binding)?;
    Ok(PageControlSession {
        display_attachment: BrowserDisplayAttachment::default(),
        generation: binding.generation.clone(),
        stream_id: binding.stream_id.clone(),
        socket_path: binding.control_socket_path.clone(),
        shutdown_socket_path: binding.shutdown_socket_path.clone(),
        adapter_id: binding.adapter.clone(),
        principal_id: binding.principal_id.clone(),
        engine: binding.engine,
        display_mode: binding.display_mode,
        guarantee_level: binding.guarantee_level,
        isolated_session: binding.isolated_session,
        isolation_session_dir: binding
            .isolation
            .as_ref()
            .map(|isolation| isolation.session_dir.clone()),
        isolation_kind: binding
            .isolation
            .as_ref()
            .map(|isolation| isolation.kind.clone()),
        control_service: binding.control_service.clone(),
        process: binding.process.clone(),
        transport_authority: binding.transport_authority.clone(),
        transport_receipt: binding.transport_receipt.clone(),
    })
}

fn validate_engine_cleanup_binding(binding: &EngineCleanupBinding) -> Result<(), String> {
    if binding.schema != BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA
        || !is_safe_id(&binding.page_id)
        || binding.page_id.len() > 256
        || !is_safe_id(&binding.generation)
        || binding.generation.len() > 256
        || !is_safe_id(&binding.stream_id)
        || binding.stream_id.len() > 256
        || !is_safe_id(&binding.adapter)
        || binding.adapter.len() > 256
    {
        return Err("invalid Runtime Browser cleanup binding".to_string());
    }
    validate_control_socket_path(&binding.control_socket_path)?;
    if let Some(path) = &binding.shutdown_socket_path {
        validate_control_socket_path(path)?;
    }
    if binding.isolated_session != binding.isolation.is_some() {
        return Err("Browser cleanup isolation binding is inconsistent".to_string());
    }
    if let Some(authority) = &binding.transport_authority {
        validate_vz_transport_authority(authority)?;
        validate_vz_transport_effect_receipt(
            binding.transport_receipt.as_ref().ok_or_else(|| {
                "Browser cleanup omitted its VZ transport effect receipt".to_string()
            })?,
            authority,
        )?;
        if authority.get("generation").and_then(Value::as_str) != Some(binding.generation.as_str())
            || authority.get("page_id").and_then(Value::as_str) != Some(binding.page_id.as_str())
            || authority
                .pointer("/egress/stream_id")
                .and_then(Value::as_str)
                != Some(binding.stream_id.as_str())
        {
            return Err("Browser cleanup transport authority changed".to_string());
        }
    } else if binding.transport_receipt.is_some() {
        return Err("Browser cleanup has an unexpected VZ transport receipt".to_string());
    }
    if let Some(isolation) = &binding.isolation {
        if isolation.schema != "elastos.browser.engine.isolation/v1"
            || !matches!(
                isolation.kind.as_str(),
                "per_launch_selkies_target" | "per_launch_vm_target"
            )
            || !isolation.session_dir.starts_with('/')
            || isolation.session_dir.contains(['\0', '\r', '\n'])
        {
            return Err("invalid Runtime Browser cleanup isolation binding".to_string());
        }
        if isolation.kind == "per_launch_vm_target" {
            let identity = binding.control_service.as_ref().ok_or_else(|| {
                "Browser VM cleanup control-service identity is unavailable".to_string()
            })?;
            validate_control_service_identity(identity, binding.shutdown_socket_path.as_deref())?;
            if !host_process_binding_is_safe(binding.process.as_ref()) {
                return Err("Browser VM cleanup host-process binding is invalid".to_string());
            }
        }
    }
    if binding
        .isolation
        .as_ref()
        .map(|isolation| isolation.kind.as_str())
        != Some("per_launch_vm_target")
    {
        if let Some(process) = &binding.process {
            let Value::Object(process) = process else {
                return Err("Browser cleanup process binding is invalid".to_string());
            };
            if !(2..=3).contains(&process.len())
                || !process.contains_key("pid")
                || !process.contains_key("stream_bridge_pid")
                || process.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "pid" | "stream_bridge_pid" | "network_sandbox"
                    )
                })
                || !process_id_is_safe(process.get("pid"))
                || !optional_process_id_is_safe(process.get("stream_bridge_pid"))
                || process
                    .get("network_sandbox")
                    .is_some_and(|value| value.as_str() != Some("linux_new_netns"))
            {
                return Err("Browser cleanup process binding is invalid".to_string());
            }
        }
    }
    if serde_json::to_vec(binding).map_or(true, |bytes| bytes.len() > 16 * 1024) {
        return Err("Runtime Browser cleanup binding is too large".to_string());
    }
    Ok(())
}

fn validate_control_service_identity(
    identity: &ControlServiceIdentity,
    expected_socket_path: Option<&str>,
) -> Result<(), String> {
    if identity.schema != "elastos.browser.vm-control-service.identity/v1"
        || !identity
            .service_id
            .strip_prefix("service:")
            .is_some_and(is_lower_hex_64)
        || expected_socket_path != Some(identity.control_socket_path.as_str())
        || validate_control_socket_path(&identity.control_socket_path).is_err()
        || identity
            .config_fingerprint
            .as_deref()
            .is_some_and(|value| !is_lower_hex_64(value))
    {
        return Err("Browser control-service identity is invalid".to_string());
    }
    Ok(())
}

fn host_process_binding_is_safe(process: Option<&Value>) -> bool {
    let Some(Value::Object(process)) = process else {
        return false;
    };
    process.len() == 4
        && process.get("schema").and_then(Value::as_str)
            == Some("elastos.browser.host-process-binding/v1")
        && process
            .get("ownership_id")
            .and_then(Value::as_str)
            .and_then(|value| value.strip_prefix("process:"))
            .is_some_and(is_lower_hex_64)
        && process_id_is_safe(process.get("pid"))
        && process.get("stream_bridge_pid").is_some_and(Value::is_null)
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn live_vm_control_service_identity(
    adapter: &AdapterConfig,
) -> Result<(String, ControlServiceIdentity), String> {
    let control_socket_path = adapter
        .supervisor
        .as_ref()
        .and_then(|supervisor| supervisor.control_socket_path.as_deref())
        .ok_or_else(|| "selected Browser control service is unavailable".to_string())?;
    let status = supervisor_control_json_bounded(
        control_socket_path,
        "GET",
        "/status",
        None,
        BROWSER_ENGINE_RECONCILIATION_TIMEOUT,
        MAX_BROWSER_ENGINE_RECONCILIATION_RESPONSE_BYTES,
    )?;
    if status.get("schema").and_then(Value::as_str)
        != Some("elastos.browser.vm-control-service.status/v1")
    {
        return Err("Browser control-service status schema is invalid".to_string());
    }
    let identity = serde_json::from_value::<ControlServiceIdentity>(
        status
            .get("control_service")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|_| "Browser live control-service identity is unavailable".to_string())?;
    validate_control_service_identity(&identity, Some(control_socket_path))?;
    Ok((control_socket_path.to_string(), identity))
}

fn process_id_is_safe(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_u64)
        .is_some_and(|pid| pid > 0 && pid <= i32::MAX as u64)
}

fn optional_process_id_is_safe(value: Option<&Value>) -> bool {
    value.is_some_and(|value| value.is_null() || process_id_is_safe(Some(value)))
}

struct DurableReconciliationBinding<'a> {
    control_socket_path: &'a str,
    live_control_service: &'a ControlServiceIdentity,
    principal_id: Option<&'a str>,
    lifecycle_generation: &'a str,
    stream_id: &'a str,
    transport_authority: Option<&'a Value>,
}

fn page_control_session_from_durable_reconciliation(
    reconciliation: &Value,
    adapter: &AdapterConfig,
    expected: DurableReconciliationBinding<'_>,
) -> Result<(String, PageControlSession), String> {
    let DurableReconciliationBinding {
        control_socket_path,
        live_control_service,
        principal_id,
        lifecycle_generation,
        stream_id,
        transport_authority,
    } = expected;
    let launch = serde_json::from_value::<DurableControlLaunchIdentity>(
        reconciliation.get("launch").cloned().unwrap_or(Value::Null),
    )
    .map_err(|_| "Browser control-service launch binding is invalid".to_string())?;
    let binding = serde_json::from_value::<EngineCleanupBinding>(
        reconciliation
            .get("cleanup_binding")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|_| "Browser control-service cleanup binding is unavailable".to_string())?;
    validate_engine_cleanup_binding(&binding)?;
    let recorded_control_service = serde_json::from_value::<ControlServiceIdentity>(
        reconciliation
            .get("control_service")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|_| "Browser durable control-service identity is unavailable".to_string())?;
    let responder_control_service = serde_json::from_value::<ControlServiceIdentity>(
        reconciliation
            .get("responder_control_service")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|_| "Browser responding control-service identity is unavailable".to_string())?;
    if launch.lifecycle_generation != lifecycle_generation
        || launch.stream_id != stream_id
        || launch.principal_id.as_deref() != principal_id
        || launch.adapter != adapter.id
        || launch.transport_authority.as_ref() != transport_authority
        || match transport_authority {
            Some(authority) => {
                launch.page_id.as_deref() != Some(binding.page_id.as_str())
                    || launch.vm_id.as_deref() != authority.get("vm_id").and_then(Value::as_str)
            }
            None => launch.page_id.is_some() || launch.vm_id.is_some(),
        }
        || binding.generation != lifecycle_generation
        || binding.stream_id != stream_id
        || binding.principal_id.as_deref() != principal_id
        || binding.adapter != launch.adapter
        || binding.engine != launch.engine
        || binding.display_mode != launch.display_mode
        || binding.guarantee_level != launch.guarantee_level
        || binding.transport_authority.as_ref() != transport_authority
        || binding.control_service.as_ref() != Some(live_control_service)
        || recorded_control_service != *live_control_service
        || responder_control_service != *live_control_service
    {
        return Err("Browser durable launch or cleanup identity changed".to_string());
    }
    if launch
        .principal_id
        .as_deref()
        .is_some_and(|principal| !is_safe_id(principal) || principal.len() > 512)
    {
        return Err("Browser durable launch principal is invalid".to_string());
    }
    if adapter.kind != launch.engine
        || !adapter.display_modes.contains(&launch.display_mode)
        || !adapter_supports_guarantee(adapter.kind, launch.guarantee_level)
    {
        return Err("Browser durable launch adapter contract changed".to_string());
    }
    if binding.shutdown_socket_path.as_deref() != Some(control_socket_path)
        || !binding.isolated_session
        || binding
            .isolation
            .as_ref()
            .map(|isolation| isolation.kind.as_str())
            != Some("per_launch_vm_target")
    {
        return Err("Browser durable launch control or isolation binding changed".to_string());
    }
    let page_id = binding.page_id.clone();
    let session = page_control_session_from_cleanup(&binding)?;
    Ok((page_id, session))
}

fn engine_terminal_cleanup_result(
    binding: &EngineCleanupBinding,
    supervisor_receipt: Value,
) -> Result<Value, String> {
    validate_engine_cleanup_binding(binding)?;
    let transport_effects_terminal = binding.transport_authority.is_none()
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
            supervisor_receipt
                .pointer(&format!("/effects/{effect}"))
                .and_then(Value::as_bool)
                == Some(true)
        });
    if supervisor_receipt.get("schema").and_then(Value::as_str)
        != Some(BROWSER_SUPERVISOR_CLEANUP_RESULT_SCHEMA)
        || supervisor_receipt.get("page_id").and_then(Value::as_str)
            != Some(binding.page_id.as_str())
        || supervisor_receipt.get("generation").and_then(Value::as_str)
            != Some(binding.generation.as_str())
        || supervisor_receipt.get("binding")
            != Some(&serde_json::to_value(binding).unwrap_or(Value::Null))
        || supervisor_receipt.get("terminal").and_then(Value::as_bool) != Some(true)
        || supervisor_receipt
            .pointer("/effects/page_absent")
            .and_then(Value::as_bool)
            != Some(true)
        || supervisor_receipt
            .pointer("/effects/child_absent")
            .and_then(Value::as_bool)
            != Some(true)
        || supervisor_receipt
            .pointer("/effects/vm_absent")
            .and_then(Value::as_bool)
            != Some(true)
        || supervisor_receipt
            .pointer("/effects/route_absent")
            .and_then(Value::as_bool)
            != Some(true)
        || supervisor_receipt
            .pointer("/effects/socket_absent")
            .and_then(Value::as_bool)
            != Some(true)
        || !transport_effects_terminal
    {
        return Err(
            "Browser supervisor did not return an exact typed terminal cleanup receipt".to_string(),
        );
    }
    Ok(json!({
        "schema": BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
        "page_id": binding.page_id,
        "generation": binding.generation,
        "binding": binding,
        "terminal": true,
        "effects": supervisor_receipt["effects"],
    }))
}

struct LaunchContext<'a> {
    url: &'a str,
    stream_session: &'a StreamSessionReceipt,
    profile: BrowserProfileDescriptor,
    adapter_id: Option<String>,
    principal_id: Option<String>,
    reason: Option<String>,
    wallet: Value,
    viewport: Option<ViewportRequest>,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
}

struct VzTransportLaunchContext {
    page_id: String,
    vm_id: String,
    authority: Value,
    secret: Value,
}

fn vz_transport_launch_context(
    page_id: Option<String>,
    vm_id: Option<String>,
    authority: Option<Value>,
    secret: Option<Value>,
) -> Result<Option<VzTransportLaunchContext>, String> {
    match (page_id, vm_id, authority, secret) {
        (None, None, None, None) => Ok(None),
        (Some(page_id), Some(vm_id), Some(authority), Some(secret)) => {
            Ok(Some(VzTransportLaunchContext {
                page_id,
                vm_id,
                authority,
                secret,
            }))
        }
        _ => Err(
            "Browser VZ launch requires page_id, vm_id, transport_authority, and transport_secret together"
                .to_string(),
        ),
    }
}

impl BrowserEngineAdapter {
    fn new() -> Self {
        Self {
            adapters: Vec::new(),
            page_control_sessions: BTreeMap::new(),
            max_active_sessions: default_max_active_sessions(),
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Readiness {
                adapter_id,
                principal_id,
            } => self.readiness(&adapter_id, principal_id),
            Request::Status {
                principal_id,
                lifecycle_generation,
                stream_id,
                adapter_id,
                transport_authority,
            } => self.status_request(
                principal_id,
                lifecycle_generation,
                stream_id,
                adapter_id,
                transport_authority,
            ),
            Request::Launch {
                url,
                stream_session,
                lifecycle_generation,
                profile,
                adapter_id,
                principal_id,
                reason,
                wallet,
                viewport,
                display_mode,
                guarantee_level,
                page_id,
                vm_id,
                transport_authority,
                transport_secret,
            } => {
                let transport = match vz_transport_launch_context(
                    page_id,
                    vm_id,
                    transport_authority,
                    transport_secret,
                ) {
                    Ok(transport) => transport,
                    Err(message) => return Response::error("invalid_request", message),
                };
                self.launch_with_generation(
                    LaunchContext {
                        url: &url,
                        stream_session: &stream_session,
                        profile,
                        adapter_id,
                        principal_id,
                        reason,
                        wallet,
                        viewport,
                        display_mode,
                        guarantee_level,
                    },
                    &lifecycle_generation,
                    transport,
                )
            }
            Request::AttachStream {
                page_id,
                stream_session,
                principal_id,
            } => self.attach_stream(&page_id, &stream_session, principal_id),
            Request::ClosePage {
                page_id,
                runtime_cleanup,
                principal_id,
            } => self.close_page(&page_id, principal_id, runtime_cleanup),
            Request::PageStatus {
                page_id,
                principal_id,
            } => self.page_status(&page_id, principal_id),
            Request::Diagnostics {
                page_id,
                principal_id,
            } => self.diagnostics(&page_id, principal_id),
            Request::Inspect {
                page_id,
                principal_id,
                request,
            } => self.inspect(&page_id, principal_id, request),
            Request::Input {
                page_id,
                event,
                principal_id,
            } => self.input(&page_id, event, principal_id),
            Request::WebrtcSignal {
                page_id,
                signal,
                channel,
                principal_id,
            } => self.webrtc_signal(&page_id, signal, channel, principal_id),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let config = match parse_config(config) {
            Ok(config) => config,
            Err(err) => return Response::error("invalid_config", err),
        };
        self.adapters = config.adapters;
        self.max_active_sessions = config.max_active_sessions;
        self.page_control_sessions.clear();
        let mut prewarm_results = Vec::new();
        for adapter in &self.adapters {
            let Some(supervisor) = adapter.supervisor.as_ref() else {
                continue;
            };
            if supervisor
                .env
                .get("ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE")
                .map(String::as_str)
                != Some("1")
            {
                continue;
            }
            match run_supervisor_prewarm(supervisor) {
                Ok(result) => prewarm_results.push(json!({
                    "adapter": adapter.id,
                    "status": "ok",
                    "result": result,
                })),
                Err(err) => {
                    prewarm_results.push(json!({
                        "adapter": adapter.id,
                        "status": "error",
                        "code": "engine_process_unavailable",
                        "message": err,
                    }));
                }
            }
        }
        Response::ok(json!({
            "provider": BROWSER_ENGINE_PROVIDER_ID,
            "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
            "adapter_count": self.adapters.len(),
            "active_sessions": self.page_control_sessions.len(),
            "max_active_sessions": self.max_active_sessions,
            "prewarm_results": prewarm_results,
            "direct_network": false,
            "wallet_injection": false,
        }))
    }

    fn status_request(
        &mut self,
        principal_id: Option<String>,
        lifecycle_generation: Option<String>,
        stream_id: Option<String>,
        adapter_id: Option<String>,
        transport_authority: Option<Value>,
    ) -> Response {
        if lifecycle_generation.is_some() || stream_id.is_some() {
            let (Some(lifecycle_generation), Some(stream_id)) = (lifecycle_generation, stream_id)
            else {
                return Response::error(
                    "invalid_request",
                    "Browser launch reconciliation requires lifecycle_generation and stream_id",
                );
            };
            return self.reconcile_launch(
                principal_id,
                &lifecycle_generation,
                &stream_id,
                adapter_id.as_deref(),
                transport_authority.as_ref(),
            );
        }
        self.status(principal_id)
    }

    fn status(&mut self, principal_id: Option<String>) -> Response {
        let stale_sessions_reaped = self.reconcile_page_control_sessions();
        Response::ok(json!({
            "provider": BROWSER_ENGINE_PROVIDER_ID,
            "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
            "status": if self.adapters.is_empty() { "unavailable" } else { "configured" },
            "principal_id": principal_id,
            "adapter_count": self.adapters.len(),
            "active_sessions": self.page_control_sessions.len(),
            "stale_sessions_reaped": stale_sessions_reaped,
            "max_active_sessions": self.max_active_sessions,
            "capacity_available": self.page_control_sessions.len() < self.max_active_sessions,
            "direct_network": false,
            "wallet_injection": false,
            "stream_session_schema": "elastos.exit.stream-session/v1",
            "required_byte_transport": "adapter_ipc",
            "display_session_schema": "elastos.browser.display-session/v1",
            "adapters": self.adapter_summaries(),
            "supported_display_modes": self.supported_display_modes(),
            "supported_guarantee_levels": self.supported_guarantee_levels(),
            "operations": ["status", "readiness", "launch", "attach_stream", "close_page", "page_status", "diagnostics", "inspect", "input", "webrtc_signal"],
        }))
    }

    fn readiness(&self, adapter_id: &str, _principal_id: Option<String>) -> Response {
        if adapter_id.is_empty() || !is_safe_id(adapter_id) {
            return Response::error(
                "invalid_request",
                "Browser readiness requires a valid Engine identity",
            );
        }
        let Some(adapter) = self.select_adapter(Some(adapter_id)) else {
            return Response::error(
                "engine_not_found",
                "The selected Browser Engine is unavailable",
            );
        };
        let readiness = adapter
            .supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.control_socket_path.as_deref())
            .map(|socket| {
                supervisor_control_json_bounded(
                    socket,
                    "GET",
                    "/readiness",
                    None,
                    std::time::Duration::from_secs(10),
                    8192,
                )
                .map_err(|_| BrowserEngineReadinessReason::ControlUnavailable)
                .and_then(|value| {
                    if value.get("schema").and_then(Value::as_str)
                        != Some(BROWSER_ENGINE_READINESS_SCHEMA)
                    {
                        return Err(BrowserEngineReadinessReason::ReadinessUnsupported);
                    }
                    serde_json::from_value::<BrowserEngineReadiness>(value["readiness"].clone())
                        .map_err(|_| BrowserEngineReadinessReason::ReadinessUnsupported)
                })
                .unwrap_or_else(|reason| BrowserEngineReadiness::Unavailable { reason })
            })
            .unwrap_or(BrowserEngineReadiness::Unavailable {
                reason: BrowserEngineReadinessReason::PreparationRequired,
            });
        Response::ok(json!({
            "schema": BROWSER_ENGINE_READINESS_SCHEMA,
            "adapter_id": adapter_id,
            "readiness": readiness,
        }))
    }

    fn reconcile_launch(
        &mut self,
        principal_id: Option<String>,
        lifecycle_generation: &str,
        stream_id: &str,
        adapter_id: Option<&str>,
        transport_authority: Option<&Value>,
    ) -> Response {
        if !is_safe_id(lifecycle_generation)
            || lifecycle_generation.len() > 256
            || !is_safe_id(stream_id)
            || stream_id.len() > 256
            || adapter_id
                .is_some_and(|value| value.is_empty() || !is_safe_id(value) || value.len() > 256)
        {
            return Response::error(
                "invalid_request",
                "Browser launch reconciliation identity is invalid",
            );
        }
        if let Some(authority) = transport_authority {
            if validate_vz_transport_authority(authority).is_err()
                || authority.get("generation").and_then(Value::as_str) != Some(lifecycle_generation)
                || authority
                    .pointer("/egress/stream_id")
                    .and_then(Value::as_str)
                    != Some(stream_id)
                || authority.get("principal_id").and_then(Value::as_str) != principal_id.as_deref()
            {
                return Response::error(
                    "invalid_request",
                    "Browser VZ transport reconciliation binding is invalid",
                );
            }
        }
        let reconcile_ok = |mut data: Value| {
            if let (Some(data), Some(authority)) = (data.as_object_mut(), transport_authority) {
                data.insert("transport_authority".to_string(), authority.clone());
            }
            Response::ok(data)
        };

        let Some(adapter_id) = adapter_id else {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "selected Browser Engine Adapter identity is unavailable",
            }));
        };
        let Some(selected_adapter) = self.select_adapter(Some(adapter_id)) else {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "selected Browser Engine Adapter is unavailable",
            }));
        };
        let live_vm_control_service = if selected_adapter.kind == AdapterKind::ChromiumMicrovm {
            match live_vm_control_service_identity(&selected_adapter) {
                Ok(identity) => Some(identity),
                Err(_) => {
                    return reconcile_ok(json!({
                        "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                        "state": "cleanup_pending",
                        "lifecycle_generation": lifecycle_generation,
                        "stream_id": stream_id,
                        "reason": "selected Browser control-service identity is unavailable",
                    }));
                }
            }
        } else {
            None
        };
        let exact_sessions = self
            .page_control_sessions
            .iter()
            .filter(|(_, session)| {
                session.generation == lifecycle_generation && session.stream_id == stream_id
            })
            .map(|(page_id, session)| (page_id.clone(), session.clone()))
            .collect::<Vec<_>>();
        if exact_sessions.len() > 1 {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "provider has multiple effects for one lifecycle identity",
            }));
        }
        if let Some((page_id, session)) = exact_sessions.into_iter().next() {
            if !page_control_session_principal_matches(&session, principal_id.as_deref()) {
                return Response::error(
                    "cleanup_binding_mismatch",
                    "Browser launch reconciliation principal changed",
                );
            }
            if session.adapter_id != selected_adapter.id
                || validate_engine_cleanup_binding(&engine_cleanup_binding(&page_id, &session))
                    .is_err()
                || live_vm_control_service.as_ref().is_some_and(
                    |(control_socket_path, live_control_service)| {
                        session.shutdown_socket_path.as_deref()
                            != Some(control_socket_path.as_str())
                            || session.control_service.as_ref() != Some(live_control_service)
                    },
                )
            {
                return reconcile_ok(json!({
                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                    "state": "cleanup_pending",
                    "lifecycle_generation": lifecycle_generation,
                    "stream_id": stream_id,
                    "reason": "selected Browser Engine Adapter or cleanup identity changed",
                }));
            }
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "effect_acquired",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "effect": {
                    "provider": BROWSER_ENGINE_PROVIDER_ID,
                    "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
                    "page_id": page_id,
                    "adapter": session.adapter_id,
                    "engine": session.engine,
                    "stream_id": session.stream_id,
                    "transport_authority": session.transport_authority,
                    "transport_receipt": session.transport_receipt,
                    "runtime_cleanup": engine_cleanup_binding(&page_id, &session),
                },
            }));
        }
        if self.page_control_sessions.values().any(|session| {
            session.generation == lifecycle_generation || session.stream_id == stream_id
        }) {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "provider lifecycle identity conflict",
            }));
        }

        let Some((control_socket_path, live_control_service)) = live_vm_control_service else {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "selected Browser control service is unavailable",
            }));
        };
        let mut control_reconciliation_request = json!({
            "schema": "elastos.browser.vm-control-service.reconcile-launch/v1",
            "lifecycle_generation": lifecycle_generation,
            "stream_id": stream_id,
        });
        if let Some(authority) = transport_authority {
            control_reconciliation_request["transport_authority"] = authority.clone();
        }
        let reconciliation = supervisor_control_json_bounded(
            &control_socket_path,
            "POST",
            "/launches/reconcile",
            Some(control_reconciliation_request),
            BROWSER_ENGINE_RECONCILIATION_TIMEOUT,
            MAX_BROWSER_ENGINE_RECONCILIATION_RESPONSE_BYTES,
        );
        let Ok(reconciliation) = reconciliation else {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "selected Browser control service did not reconcile",
            }));
        };
        let response_identity = serde_json::from_value::<ControlServiceIdentity>(
            reconciliation
                .get("responder_control_service")
                .cloned()
                .unwrap_or(Value::Null),
        );
        let record_identity = serde_json::from_value::<ControlServiceIdentity>(
            reconciliation
                .get("control_service")
                .cloned()
                .unwrap_or(Value::Null),
        );
        let stable_terminal_service_identity = record_identity.as_ref().is_ok_and(|record| {
            reconciliation.get("state").and_then(Value::as_str)
                == Some("terminal_post_effect_cleanup")
                && reconciliation.get("launch_settlement_result").is_some()
                && record.schema == live_control_service.schema
                && record.service_id == live_control_service.service_id
                && record.control_socket_path == live_control_service.control_socket_path
                && validate_control_service_identity(
                    record,
                    Some(live_control_service.control_socket_path.as_str()),
                )
                .is_ok()
        });
        if reconciliation.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vm-control-service.launch-reconciliation/v1")
            || reconciliation
                .pointer("/launch/lifecycle_generation")
                .and_then(Value::as_str)
                != Some(lifecycle_generation)
            || reconciliation
                .pointer("/launch/stream_id")
                .and_then(Value::as_str)
                != Some(stream_id)
            || reconciliation.get("transport_authority") != transport_authority
            || reconciliation.pointer("/launch/transport_authority") != transport_authority
            || response_identity.as_ref().ok() != Some(&live_control_service)
            || (record_identity.as_ref().ok() != Some(&live_control_service)
                && !stable_terminal_service_identity)
        {
            return reconcile_ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "Browser durable or responding control-service identity changed",
            }));
        }
        match reconciliation.get("state").and_then(Value::as_str) {
            Some("did_not_act")
                if reconciliation
                    .pointer("/effects/page_acquired")
                    .and_then(Value::as_bool)
                    == Some(false)
                    && reconciliation
                        .pointer("/effects/vm_acquired")
                        .and_then(Value::as_bool)
                        == Some(false) =>
            {
                return reconcile_ok(json!({
                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                    "state": "did_not_act",
                    "lifecycle_generation": lifecycle_generation,
                    "stream_id": stream_id,
                    "effects": {
                        "page_acquired": false,
                        "vm_acquired": false,
                    },
                }));
            }
            Some("terminal_post_effect_cleanup") => {
                let Some(page_acquired) = reconciliation
                    .pointer("/effects/page_acquired")
                    .and_then(Value::as_bool)
                else {
                    return reconcile_ok(json!({
                        "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                        "state": "cleanup_pending",
                        "lifecycle_generation": lifecycle_generation,
                        "stream_id": stream_id,
                        "reason": "terminal Browser cleanup effects are incomplete",
                    }));
                };
                let Some(vm_acquired) = reconciliation
                    .pointer("/effects/vm_acquired")
                    .and_then(Value::as_bool)
                else {
                    return reconcile_ok(json!({
                        "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                        "state": "cleanup_pending",
                        "lifecycle_generation": lifecycle_generation,
                        "stream_id": stream_id,
                        "reason": "terminal Browser cleanup effects are incomplete",
                    }));
                };
                let terminal_cleanup_receipt = match transport_authority {
                    Some(authority) => {
                        if let Some(settlement) = reconciliation.get("launch_settlement_result") {
                            let launch = reconciliation.get("launch").and_then(Value::as_object);
                            if selected_adapter.kind != AdapterKind::ChromiumMicrovm
                                || page_acquired
                                || vm_acquired
                                    != settlement
                                        .pointer("/effects/vm")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                || launch
                                    .and_then(|value| value.get("adapter"))
                                    .and_then(Value::as_str)
                                    != Some(selected_adapter.id.as_str())
                                || launch.and_then(|value| value.get("engine"))
                                    != Some(&json!(selected_adapter.kind))
                                || launch
                                    .and_then(|value| value.get("principal_id"))
                                    .and_then(Value::as_str)
                                    != principal_id.as_deref()
                                || launch
                                    .and_then(|value| value.get("display_mode"))
                                    .and_then(Value::as_str)
                                    != Some(BrowserDisplayMode::WebrtcRemoteDisplay.as_str())
                                || launch
                                    .and_then(|value| value.get("guarantee_level"))
                                    .and_then(Value::as_str)
                                    != Some(BrowserGuaranteeLevel::MechanismMicrovm.as_str())
                                || launch.and_then(|value| value.get("page_id"))
                                    != authority.get("page_id")
                                || launch.and_then(|value| value.get("vm_id"))
                                    != authority.get("vm_id")
                                || settlement.get("state").and_then(Value::as_str)
                                    != Some("terminal_post_effect_cleanup")
                                || validate_vz_launch_settlement_binding(settlement, authority)
                                    .is_err()
                            {
                                return reconcile_ok(json!({
                                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                    "state": "cleanup_pending",
                                    "lifecycle_generation": lifecycle_generation,
                                    "stream_id": stream_id,
                                    "reason": "terminal Browser VZ launch settlement is invalid",
                                }));
                            }
                            return reconcile_ok(json!({
                                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                "state": "terminal_post_effect_cleanup",
                                "lifecycle_generation": lifecycle_generation,
                                "stream_id": stream_id,
                                "effects": {
                                    "page_acquired": page_acquired,
                                    "vm_acquired": vm_acquired,
                                },
                                "terminal_cleanup_receipt": settlement,
                            }));
                        }
                        let binding =
                            reconciliation
                                .get("cleanup_binding")
                                .cloned()
                                .and_then(|value| {
                                    serde_json::from_value::<EngineCleanupBinding>(value).ok()
                                });
                        let Some(binding) = binding else {
                            return reconcile_ok(json!({
                                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                "state": "cleanup_pending",
                                "lifecycle_generation": lifecycle_generation,
                                "stream_id": stream_id,
                                "reason": "terminal Browser cleanup binding is unavailable",
                            }));
                        };
                        if binding.page_id
                            != authority
                                .get("page_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                            || binding.generation != lifecycle_generation
                            || binding.stream_id != stream_id
                            || binding.adapter != selected_adapter.id
                            || binding.engine != selected_adapter.kind
                            || binding.principal_id.as_deref() != principal_id.as_deref()
                            || binding.transport_authority.as_ref() != Some(authority)
                            || binding.control_service.as_ref() != Some(&live_control_service)
                            || binding.shutdown_socket_path.as_deref()
                                != Some(control_socket_path.as_str())
                        {
                            return reconcile_ok(json!({
                                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                "state": "cleanup_pending",
                                "lifecycle_generation": lifecycle_generation,
                                "stream_id": stream_id,
                                "reason": "terminal Browser cleanup identity changed",
                            }));
                        }
                        match engine_terminal_cleanup_result(
                            &binding,
                            reconciliation
                                .get("terminal_cleanup_receipt")
                                .cloned()
                                .unwrap_or(Value::Null),
                        ) {
                            Ok(receipt) => Some(receipt),
                            Err(_) => {
                                return reconcile_ok(json!({
                                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                    "state": "cleanup_pending",
                                    "lifecycle_generation": lifecycle_generation,
                                    "stream_id": stream_id,
                                    "reason": "terminal Browser cleanup receipt is invalid",
                                }));
                            }
                        }
                    }
                    None => {
                        if reconciliation.get("terminal_cleanup_receipt").is_some() {
                            return reconcile_ok(json!({
                                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                                "state": "cleanup_pending",
                                "lifecycle_generation": lifecycle_generation,
                                "stream_id": stream_id,
                                "reason": "unexpected terminal Browser transport receipt",
                            }));
                        }
                        None
                    }
                };
                return reconcile_ok(json!({
                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                    "state": "terminal_post_effect_cleanup",
                    "lifecycle_generation": lifecycle_generation,
                    "stream_id": stream_id,
                    "effects": {
                        "page_acquired": page_acquired,
                        "vm_acquired": vm_acquired,
                    },
                    "terminal_cleanup_receipt": terminal_cleanup_receipt,
                }));
            }
            Some("cleanup_pending") | Some("effect_acquired") => {
                if let Ok((page_id, session)) = page_control_session_from_durable_reconciliation(
                    &reconciliation,
                    &selected_adapter,
                    DurableReconciliationBinding {
                        control_socket_path: &control_socket_path,
                        live_control_service: &live_control_service,
                        principal_id: principal_id.as_deref(),
                        lifecycle_generation,
                        stream_id,
                        transport_authority,
                    },
                ) {
                    if !self.page_control_sessions.contains_key(&page_id) {
                        self.page_control_sessions.insert(page_id.clone(), session);
                        let session = self
                            .page_control_sessions
                            .get(&page_id)
                            .expect("reconciled Browser page session");
                        return reconcile_ok(json!({
                            "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                            "state": "effect_acquired",
                            "lifecycle_generation": lifecycle_generation,
                            "stream_id": stream_id,
                            "effect": {
                                "provider": BROWSER_ENGINE_PROVIDER_ID,
                                "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
                                "page_id": page_id,
                                "adapter": session.adapter_id,
                                "engine": session.engine,
                                "stream_id": session.stream_id,
                                "transport_authority": session.transport_authority,
                                "transport_receipt": session.transport_receipt,
                                "runtime_cleanup": engine_cleanup_binding(&page_id, session),
                            },
                        }));
                    }
                }
            }
            _ => {}
        }

        reconcile_ok(json!({
            "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
            "state": "cleanup_pending",
            "lifecycle_generation": lifecycle_generation,
            "stream_id": stream_id,
            "reason": "no exact provider or control-service launch proof is currently available",
        }))
    }

    #[cfg(test)]
    fn launch(
        &mut self,
        url: &str,
        stream_session: &StreamSessionReceipt,
        principal_id: Option<String>,
        reason: Option<String>,
        wallet: Value,
    ) -> Response {
        self.launch_with_viewport(LaunchContext {
            url,
            stream_session,
            profile: BrowserProfileDescriptor {
                schema: "elastos.browser.profile/v1".to_string(),
                scope: "active_principal".to_string(),
                storage: "principal_owned_profile_disk".to_string(),
                storage_posture: "principal_owned_reset_scoped_unprotected".to_string(),
                protected_storage: false,
                encrypted: false,
                recoverable: false,
                recovery: "not_recovery_kit_packaged".to_string(),
                uri: "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4"
                    .to_string(),
                public_uri: "localhost://Users/self/BrowserProfiles/default/profile.ext4"
                    .to_string(),
                profile_key:
                    "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                        .to_string(),
                disk_path: "/tmp/elastos-browser-profile-test/BrowserProfiles/default/profile.ext4"
                    .to_string(),
                reset: "whole_profile".to_string(),
            },
            adapter_id: None,
            principal_id,
            reason,
            wallet,
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })
    }

    #[cfg(test)]
    fn launch_with_viewport(&mut self, context: LaunchContext<'_>) -> Response {
        self.launch_with_generation(context, "sha256:test-lifecycle-generation", None)
    }

    fn launch_with_generation(
        &mut self,
        context: LaunchContext<'_>,
        lifecycle_generation: &str,
        transport: Option<VzTransportLaunchContext>,
    ) -> Response {
        if !is_safe_id(lifecycle_generation) || lifecycle_generation.len() > 256 {
            return Response::error(
                "invalid_request",
                "lifecycle_generation must be a bounded safe Runtime identifier",
            );
        }
        if let Some(adapter_id) = context
            .adapter_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            if !is_safe_id(adapter_id) {
                return Response::error("invalid_request", "adapter_id must be a safe identifier");
            }
        }
        let Some(adapter) = self.select_adapter(context.adapter_id.as_deref()) else {
            let message = if self.adapters.is_empty() {
                "No Browser Engine Adapter is configured; refusing to launch host browser engine"
            } else {
                "Requested Browser Engine Adapter is not configured"
            };
            return Response::error("engine_unavailable", message);
        };
        if let Some(transport) = transport.as_ref() {
            if adapter.kind != AdapterKind::ChromiumMicrovm {
                return Response::error(
                    "invalid_request",
                    "Browser VZ transport authority is valid only for chromium_microvm",
                );
            }
            if let Err(message) = validate_vz_transport_launch(
                &transport.authority,
                &transport.secret,
                lifecycle_generation,
                &context.stream_session.stream_id,
                &transport.page_id,
                &transport.vm_id,
                context.principal_id.as_deref(),
            ) {
                return Response::error("invalid_request", message);
            }
        }
        if let Err(err) = validate_url(context.url) {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_stream_session(context.stream_session) {
            return Response::error("invalid_stream_session", err);
        }
        if let Err(err) = validate_browser_profile(&context.profile) {
            return Response::error("invalid_profile", err);
        }
        if let Some(viewport) = context.viewport {
            if let Err(err) = validate_viewport(viewport) {
                return Response::error("invalid_request", err);
            }
        }
        if context.stream_session.byte_transport != "adapter_ipc" {
            return Response::error(
                "byte_transport_unavailable",
                "Browser Engine Adapter requires an attached adapter_ipc byte transport",
            );
        }
        self.reconcile_page_control_sessions();
        if self.page_control_sessions.len() >= self.max_active_sessions {
            return Response::error(
                "browser_capacity_unavailable",
                format!(
                    "Browser Engine Adapter has reached its active session limit ({})",
                    self.max_active_sessions
                ),
            );
        }
        if !adapter.display_modes.contains(&context.display_mode) {
            return Response::error(
                "display_session_unavailable",
                format!(
                    "{} display sessions are not declared by adapter {}",
                    context.display_mode.as_str(),
                    adapter.id
                ),
            );
        }
        if !adapter_supports_guarantee(adapter.kind, context.guarantee_level) {
            return Response::error(
                "guarantee_unavailable",
                format!(
                    "{} guarantee is not declared by adapter {}",
                    context.guarantee_level.as_str(),
                    adapter.id
                ),
            );
        }
        if adapter.kind != AdapterKind::ContractProof {
            return self.launch_with_supervisor(
                &adapter,
                context,
                lifecycle_generation,
                transport.as_ref(),
            );
        }
        let page_id = stable_page_id(context.url, &context.stream_session.stream_id);
        let view = context.viewport.map(|viewport| {
            json!({
                "schema": "elastos.browser.view/v1",
                "mode": "webrtc_remote_display",
                "width": viewport.width,
                "height": viewport.height,
            })
        });
        Response::ok(json!({
            "schema": "elastos.browser.engine.page/v1",
            "provider": BROWSER_ENGINE_PROVIDER_ID,
            "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
            "page_id": page_id,
            "adapter": adapter.id,
            "engine": adapter.kind,
            "url": context.url,
            "stream_id": context.stream_session.stream_id,
            "principal_id": context.principal_id,
            "reason": context.reason,
            "rendering": "reserved",
            "direct_network": false,
            "wallet_injection": false,
            "network_mode": "runtime_net_only",
            "display_session": display_session_receipt(BrowserDisplayMode::WebrtcRemoteDisplay, &context.stream_session.stream_id, &page_id),
            "view": view,
        }))
    }

    fn launch_with_supervisor(
        &mut self,
        adapter: &AdapterConfig,
        context: LaunchContext<'_>,
        lifecycle_generation: &str,
        transport: Option<&VzTransportLaunchContext>,
    ) -> Response {
        let Some(supervisor) = &adapter.supervisor else {
            return Response::error(
                "engine_process_unavailable",
                "Native Browser Engine Adapter requires an operator-approved supervisor",
            );
        };
        let result = match run_supervisor_launch(
            supervisor,
            adapter,
            &context,
            lifecycle_generation,
            transport,
        ) {
            Ok(result) => result,
            Err(err) => return Response::supervisor_error(adapter, err),
        };
        if let Err(err) = validate_supervisor_result(
            &result,
            adapter,
            &context.stream_session.stream_id,
            context.display_mode,
            transport,
        ) {
            return Response::error("invalid_supervisor_result", err);
        }
        let supervisor_control_socket_path = supervisor.control_socket_path.clone();
        let control_socket_path = result
            .control_socket_path
            .clone()
            .or_else(|| supervisor_control_socket_path.clone());
        if let Some(socket_path) = &control_socket_path {
            if let Err(err) = validate_control_socket_path(socket_path) {
                return Response::error("invalid_supervisor_result", err);
            }
            let shutdown_socket_path = if result
                .isolation
                .as_ref()
                .map(|isolation| isolation.kind.as_str())
                == Some("per_launch_vm_target")
            {
                supervisor_control_socket_path.clone()
            } else {
                None
            };
            if let Some(shutdown_socket_path) = &shutdown_socket_path {
                if let Err(err) = validate_control_socket_path(shutdown_socket_path) {
                    return Response::error("invalid_supervisor_result", err);
                }
            }
            let control_service = if shutdown_socket_path.is_some() {
                let Some(identity) = result.control_service.clone() else {
                    return Response::error(
                        "invalid_supervisor_result",
                        "Browser VM supervisor omitted its control-service identity",
                    );
                };
                if let Err(err) =
                    validate_control_service_identity(&identity, shutdown_socket_path.as_deref())
                {
                    return Response::error("invalid_supervisor_result", err);
                }
                let live_status = supervisor_control_json_bounded(
                    shutdown_socket_path
                        .as_deref()
                        .expect("VM shutdown socket path"),
                    "GET",
                    "/status",
                    None,
                    BROWSER_ENGINE_RECONCILIATION_TIMEOUT,
                    MAX_BROWSER_ENGINE_RECONCILIATION_RESPONSE_BYTES,
                );
                let live_identity = live_status.and_then(|status| {
                    if status.get("schema").and_then(Value::as_str)
                        != Some("elastos.browser.vm-control-service.status/v1")
                    {
                        return Err(
                            "Browser VM control service returned an invalid status schema"
                                .to_string(),
                        );
                    }
                    serde_json::from_value::<ControlServiceIdentity>(
                        status
                            .get("control_service")
                            .cloned()
                            .unwrap_or(Value::Null),
                    )
                    .map_err(|_| {
                        "Browser VM live control-service identity is unavailable".to_string()
                    })
                });
                if live_identity.as_ref().ok() != Some(&identity) {
                    return Response::error(
                        "invalid_supervisor_result",
                        "Browser VM control-service identity changed after dispatch",
                    );
                }
                Some(identity)
            } else {
                result.control_service.clone()
            };
            self.page_control_sessions.insert(
                result.page_id.clone(),
                PageControlSession {
                    display_attachment: BrowserDisplayAttachment::initial(
                        result
                            .display_session
                            .get("display_generation")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    ),
                    generation: lifecycle_generation.to_string(),
                    stream_id: context.stream_session.stream_id.clone(),
                    socket_path: socket_path.clone(),
                    shutdown_socket_path,
                    adapter_id: adapter.id.clone(),
                    principal_id: context.principal_id.clone(),
                    engine: adapter.kind,
                    display_mode: context.display_mode,
                    guarantee_level: context.guarantee_level,
                    isolated_session: result.isolated_session,
                    isolation_session_dir: result
                        .isolation
                        .as_ref()
                        .map(|isolation| isolation.session_dir.clone()),
                    isolation_kind: result
                        .isolation
                        .as_ref()
                        .map(|isolation| isolation.kind.clone()),
                    control_service,
                    process: result.process.clone(),
                    transport_authority: result.transport_authority.clone(),
                    transport_receipt: result.transport_receipt.clone(),
                },
            );
        }
        let runtime_cleanup = self
            .page_control_sessions
            .get(&result.page_id)
            .map(|session| engine_cleanup_binding(&result.page_id, session));
        Response::ok(json!({
            "schema": "elastos.browser.engine.page/v1",
            "provider": BROWSER_ENGINE_PROVIDER_ID,
            "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
            "page_id": result.page_id,
            "adapter": adapter.id,
            "engine": adapter.kind,
            "url": context.url,
            "actual_url": result.actual_url,
            "title": result.title,
            "stream_id": context.stream_session.stream_id,
            "principal_id": context.principal_id,
            "reason": context.reason,
            "rendering": "host_supervisor",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "display_session": result.display_session,
            "view": result.view,
            "wallet_bridge": result.wallet_bridge,
            "engine_control": if control_socket_path.is_some() { "page_scoped" } else { "unavailable" },
            "isolated_engine_session": result.isolated_session,
            "isolation": result.isolation,
            "transport_authority": result.transport_authority,
            "transport_receipt": result.transport_receipt,
            "runtime_cleanup": runtime_cleanup,
        }))
    }

    fn attach_stream(
        &self,
        page_id: &str,
        stream_session: &StreamSessionReceipt,
        principal_id: Option<String>,
    ) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        if self.adapters.is_empty() {
            return Response::error(
                "engine_unavailable",
                "No Browser Engine Adapter is configured for stream attachment",
            );
        }
        if let Err(err) = validate_stream_session(stream_session) {
            return Response::error("invalid_stream_session", err);
        }
        if stream_session.byte_transport != "adapter_ipc" {
            return Response::error(
                "byte_transport_unavailable",
                "Cannot attach a Browser Engine Adapter to a stream without adapter_ipc transport",
            );
        }
        Response::ok(json!({
            "attached": true,
            "page_id": page_id,
            "stream_id": stream_session.stream_id,
            "principal_id": principal_id,
        }))
    }

    fn close_page(
        &mut self,
        page_id: &str,
        principal_id: Option<String>,
        runtime_cleanup: EngineCleanupBinding,
    ) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        if runtime_cleanup.page_id != page_id {
            return Response::error("invalid_request", "Browser cleanup page binding changed");
        }
        let bound_session = match page_control_session_from_cleanup(&runtime_cleanup) {
            Ok(session) => session,
            Err(err) => return Response::error("invalid_request", err),
        };
        let session = match self.page_control_sessions.get(page_id) {
            Some(session) => {
                if engine_cleanup_binding(page_id, session) != runtime_cleanup {
                    return Response::error(
                        "cleanup_binding_mismatch",
                        "Browser cleanup binding does not match the active provider effect",
                    );
                }
                session.clone()
            }
            None => bound_session,
        };
        if !page_control_session_principal_matches(&session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        if session.isolated_session {
            let shutdown_socket_path = session
                .shutdown_socket_path
                .clone()
                .unwrap_or_else(|| session.socket_path.clone());
            let shutdown_result = supervisor_control_json(
                &shutdown_socket_path,
                "POST",
                "/shutdown",
                Some(json!({
                    "page_id": page_id,
                    "principal_id": principal_id,
                    "runtime_cleanup": runtime_cleanup,
                    "force_retire_vm": true,
                })),
            );
            return match shutdown_result {
                Ok(data) => {
                    match engine_terminal_cleanup_result(&runtime_cleanup, data) {
                        Ok(terminal) => {
                            self.page_control_sessions.remove(page_id);
                            Response::ok(terminal)
                        }
                        Err(proof_err) => {
                            match cleanup_isolated_session(&session, &runtime_cleanup) {
                            Ok(data) => match engine_terminal_cleanup_result(
                                &runtime_cleanup,
                                data,
                            ) {
                                Ok(terminal) => {
                                    self.page_control_sessions.remove(page_id);
                                    Response::ok(terminal)
                                }
                                Err(cleanup_err) => Response::error(
                                    "engine_close_indeterminate",
                                    format!(
                                        "{proof_err}; deterministic cleanup proof invalid: {cleanup_err}"
                                    ),
                                ),
                            },
                            Err(cleanup_err) => Response::error(
                                "engine_close_indeterminate",
                                format!(
                                    "{proof_err}; deterministic cleanup failed: {cleanup_err}"
                                ),
                            ),
                        }
                        }
                    }
                }
                Err(err) => match cleanup_isolated_session(&session, &runtime_cleanup) {
                    Ok(data) => match engine_terminal_cleanup_result(&runtime_cleanup, data) {
                        Ok(terminal) => {
                            self.page_control_sessions.remove(page_id);
                            Response::ok(terminal)
                        }
                        Err(cleanup_err) => Response::error(
                            "engine_close_indeterminate",
                            format!("{err}; cleanup proof invalid: {cleanup_err}"),
                        ),
                    },
                    Err(cleanup_err) => Response::error(
                        "engine_process_unavailable",
                        format!("{err}; cleanup failed: {cleanup_err}"),
                    ),
                },
            };
        }
        let socket_path = session.socket_path.clone();
        let body = json!({
            "page_id": page_id,
            "principal_id": principal_id,
            "runtime_cleanup": runtime_cleanup,
        });
        let close_result = match supervisor_control_json(
            &socket_path,
            "POST",
            &format!("/pages/{page_id}/close"),
            Some(body),
        ) {
            Ok(data) => data,
            Err(err) => return Response::error("engine_process_unavailable", err),
        };
        let terminal = match engine_terminal_cleanup_result(&runtime_cleanup, close_result) {
            Ok(result) => result,
            Err(err) => return Response::error("engine_close_indeterminate", err),
        };
        self.page_control_sessions.remove(page_id);
        Response::ok(terminal)
    }

    fn page_status(&self, page_id: &str, principal_id: Option<String>) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        let Some(session) = self.page_control_session(page_id) else {
            return Response::error(
                "engine_process_unavailable",
                "Browser page has no page-scoped engine control session",
            );
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        match supervisor_control_json(
            &session.socket_path,
            "GET",
            &format!("/pages/{page_id}/status"),
            None,
        ) {
            Ok(mut data) => {
                let transport_proof = match (
                    session.transport_authority.as_ref(),
                    session.transport_receipt.as_ref(),
                ) {
                    (Some(authority), Some(receipt)) => {
                        match vz_public_transport_proof(authority, receipt) {
                            Ok(proof) => Some(proof),
                            Err(err) => return Response::error("engine_process_unavailable", err),
                        }
                    }
                    (None, None) => None,
                    _ => {
                        return Response::error(
                            "engine_process_unavailable",
                            "Browser VZ transport proof is incomplete",
                        )
                    }
                };
                if let Some(object) = data.as_object_mut() {
                    let mut identity = json!({
                        "schema": "elastos.browser.engine.identity/v1",
                        "adapter": session.adapter_id.as_str(),
                        "engine": session.engine,
                        "display_mode": session.display_mode.as_str(),
                        "guarantee_level": session.guarantee_level.as_str(),
                        "engine_control": "page_scoped",
                        "isolated_engine_session": session.isolated_session,
                        "isolation_kind": session.isolation_kind.as_deref(),
                    });
                    if let Some(proof) = transport_proof {
                        identity["transport_proof"] = proof;
                    }
                    object.insert("engine_identity".to_string(), identity);
                }
                Response::ok(data)
            }
            Err(err) => Response::error("engine_process_unavailable", err),
        }
    }

    fn diagnostics(&self, page_id: &str, principal_id: Option<String>) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        let Some(session) = self.page_control_session(page_id) else {
            return Response::error(
                "engine_process_unavailable",
                "Browser page has no page-scoped engine control session",
            );
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        match supervisor_control_json(
            &session.socket_path,
            "GET",
            &format!("/pages/{page_id}/diagnostics"),
            None,
        ) {
            Ok(data) => Response::ok(data),
            Err(err) => Response::error("engine_process_unavailable", err),
        }
    }

    fn inspect(
        &self,
        page_id: &str,
        principal_id: Option<String>,
        request: Option<BrowserInspectionRequest>,
    ) -> Response {
        let error = |error: BrowserInspectionError| {
            Response::error(error.code(), "Browser page inspection could not complete.")
        };
        if !is_safe_id(page_id) {
            return error(BrowserInspectionError::Invalid);
        }
        let Some(session) = self.page_control_session(page_id) else {
            return Response::error("page_not_found", "browser page not found");
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        if let Some(request) = request.as_ref() {
            if let Err(reason) = request.validate() {
                return error(reason);
            }
        }
        let body = request
            .as_ref()
            .map(|request| serde_json::to_value(request).expect("inspection request serializes"));
        match supervisor_control_json_bounded(
            &session.socket_path,
            if body.is_some() { "POST" } else { "GET" },
            &format!("/pages/{page_id}/inspect"),
            body,
            std::time::Duration::from_millis(2500),
            BROWSER_INSPECTION_MAX_RESPONSE_BYTES + 4096,
        ) {
            Ok(data) => match validate_browser_inspection_result(page_id, request.as_ref(), data) {
                Ok(data) => Response::ok(data),
                Err(reason) => error(reason),
            },
            Err(reason) => error(
                BrowserInspectionError::from_code(&reason)
                    .unwrap_or(BrowserInspectionError::Failed),
            ),
        }
    }

    fn input(&self, page_id: &str, event: Value, principal_id: Option<String>) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        let Some(session) = self.page_control_session(page_id) else {
            return Response::error(
                "engine_process_unavailable",
                "Browser page has no page-scoped engine control session",
            );
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        if event
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("operator_"))
        {
            if !elastos_common::browser_protocol::browser_operator_event_valid(&event) {
                return Response::error(
                    "invalid_operator_input",
                    "Browser operator input is invalid",
                );
            }
            return match supervisor_control_json_bounded(
                &session.socket_path,
                "POST",
                &format!("/pages/{page_id}/input"),
                Some(json!({"event":event,"principal_id":principal_id})),
                std::time::Duration::from_millis(2500),
                8192,
            ) {
                Ok(data) => Response::ok(data),
                Err(_) => Response::error(
                    "operator_outcome_uncertain",
                    "Browser operator input requires reconciliation",
                ),
            };
        }
        match supervisor_control_json(
            &session.socket_path,
            "POST",
            &format!("/pages/{page_id}/input"),
            Some(json!({
                "event": event,
                "principal_id": principal_id,
            })),
        ) {
            Ok(data) => Response::ok(data),
            Err(err) => Response::error("engine_process_unavailable", err),
        }
    }

    fn webrtc_signal(
        &mut self,
        page_id: &str,
        signal: Value,
        channel: Option<String>,
        principal_id: Option<String>,
    ) -> Response {
        if !is_safe_id(page_id) {
            return Response::error("invalid_request", "page_id must be a safe identifier");
        }
        let signal_type = match validate_webrtc_signal(&signal) {
            Ok(signal_type) => signal_type,
            Err(err) => {
                return Response::error("invalid_request", err);
            }
        };
        let Some(session) = self.page_control_sessions.get_mut(page_id) else {
            return Response::error(
                "engine_process_unavailable",
                "Browser page has no page-scoped engine control session",
            );
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        let display_error =
            |error: BrowserDisplayError| Response::error(error.code(), error.message());
        let generation = signal.get("display_generation").and_then(Value::as_str);
        let request_id = signal
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let attach = signal_type == "display_attach";
        let channel = match channel.as_deref() {
            None if attach => None,
            Some("audio") if !attach => Some("audio"),
            Some("video") | None if !attach => Some("video"),
            _ => {
                return Response::error(
                    "invalid_request",
                    "WebRTC channel must be video or audio; display attach replaces both",
                )
            }
        };
        if attach {
            match session
                .display_attachment
                .begin(request_id, generation.unwrap_or(""))
            {
                Ok(Some(cached)) => return Response::ok(cached),
                Ok(None) => {}
                Err(error) => return display_error(error),
            }
        } else if let Err(error) = session.display_attachment.check_signal(generation) {
            return display_error(error);
        }
        let mut body = json!({"signal": signal, "principal_id": principal_id});
        if let Some(channel) = channel {
            body["channel"] = json!(channel);
        }
        let response = if attach {
            supervisor_control_json_bounded(
                &session.socket_path,
                "POST",
                &format!("/pages/{page_id}/webrtc"),
                Some(body),
                std::time::Duration::from_secs(5),
                1024 * 1024,
            )
        } else {
            supervisor_control_json(
                &session.socket_path,
                "POST",
                &format!("/pages/{page_id}/webrtc"),
                Some(body),
            )
        };
        let outcome = match response {
            Ok(data) => Ok(data),
            Err(error) => match BrowserDisplayError::from_code(&error) {
                Some(error) => Err(error),
                None if attach => Err(BrowserDisplayError::Uncertain),
                None => return Response::error("engine_process_unavailable", error),
            },
        };
        let outcome = if attach {
            session.display_attachment.finish(
                page_id,
                request_id,
                generation.unwrap_or(""),
                outcome,
            )
        } else {
            outcome.and_then(|data| {
                validate_webrtc_response(signal_type, &data)
                    .map_err(|_| BrowserDisplayError::Uncertain)?;
                if let Some(generation) = generation {
                    if data.get("display_generation").and_then(Value::as_str) != Some(generation)
                        || data.get("page_id").and_then(Value::as_str) != Some(page_id)
                        || (signal_type != "offer" && data.get("accepted") != Some(&json!(true)))
                    {
                        return Err(BrowserDisplayError::GenerationMismatch);
                    }
                }
                Ok(data)
            })
        };
        match outcome {
            Ok(data) => Response::ok(data),
            Err(error) => display_error(error),
        }
    }

    fn page_control_session(&self, page_id: &str) -> Option<&PageControlSession> {
        self.page_control_sessions.get(page_id)
    }

    fn reconcile_page_control_sessions(&mut self) -> usize {
        let stale_page_ids = self
            .page_control_sessions
            .iter()
            .filter(|(page_id, session)| Self::page_control_session_is_stale(page_id, session))
            .map(|(page_id, _)| page_id.clone())
            .collect::<Vec<_>>();
        let stale_count = stale_page_ids.len();
        for page_id in stale_page_ids {
            self.page_control_sessions.remove(&page_id);
        }
        stale_count
    }

    fn page_control_session_is_stale(page_id: &str, session: &PageControlSession) -> bool {
        if !session.isolated_session {
            return false;
        }
        let status_socket_path = session
            .shutdown_socket_path
            .as_deref()
            .unwrap_or(&session.socket_path);
        let Ok(status) = supervisor_control_json(status_socket_path, "GET", "/status", None) else {
            return true;
        };
        if let Some(page_ids) = status.get("page_ids").and_then(|value| value.as_array()) {
            return !page_ids.iter().any(|value| value.as_str() == Some(page_id));
        }
        status.get("active_pages").and_then(|value| value.as_u64()) == Some(0)
    }

    fn supported_display_modes(&self) -> Vec<&'static str> {
        let mut modes = BTreeSet::new();
        for adapter in &self.adapters {
            for mode in &adapter.display_modes {
                modes.insert(mode.as_str());
            }
        }
        modes.into_iter().collect()
    }

    fn select_adapter(&self, adapter_id: Option<&str>) -> Option<AdapterConfig> {
        let Some(adapter_id) = adapter_id.filter(|value| !value.is_empty()) else {
            return self.adapters.first().cloned();
        };
        if !is_safe_id(adapter_id) {
            return None;
        }
        self.adapters
            .iter()
            .find(|adapter| adapter.id == adapter_id)
            .cloned()
    }

    fn adapter_summaries(&self) -> Vec<BrowserEngineAdapterCapabilities> {
        self.adapters
            .iter()
            .enumerate()
            .map(|(index, adapter)| BrowserEngineAdapterCapabilities {
                id: adapter.id.clone(),
                engine: serde_json::to_value(adapter.kind)
                    .expect("AdapterKind serializes as a string")
                    .as_str()
                    .expect("AdapterKind has a string representation")
                    .to_string(),
                default: index == 0,
                supported_display_modes: adapter.display_modes.clone(),
                supported_guarantee_levels: adapter_guarantee_levels(adapter.kind),
                backing_substrate: adapter_backing_substrate(adapter).to_string(),
                network_mode: "runtime_net_only".to_string(),
                direct_network: false,
                wallet_injection: false,
            })
            .collect()
    }

    fn supported_guarantee_levels(&self) -> Vec<&'static str> {
        let mut levels = BTreeSet::new();
        for adapter in &self.adapters {
            for level in adapter_guarantee_levels(adapter.kind) {
                levels.insert(level.as_str());
            }
        }
        levels.into_iter().collect()
    }
}

fn adapter_guarantee_levels(kind: AdapterKind) -> Vec<BrowserGuaranteeLevel> {
    match kind {
        AdapterKind::ChromiumMicrovm => vec![BrowserGuaranteeLevel::MechanismMicrovm],
        AdapterKind::SelkiesGstreamer
        | AdapterKind::HostedRemoteBrowser
        | AdapterKind::ChromiumHeadless
        | AdapterKind::ContractProof => vec![BrowserGuaranteeLevel::OperatorRbi],
        AdapterKind::Cef
        | AdapterKind::Webview2
        | AdapterKind::Geckoview
        | AdapterKind::Wkwebview => vec![BrowserGuaranteeLevel::PolicyWebview],
    }
}

fn adapter_backing_substrate(adapter: &AdapterConfig) -> &'static str {
    match adapter.kind {
        AdapterKind::ChromiumMicrovm if adapter_uses_remote_vz_launcher(adapter) => {
            "remote_operator_vm"
        }
        AdapterKind::ChromiumMicrovm => "local_microvm",
        AdapterKind::Cef
        | AdapterKind::Webview2
        | AdapterKind::Geckoview
        | AdapterKind::Wkwebview => "host_policy_webview",
        AdapterKind::SelkiesGstreamer
        | AdapterKind::HostedRemoteBrowser
        | AdapterKind::ChromiumHeadless
        | AdapterKind::ContractProof => "operator_rbi",
    }
}

fn adapter_uses_remote_vz_launcher(adapter: &AdapterConfig) -> bool {
    let launcher = adapter
        .supervisor
        .as_ref()
        .and_then(|supervisor| {
            supervisor
                .env
                .get("ELASTOS_BROWSER_VM_CONTROL_LAUNCHER")
                .or(Some(&supervisor.program))
        })
        .map(String::as_str)
        .unwrap_or_default();
    std::path::Path::new(launcher)
        .file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.starts_with("browser-vm-remote-vz-launcher"))
        .unwrap_or(false)
}

fn page_control_session_principal_matches(
    session: &PageControlSession,
    principal_id: Option<&str>,
) -> bool {
    match session.principal_id.as_deref() {
        Some(owner) => principal_id == Some(owner),
        None => true,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineConfig {
    #[serde(default)]
    adapters: Vec<AdapterConfig>,
    #[serde(default = "default_max_active_sessions")]
    max_active_sessions: usize,
}

fn default_max_active_sessions() -> usize {
    4
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterConfig {
    id: String,
    kind: AdapterKind,
    #[serde(default)]
    network_mode: AdapterNetworkMode,
    display_modes: Vec<BrowserDisplayMode>,
    #[serde(default)]
    supervisor: Option<EngineSupervisorConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineSupervisorConfig {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_supervisor_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    control_socket_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterKind {
    Cef,
    ChromiumHeadless,
    ChromiumMicrovm,
    SelkiesGstreamer,
    HostedRemoteBrowser,
    Webview2,
    Geckoview,
    Wkwebview,
    ContractProof,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterNetworkMode {
    RuntimeNetOnly,
}

impl Default for AdapterNetworkMode {
    fn default() -> Self {
        Self::RuntimeNetOnly
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamSessionReceipt {
    schema: String,
    stream_id: String,
    target: String,
    byte_transport: String,
    #[serde(default)]
    adapter_ipc: Option<AdapterIpcEndpoint>,
    #[serde(default)]
    relay_ipc: Option<RelayIpcEndpoint>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdapterIpcEndpoint {
    schema: String,
    kind: AdapterIpcKind,
    path: String,
    stream_id: String,
    #[serde(default)]
    runtime_stream_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelayIpcEndpoint {
    schema: String,
    kind: AdapterIpcKind,
    path: String,
    #[serde(default)]
    stream_id: Option<String>,
}

fn adapter_supports_guarantee(kind: AdapterKind, guarantee_level: BrowserGuaranteeLevel) -> bool {
    matches!(
        (kind, guarantee_level),
        (
            AdapterKind::ChromiumMicrovm,
            BrowserGuaranteeLevel::MechanismMicrovm
        ) | (
            AdapterKind::SelkiesGstreamer,
            BrowserGuaranteeLevel::OperatorRbi
        ) | (
            AdapterKind::HostedRemoteBrowser,
            BrowserGuaranteeLevel::OperatorRbi
        ) | (
            AdapterKind::ContractProof,
            BrowserGuaranteeLevel::OperatorRbi
        ) | (
            AdapterKind::ChromiumHeadless,
            BrowserGuaranteeLevel::OperatorRbi
        ) | (AdapterKind::Cef, BrowserGuaranteeLevel::PolicyWebview)
            | (AdapterKind::Webview2, BrowserGuaranteeLevel::PolicyWebview)
            | (AdapterKind::Geckoview, BrowserGuaranteeLevel::PolicyWebview)
            | (AdapterKind::Wkwebview, BrowserGuaranteeLevel::PolicyWebview)
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorLaunchResult {
    schema: String,
    page_id: String,
    adapter: String,
    engine: AdapterKind,
    stream_id: String,
    #[serde(default)]
    vm_id: Option<String>,
    #[serde(default)]
    actual_url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    network_mode: AdapterNetworkMode,
    direct_network: bool,
    wallet_injection: bool,
    display_session: Value,
    #[serde(default)]
    view: Option<Value>,
    #[serde(default)]
    wallet_bridge: Option<Value>,
    #[serde(default)]
    control_socket_path: Option<String>,
    #[serde(default)]
    isolated_session: bool,
    #[serde(default)]
    isolation: Option<SupervisorIsolation>,
    #[serde(default)]
    control_service: Option<ControlServiceIdentity>,
    #[serde(default)]
    process: Option<Value>,
    #[serde(default)]
    transport_authority: Option<Value>,
    #[serde(default)]
    transport_receipt: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SupervisorIsolation {
    schema: String,
    kind: String,
    session_dir: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterIpcKind {
    UnixSocket,
}

fn decode_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}

fn main() {
    eprintln!(
        "browser-engine-adapter: starting v{} (engine process required)",
        PROVIDER_VERSION
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = BrowserEngineAdapter::new();

    for line in stdin.lock().lines() {
        let response = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => match decode_request(&line) {
                Ok(Request::Shutdown) => {
                    let response = Response::empty_ok();
                    let _ = write_response(&mut stdout, &response);
                    break;
                }
                Ok(request) => provider.handle(request),
                Err(err) => Response::error("invalid_request", err.to_string()),
            },
            Err(err) => Response::error("stdin_error", err.to_string()),
        };

        if write_response(&mut stdout, &response).is_err() {
            break;
        }
    }

    eprintln!("browser-engine-adapter: exiting");
}

fn write_response(stdout: &mut io::Stdout, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()
}

#[cfg(test)]
mod tests;
