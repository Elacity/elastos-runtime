//! ElastOS browser-engine-adapter Capsule
//!
//! Internal contract between the Browser UI capsule and a real browser engine
//! host adapter. The provider never gives browser capsules raw host network,
//! wallet, or browser-engine authority; configured host adapters attach only
//! through Runtime-owned stream and display sessions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};

mod display;
mod ids;
mod supervisor;
mod validation;

use display::*;
use ids::*;
use supervisor::*;
use validation::*;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const BROWSER_ENGINE_PROVIDER_ID: &str = "browser-engine-adapter";
const BROWSER_ENGINE_PROTOCOL_VERSION: &str = "2.0";
const BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA: &str = "elastos.browser.engine-cleanup-binding/v2";
const BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA: &str = "elastos.browser.engine-cleanup-result/v2";
const BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA: &str =
    "elastos.browser.engine.launch-reconciliation/v1";
const BROWSER_ENGINE_RECONCILIATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(1);
const BROWSER_ENGINE_RECONCILIATION_TOTAL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(2);
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
    Status {
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        lifecycle_generation: Option<String>,
        #[serde(default)]
        stream_id: Option<String>,
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
    #[serde(default)]
    process: Option<Value>,
}

#[derive(Debug, Clone)]
struct PageControlSession {
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
    process: Option<Value>,
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
        process: session.process.clone(),
    }
}

fn page_control_session_from_cleanup(
    binding: &EngineCleanupBinding,
) -> Result<PageControlSession, String> {
    validate_engine_cleanup_binding(binding)?;
    Ok(PageControlSession {
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
        process: binding.process.clone(),
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
    }
    if serde_json::to_vec(binding).map_or(true, |bytes| bytes.len() > 16 * 1024) {
        return Err("Runtime Browser cleanup binding is too large".to_string());
    }
    Ok(())
}

fn engine_terminal_cleanup_result(
    binding: &EngineCleanupBinding,
    supervisor_receipt: Value,
) -> Result<Value, String> {
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
            Request::Status {
                principal_id,
                lifecycle_generation,
                stream_id,
            } => self.status_request(principal_id, lifecycle_generation, stream_id),
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
            } => self.launch_with_generation(
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
            ),
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
                    return Response::error(
                        "engine_process_unavailable",
                        format!(
                            "Browser Engine Adapter prewarm failed for {}: {err}",
                            adapter.id
                        ),
                    )
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
    ) -> Response {
        if lifecycle_generation.is_some() || stream_id.is_some() {
            let (Some(lifecycle_generation), Some(stream_id)) = (lifecycle_generation, stream_id)
            else {
                return Response::error(
                    "invalid_request",
                    "Browser launch reconciliation requires lifecycle_generation and stream_id",
                );
            };
            return self.reconcile_launch(principal_id, &lifecycle_generation, &stream_id);
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
            "operations": ["status", "launch", "attach_stream", "close_page", "page_status", "diagnostics", "input", "webrtc_signal"],
        }))
    }

    fn reconcile_launch(
        &mut self,
        principal_id: Option<String>,
        lifecycle_generation: &str,
        stream_id: &str,
    ) -> Response {
        if !is_safe_id(lifecycle_generation)
            || lifecycle_generation.len() > 256
            || !is_safe_id(stream_id)
            || stream_id.len() > 256
        {
            return Response::error(
                "invalid_request",
                "Browser launch reconciliation identity is invalid",
            );
        }

        let exact_sessions = self
            .page_control_sessions
            .iter()
            .filter(|(_, session)| {
                session.generation == lifecycle_generation && session.stream_id == stream_id
            })
            .collect::<Vec<_>>();
        if exact_sessions.len() > 1 {
            return Response::ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "provider has multiple effects for one lifecycle identity",
            }));
        }
        if let Some((page_id, session)) = exact_sessions.into_iter().next() {
            if !page_control_session_principal_matches(session, principal_id.as_deref()) {
                return Response::error(
                    "cleanup_binding_mismatch",
                    "Browser launch reconciliation principal changed",
                );
            }
            return Response::ok(json!({
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
                    "runtime_cleanup": engine_cleanup_binding(page_id, session),
                },
            }));
        }
        if self.page_control_sessions.values().any(|session| {
            session.generation == lifecycle_generation || session.stream_id == stream_id
        }) {
            return Response::ok(json!({
                "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                "state": "cleanup_pending",
                "lifecycle_generation": lifecycle_generation,
                "stream_id": stream_id,
                "reason": "provider lifecycle identity conflict",
            }));
        }

        let mut control_services = BTreeMap::<String, Vec<AdapterConfig>>::new();
        for adapter in self.adapters.clone() {
            if let Some(control_socket_path) = adapter
                .supervisor
                .as_ref()
                .and_then(|supervisor| supervisor.control_socket_path.as_ref())
            {
                control_services
                    .entry(control_socket_path.clone())
                    .or_default()
                    .push(adapter);
            }
        }
        let mut every_control_service_proved_terminal = !control_services.is_empty();
        let mut terminal_effects = None::<(bool, bool)>;
        let reconciliation_deadline =
            std::time::Instant::now() + BROWSER_ENGINE_RECONCILIATION_TOTAL_TIMEOUT;
        for (control_socket_path, adapters) in control_services {
            let remaining =
                reconciliation_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                every_control_service_proved_terminal = false;
                break;
            }
            let Ok(reconciliation) = supervisor_control_json_bounded(
                &control_socket_path,
                "POST",
                "/launches/reconcile",
                Some(json!({
                    "schema": "elastos.browser.vm-control-service.reconcile-launch/v1",
                    "lifecycle_generation": lifecycle_generation,
                    "stream_id": stream_id,
                })),
                BROWSER_ENGINE_RECONCILIATION_TIMEOUT.min(remaining),
                MAX_BROWSER_ENGINE_RECONCILIATION_RESPONSE_BYTES,
            ) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            if reconciliation.get("schema").and_then(Value::as_str)
                != Some("elastos.browser.vm-control-service.launch-reconciliation/v1")
                || reconciliation
                    .get("launch")
                    .and_then(|value| value.get("lifecycle_generation"))
                    .and_then(Value::as_str)
                    != Some(lifecycle_generation)
                || reconciliation
                    .get("launch")
                    .and_then(|value| value.get("stream_id"))
                    .and_then(Value::as_str)
                    != Some(stream_id)
            {
                every_control_service_proved_terminal = false;
                continue;
            }
            match reconciliation.get("state").and_then(Value::as_str) {
                Some("did_not_act") => {
                    let effects = reconciliation.get("effects");
                    if effects
                        .and_then(|effects| effects.get("page_acquired"))
                        .and_then(Value::as_bool)
                        != Some(false)
                        || effects
                            .and_then(|effects| effects.get("vm_acquired"))
                            .and_then(Value::as_bool)
                            != Some(false)
                    {
                        every_control_service_proved_terminal = false;
                    }
                    continue;
                }
                Some("terminal_post_effect_cleanup") => {
                    let effects = reconciliation.get("effects");
                    let Some(page_acquired) = effects
                        .and_then(|effects| effects.get("page_acquired"))
                        .and_then(Value::as_bool)
                    else {
                        every_control_service_proved_terminal = false;
                        continue;
                    };
                    let Some(vm_acquired) = effects
                        .and_then(|effects| effects.get("vm_acquired"))
                        .and_then(Value::as_bool)
                    else {
                        every_control_service_proved_terminal = false;
                        continue;
                    };
                    let aggregate = terminal_effects.get_or_insert((false, false));
                    aggregate.0 |= page_acquired;
                    aggregate.1 |= vm_acquired;
                    continue;
                }
                Some("cleanup_pending") => {
                    every_control_service_proved_terminal = false;
                    continue;
                }
                Some("effect_acquired") => {}
                _ => {
                    every_control_service_proved_terminal = false;
                    continue;
                }
            }

            let Some(launch) = reconciliation.get("launch") else {
                every_control_service_proved_terminal = false;
                continue;
            };
            let Some(adapter_id) = launch.get("adapter").and_then(Value::as_str) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            let Some(adapter) = adapters.iter().find(|adapter| adapter.id == adapter_id) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            let recovered_principal = launch
                .get("principal_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            if principal_id.as_deref() != recovered_principal.as_deref()
                || launch.get("engine").and_then(Value::as_str)
                    != serde_json::to_value(adapter.kind)
                        .ok()
                        .as_ref()
                        .and_then(Value::as_str)
            {
                every_control_service_proved_terminal = false;
                continue;
            }
            let Ok(display_mode) = serde_json::from_value::<BrowserDisplayMode>(
                launch.get("display_mode").cloned().unwrap_or(Value::Null),
            ) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            let Ok(guarantee_level) = serde_json::from_value::<BrowserGuaranteeLevel>(
                launch
                    .get("guarantee_level")
                    .cloned()
                    .unwrap_or(Value::Null),
            ) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            let Ok(result) = serde_json::from_value::<SupervisorLaunchResult>(
                reconciliation
                    .get("supervisor_result")
                    .cloned()
                    .unwrap_or(Value::Null),
            ) else {
                every_control_service_proved_terminal = false;
                continue;
            };
            if validate_supervisor_result(&result, adapter, stream_id, display_mode).is_err() {
                every_control_service_proved_terminal = false;
                continue;
            }
            let recovered_control_socket = result
                .control_socket_path
                .clone()
                .or_else(|| Some(control_socket_path.clone()));
            let Some(recovered_control_socket) = recovered_control_socket else {
                every_control_service_proved_terminal = false;
                continue;
            };
            if validate_control_socket_path(&recovered_control_socket).is_err() {
                every_control_service_proved_terminal = false;
                continue;
            }
            let shutdown_socket_path = (result
                .isolation
                .as_ref()
                .map(|isolation| isolation.kind.as_str())
                == Some("per_launch_vm_target"))
            .then(|| control_socket_path.clone());
            let page_id = result.page_id.clone();
            self.page_control_sessions.insert(
                page_id.clone(),
                PageControlSession {
                    generation: lifecycle_generation.to_string(),
                    stream_id: stream_id.to_string(),
                    socket_path: recovered_control_socket,
                    shutdown_socket_path,
                    adapter_id: adapter.id.clone(),
                    principal_id: recovered_principal,
                    engine: adapter.kind,
                    display_mode,
                    guarantee_level,
                    isolated_session: result.isolated_session,
                    isolation_session_dir: result
                        .isolation
                        .as_ref()
                        .map(|isolation| isolation.session_dir.clone()),
                    isolation_kind: result
                        .isolation
                        .as_ref()
                        .map(|isolation| isolation.kind.clone()),
                    process: result.process.clone(),
                },
            );
            let session = self
                .page_control_sessions
                .get(&page_id)
                .expect("reconciled Browser page session");
            return Response::ok(json!({
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
                    "runtime_cleanup": engine_cleanup_binding(&page_id, session),
                },
            }));
        }

        if every_control_service_proved_terminal {
            if let Some((page_acquired, vm_acquired)) = terminal_effects {
                return Response::ok(json!({
                    "schema": BROWSER_ENGINE_LAUNCH_RECONCILIATION_SCHEMA,
                    "state": "terminal_post_effect_cleanup",
                    "lifecycle_generation": lifecycle_generation,
                    "stream_id": stream_id,
                    "effects": {
                        "page_acquired": page_acquired,
                        "vm_acquired": vm_acquired,
                    },
                }));
            }
            return Response::ok(json!({
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

        Response::ok(json!({
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
        self.launch_with_generation(context, "sha256:test-lifecycle-generation")
    }

    fn launch_with_generation(
        &mut self,
        context: LaunchContext<'_>,
        lifecycle_generation: &str,
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
            return self.launch_with_supervisor(&adapter, context, lifecycle_generation);
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
    ) -> Response {
        let Some(supervisor) = &adapter.supervisor else {
            return Response::error(
                "engine_process_unavailable",
                "Native Browser Engine Adapter requires an operator-approved supervisor",
            );
        };
        let result =
            match run_supervisor_launch(supervisor, adapter, &context, lifecycle_generation) {
                Ok(result) => result,
                Err(err) => return Response::error(&err.code, err.message),
            };
        if let Err(err) = validate_supervisor_result(
            &result,
            adapter,
            &context.stream_session.stream_id,
            context.display_mode,
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
            self.page_control_sessions.insert(
                result.page_id.clone(),
                PageControlSession {
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
                    process: result.process.clone(),
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
                if let Some(object) = data.as_object_mut() {
                    object.insert(
                        "engine_identity".to_string(),
                        json!({
                            "schema": "elastos.browser.engine.identity/v1",
                            "adapter": session.adapter_id.as_str(),
                            "engine": session.engine,
                            "display_mode": session.display_mode.as_str(),
                            "guarantee_level": session.guarantee_level.as_str(),
                            "engine_control": "page_scoped",
                            "isolated_engine_session": session.isolated_session,
                            "isolation_kind": session.isolation_kind.as_deref(),
                        }),
                    );
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
        &self,
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
        let Some(session) = self.page_control_session(page_id) else {
            return Response::error(
                "engine_process_unavailable",
                "Browser page has no page-scoped engine control session",
            );
        };
        if !page_control_session_principal_matches(session, principal_id.as_deref()) {
            return Response::error("page_not_found", "browser page not found");
        }
        let channel = match channel.as_deref() {
            Some("audio") => "audio",
            Some("video") | None => "video",
            _ => {
                return Response::error("invalid_request", "WebRTC channel must be video or audio")
            }
        }
        .to_string();
        match supervisor_control_json(
            &session.socket_path,
            "POST",
            &format!("/pages/{page_id}/webrtc"),
            Some(json!({
                "signal": signal,
                "channel": channel,
                "principal_id": principal_id,
            })),
        ) {
            Ok(data) => match validate_webrtc_response(signal_type, &data) {
                Ok(()) => Response::ok(data),
                Err(err) => Response::error("invalid_engine_response", err),
            },
            Err(err) => Response::error("engine_process_unavailable", err),
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

    fn adapter_summaries(&self) -> Vec<Value> {
        self.adapters
            .iter()
            .enumerate()
            .map(|(index, adapter)| {
                json!({
                    "id": adapter.id,
                    "engine": adapter.kind,
                    "default": index == 0,
                    "supported_display_modes": adapter.display_modes
                        .iter()
                        .map(|mode| mode.as_str())
                        .collect::<Vec<_>>(),
                    "supported_guarantee_levels": adapter_guarantee_levels(adapter.kind),
                    "backing_substrate": adapter_backing_substrate(adapter),
                    "network_mode": "runtime_net_only",
                    "direct_network": false,
                    "wallet_injection": false,
                })
            })
            .collect()
    }

    fn supported_guarantee_levels(&self) -> Vec<&'static str> {
        let mut levels = BTreeSet::new();
        for adapter in &self.adapters {
            for level in adapter_guarantee_levels(adapter.kind) {
                levels.insert(level);
            }
        }
        levels.into_iter().collect()
    }
}

fn adapter_guarantee_levels(kind: AdapterKind) -> Vec<&'static str> {
    match kind {
        AdapterKind::ChromiumMicrovm => vec!["mechanism_microvm"],
        AdapterKind::SelkiesGstreamer
        | AdapterKind::HostedRemoteBrowser
        | AdapterKind::ChromiumHeadless
        | AdapterKind::ContractProof => vec!["operator_rbi"],
        AdapterKind::Cef
        | AdapterKind::Webview2
        | AdapterKind::Geckoview
        | AdapterKind::Wkwebview => vec!["policy_webview"],
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BrowserProfileDescriptor {
    schema: String,
    scope: String,
    storage: String,
    storage_posture: String,
    protected_storage: bool,
    encrypted: bool,
    recoverable: bool,
    recovery: String,
    uri: String,
    public_uri: String,
    profile_key: String,
    disk_path: String,
    reset: String,
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ViewportRequest {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BrowserDisplayMode {
    WebrtcRemoteDisplay,
    NativeSurface,
}

impl BrowserDisplayMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::WebrtcRemoteDisplay => "webrtc_remote_display",
            Self::NativeSurface => "native_surface",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BrowserGuaranteeLevel {
    MechanismMicrovm,
    OperatorRbi,
    PolicyWebview,
}

impl BrowserGuaranteeLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::MechanismMicrovm => "mechanism_microvm",
            Self::OperatorRbi => "operator_rbi",
            Self::PolicyWebview => "policy_webview",
        }
    }
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
    process: Option<Value>,
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
