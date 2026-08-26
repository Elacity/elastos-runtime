use crate::config::{AdapterConfig, ConfiguredOffer};
use crate::contract::{
    ErrorClass, RunError, RunStatus, RuntimeCreateBinding, RUN_OUTPUT_CONTENT_SCHEMA,
    RUN_OUTPUT_OBJECT_SCHEMA, RUN_OUTPUT_TEXT_SCHEMA,
};
use crate::journal::{deterministic_run_id, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use url::Url;

const MAX_BACKEND_BODY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_BACKEND_JOB_ID_BYTES: usize = 512;
const MAX_BACKEND_LOG_DETAIL_BYTES: usize = 512;
const MAX_HTTP_JOB_PROGRESS_PHASE_BYTES: usize = 256;
const MAX_HTTP_JOB_PROGRESS_COUNT: u64 = u32::MAX as u64;
const HTTP_JOB_BACKEND_STATE_SCHEMA: &str = "elastos.model.provider-http-job-state/v1";
const HTTP_JOB_BACKEND_STATE_CREATING: &str = "creating";
const HTTP_JOB_BACKEND_STATE_ACTIVE: &str = "active";
pub(crate) const LOCAL_TEXT_BACKEND_STATE_SCHEMA: &str =
    "elastos.model.provider-local-text-state/v1";
const BACKEND_CONNECT_TIMEOUT_MS: u64 = 500;
const BACKEND_READ_IDLE_TIMEOUT_MS: u64 = 500;
const LOCAL_TEXT_DELTA_FLUSH_BYTES: usize = 8 * 1024;
const MAX_LOCAL_TEXT_SSE_LINE_BYTES: usize = 64 * 1024;
const MAX_LOCAL_TEXT_SSE_EVENT_BYTES: usize = 128 * 1024;

fn backend_client(timeout_ms: u64) -> std::result::Result<reqwest::Client, AdapterFault> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(BACKEND_CONNECT_TIMEOUT_MS))
        .read_timeout(Duration::from_millis(BACKEND_READ_IDLE_TIMEOUT_MS))
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|err| {
            AdapterFault::transport(
                "model backend transport was interrupted",
                format!("failed to build backend client: {err}"),
            )
        })
}

#[derive(Debug, Clone)]
pub struct AdapterFault {
    pub error: RunError,
    pub detail: Option<String>,
}

impl AdapterFault {
    pub fn context(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error: RunError {
                class: ErrorClass::ContextRejected,
                code: "context_rejected".to_string(),
                message: message.to_string(),
            },
            detail: Some(bound_detail(detail.into())),
        }
    }

    pub fn transport(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error: RunError {
                class: ErrorClass::TransportInterrupted,
                code: "transport_interrupted".to_string(),
                message: message.to_string(),
            },
            detail: Some(bound_detail(detail.into())),
        }
    }

    pub fn timeout(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error: RunError {
                class: ErrorClass::BackendTimeout,
                code: "backend_timeout".to_string(),
                message: message.to_string(),
            },
            detail: Some(bound_detail(detail.into())),
        }
    }

    pub fn malformed(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error: RunError {
                class: ErrorClass::ResponseMalformed,
                code: "response_malformed".to_string(),
                message: message.to_string(),
            },
            detail: Some(bound_detail(detail.into())),
        }
    }

    pub fn backend_failed(message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error: RunError {
                class: ErrorClass::BackendFailed,
                code: "backend_failed".to_string(),
                message: message.to_string(),
            },
            detail: Some(bound_detail(detail.into())),
        }
    }

    pub fn log(&self) {
        if let Some(detail) = &self.detail {
            eprintln!("[model-provider] adapter {}: {}", self.error.code, detail);
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventSeed {
    pub kind: &'static str,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub enum DispatchResult {
    #[allow(dead_code)]
    Terminal {
        events: Vec<EventSeed>,
        status: RunStatus,
        output: Option<Value>,
        error: Option<RunError>,
    },
    Running {
        events: Vec<EventSeed>,
        backend_state: Value,
    },
}

#[derive(Debug, Clone)]
pub enum ReconcileResult {
    StillRunning {
        events: Vec<EventSeed>,
        backend_state: Value,
        status: RunStatus,
    },
    Terminal {
        events: Vec<EventSeed>,
        status: RunStatus,
        output: Option<Value>,
        error: Option<RunError>,
    },
}

#[derive(Debug, Clone)]
pub enum CancelResult {
    Reconciling {
        events: Vec<EventSeed>,
        backend_state: Value,
    },
    #[allow(dead_code)]
    Terminal {
        events: Vec<EventSeed>,
        status: RunStatus,
        output: Option<Value>,
        error: Option<RunError>,
    },
    SettlementUnknown {
        events: Vec<EventSeed>,
    },
}

#[derive(Debug, Clone)]
pub struct CancelReservation {
    pub backend_state: Value,
    pub allow_send: bool,
}

#[derive(Debug)]
pub(crate) enum WorkerUpdate {
    Apply {
        run_id: String,
        generation: u64,
        guard: WorkerApplyGuard,
        result: ReconcileResult,
        acknowledge: oneshot::Sender<WorkerApplyAck>,
    },
    Exited {
        run_id: String,
        generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerApplyAck {
    Applied,
    Rejected,
}

#[derive(Debug, Clone)]
pub(crate) enum WorkerApplyGuard {
    None,
    HttpArtifactBackendState { backend_state: Value },
}

enum WorkerControl {
    LocalText { cancel_tx: watch::Sender<bool> },
    HttpArtifactCreate,
    HttpArtifactStatus { job_id: String },
    HttpArtifactCancel { job_id: String },
}

pub(crate) struct WorkerRecord {
    generation: u64,
    control: WorkerControl,
    join_handle: Option<JoinHandle<()>>,
    retired_handles: Vec<JoinHandle<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalTextBackendState {
    schema: String,
    cancel_requested: bool,
}

struct LocalTextStreamState {
    output_text: String,
    delta_buffer: String,
    consumed_response_bytes: u64,
}

struct LocalTextWorkerTask {
    run_id: String,
    generation: u64,
    api_url: String,
    api_key: Option<String>,
    model: String,
    offer: ConfiguredOffer,
    prompt: String,
    cancel_rx: watch::Receiver<bool>,
    updates: mpsc::Sender<WorkerUpdate>,
}

struct HttpArtifactCreateWorkerTask {
    run_id: String,
    generation: u64,
    create_url: String,
    bearer_token: Option<String>,
    poll_interval_ms: u64,
    offer: ConfiguredOffer,
    binding: RuntimeCreateBinding,
    input: Value,
    updates: mpsc::Sender<WorkerUpdate>,
}

struct HttpArtifactStatusWorkerTask {
    run_id: String,
    generation: u64,
    status_url: String,
    bearer_token: Option<String>,
    poll_interval_ms: u64,
    offer: ConfiguredOffer,
    binding: RuntimeCreateBinding,
    backend_state: Value,
    updates: mpsc::Sender<WorkerUpdate>,
}

struct HttpArtifactCancelWorkerTask {
    run_id: String,
    generation: u64,
    cancel_url: String,
    bearer_token: Option<String>,
    offer: ConfiguredOffer,
    binding: RuntimeCreateBinding,
    backend_state: Value,
    updates: mpsc::Sender<WorkerUpdate>,
}

impl LocalTextStreamState {
    fn new() -> Self {
        Self {
            output_text: String::new(),
            delta_buffer: String::new(),
            consumed_response_bytes: 0,
        }
    }
}

pub trait AdapterExecutor {
    fn dispatch(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        input: &Value,
    ) -> std::result::Result<DispatchResult, AdapterFault>;

    fn reconcile(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<ReconcileResult, AdapterFault>;

    fn cancel(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
        allow_send: bool,
    ) -> std::result::Result<CancelResult, AdapterFault>;

    fn reserve_cancel(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<CancelReservation, AdapterFault>;
}

#[derive(Clone)]
pub struct LiveAdapterExecutor {
    runtime: Handle,
    updates: mpsc::Sender<WorkerUpdate>,
    workers: Arc<Mutex<BTreeMap<String, WorkerRecord>>>,
    next_generation: Arc<AtomicU64>,
}

impl LiveAdapterExecutor {
    pub fn new(runtime: Handle, updates: mpsc::Sender<WorkerUpdate>) -> Self {
        Self {
            runtime,
            updates,
            workers: Arc::new(Mutex::new(BTreeMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn active_text_run_ids(&self) -> Vec<String> {
        self.workers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, record)| matches!(record.control, WorkerControl::LocalText { .. }))
            .map(|(run_id, _)| run_id.clone())
            .collect()
    }

    pub(crate) fn active_http_create_run_ids(&self) -> Vec<String> {
        self.workers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, record)| matches!(record.control, WorkerControl::HttpArtifactCreate))
            .map(|(run_id, _)| run_id.clone())
            .collect()
    }

    pub(crate) fn is_current_worker_generation(&self, run_id: &str, generation: u64) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(run_id)
            .map(|record| record.generation == generation)
            .unwrap_or(false)
    }

    pub(crate) fn has_local_text_worker(&self, run_id: &str) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(run_id)
            .map(|record| matches!(record.control, WorkerControl::LocalText { .. }))
            .unwrap_or(false)
    }

    pub(crate) fn has_http_artifact_create_worker(&self, run_id: &str) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(run_id)
            .map(|record| matches!(record.control, WorkerControl::HttpArtifactCreate))
            .unwrap_or(false)
    }

    fn has_http_artifact_worker(&self, run_id: &str, job_id: &str) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(run_id)
            .map(|record| match &record.control {
                WorkerControl::HttpArtifactCreate => true,
                WorkerControl::HttpArtifactStatus {
                    job_id: current_job_id,
                } => current_job_id == job_id,
                WorkerControl::HttpArtifactCancel {
                    job_id: current_job_id,
                } => current_job_id == job_id,
                WorkerControl::LocalText { .. } => false,
            })
            .unwrap_or(false)
    }

    pub(crate) fn remove_worker_if_current(
        &self,
        run_id: &str,
        generation: u64,
    ) -> Option<WorkerRecord> {
        let mut workers = self.workers.lock().unwrap();
        if workers
            .get(run_id)
            .map(|record| record.generation == generation)
            .unwrap_or(false)
        {
            return workers.remove(run_id);
        }
        None
    }

    pub(crate) async fn await_worker_record(&self, record: WorkerRecord) {
        let mut join_handles = record.retired_handles;
        if let Some(join_handle) = record.join_handle {
            join_handles.push(join_handle);
        }
        for join_handle in join_handles {
            let _ = join_handle.await;
        }
    }

    pub(crate) async fn shutdown_workers(&self) {
        let workers = {
            let mut guard = self.workers.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        let mut join_handles = Vec::new();
        for (_, record) in workers {
            join_handles.extend(record.retired_handles);
            let Some(join_handle) = record.join_handle else {
                continue;
            };
            join_handle.abort();
            join_handles.push(join_handle);
        }
        for join_handle in join_handles {
            let _ = join_handle.await;
        }
    }

    fn spawn_local_text_worker(
        &self,
        api_url: &str,
        api_key: Option<&str>,
        model: &str,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        prompt: &str,
    ) -> std::result::Result<Value, AdapterFault> {
        let run_id = deterministic_run_id(binding);
        let backend_state = serialize_local_text_backend_state(false)?;
        let mut workers = self.workers.lock().unwrap();
        if workers.contains_key(&run_id) {
            return Err(AdapterFault::context(
                "model run could not start",
                "local text worker already exists for run_id",
            ));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        workers.insert(
            run_id.clone(),
            WorkerRecord {
                generation,
                control: WorkerControl::LocalText {
                    cancel_tx: cancel_tx.clone(),
                },
                join_handle: None,
                retired_handles: Vec::new(),
            },
        );
        let updates = self.updates.clone();
        let offer = offer.clone();
        let api_url = api_url.to_string();
        let api_key = api_key.map(str::to_string);
        let model = model.to_string();
        let prompt = prompt.to_string();
        let run_id_for_task = run_id.clone();
        let join_handle = self.runtime.spawn(async move {
            run_local_text_worker(LocalTextWorkerTask {
                run_id: run_id_for_task.clone(),
                generation,
                api_url,
                api_key,
                model,
                offer,
                prompt,
                cancel_rx,
                updates: updates.clone(),
            })
            .await;
            let _ = updates
                .send(WorkerUpdate::Exited {
                    run_id: run_id_for_task,
                    generation,
                })
                .await;
        });
        workers
            .get_mut(&run_id)
            .expect("local text worker placeholder must exist")
            .join_handle = Some(join_handle);
        Ok(backend_state)
    }

    fn spawn_http_artifact_create_worker(
        &self,
        create_url: &str,
        bearer_token: Option<&str>,
        poll_interval_ms: u64,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        input: &Value,
    ) -> std::result::Result<Value, AdapterFault> {
        let run_id = deterministic_run_id(binding);
        let backend_state = serialize_http_job_creating_backend_state()?;
        let mut workers = self.workers.lock().unwrap();
        if workers.contains_key(&run_id) {
            return Err(AdapterFault::context(
                "model run could not start",
                "http artifact create worker already exists for run_id",
            ));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        workers.insert(
            run_id.clone(),
            WorkerRecord {
                generation,
                control: WorkerControl::HttpArtifactCreate,
                join_handle: None,
                retired_handles: Vec::new(),
            },
        );
        let updates = self.updates.clone();
        let offer = offer.clone();
        let binding = binding.clone();
        let input = input.clone();
        let create_url = create_url.to_string();
        let bearer_token = bearer_token.map(str::to_string);
        let run_id_for_task = run_id.clone();
        let join_handle = self.runtime.spawn(async move {
            run_http_artifact_create_worker(HttpArtifactCreateWorkerTask {
                run_id: run_id_for_task.clone(),
                generation,
                create_url,
                bearer_token,
                poll_interval_ms,
                offer,
                binding,
                input,
                updates: updates.clone(),
            })
            .await;
            let _ = updates
                .send(WorkerUpdate::Exited {
                    run_id: run_id_for_task,
                    generation,
                })
                .await;
        });
        workers
            .get_mut(&run_id)
            .expect("http artifact create worker placeholder must exist")
            .join_handle = Some(join_handle);
        Ok(backend_state)
    }

    fn spawn_http_artifact_status_worker(
        &self,
        status_url: &str,
        bearer_token: Option<&str>,
        poll_interval_ms: u64,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<Value, AdapterFault> {
        let run_id = deterministic_run_id(binding);
        let mut state = parse_http_job_backend_state(backend_state)?;
        let mut workers = self.workers.lock().unwrap();
        if workers.contains_key(&run_id) {
            return serialize_http_job_backend_state(&state);
        }
        state.next_poll_at_ms = bounded_http_job_next_poll_at_ms(&state, poll_interval_ms)?;
        let next_backend_state = serialize_http_job_backend_state(&state)?;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        workers.insert(
            run_id.clone(),
            WorkerRecord {
                generation,
                control: WorkerControl::HttpArtifactStatus {
                    job_id: state.job_id.clone(),
                },
                join_handle: None,
                retired_handles: Vec::new(),
            },
        );
        let updates = self.updates.clone();
        let offer = offer.clone();
        let binding = binding.clone();
        let status_url = status_url.to_string();
        let bearer_token = bearer_token.map(str::to_string);
        let run_id_for_task = run_id.clone();
        let backend_state_for_task = next_backend_state.clone();
        let join_handle = self.runtime.spawn(async move {
            run_http_artifact_status_worker(HttpArtifactStatusWorkerTask {
                run_id: run_id_for_task.clone(),
                generation,
                status_url,
                bearer_token,
                poll_interval_ms,
                offer,
                binding,
                backend_state: backend_state_for_task,
                updates: updates.clone(),
            })
            .await;
            let _ = updates
                .send(WorkerUpdate::Exited {
                    run_id: run_id_for_task,
                    generation,
                })
                .await;
        });
        workers
            .get_mut(&run_id)
            .expect("http artifact status worker placeholder must exist")
            .join_handle = Some(join_handle);
        Ok(next_backend_state)
    }

    fn spawn_http_artifact_cancel_worker(
        &self,
        cancel_url: &str,
        bearer_token: Option<&str>,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<Value, AdapterFault> {
        let run_id = deterministic_run_id(binding);
        let state = parse_http_job_backend_state(backend_state)?;
        let mut workers = self.workers.lock().unwrap();
        let mut retired_handles = Vec::new();
        if let Some(mut previous) = workers.remove(&run_id) {
            match previous.control {
                WorkerControl::HttpArtifactStatus { .. }
                | WorkerControl::HttpArtifactCancel { .. } => {
                    if let Some(join_handle) = previous.join_handle.take() {
                        join_handle.abort();
                        retired_handles.push(join_handle);
                    }
                    retired_handles.extend(previous.retired_handles);
                }
                WorkerControl::HttpArtifactCreate => {
                    return Err(AdapterFault::context(
                        "model run could not continue",
                        "http artifact cancel cannot supersede create worker",
                    ));
                }
                WorkerControl::LocalText { .. } => {
                    return Err(AdapterFault::context(
                        "model run could not continue",
                        "http artifact cancel cannot supersede local text worker",
                    ));
                }
            }
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        workers.insert(
            run_id.clone(),
            WorkerRecord {
                generation,
                control: WorkerControl::HttpArtifactCancel {
                    job_id: state.job_id.clone(),
                },
                join_handle: None,
                retired_handles,
            },
        );
        let updates = self.updates.clone();
        let offer = offer.clone();
        let binding = binding.clone();
        let cancel_url = cancel_url.to_string();
        let bearer_token = bearer_token.map(str::to_string);
        let run_id_for_task = run_id.clone();
        let backend_state_for_task = backend_state.clone();
        let join_handle = self.runtime.spawn(async move {
            run_http_artifact_cancel_worker(HttpArtifactCancelWorkerTask {
                run_id: run_id_for_task.clone(),
                generation,
                cancel_url,
                bearer_token,
                offer,
                binding,
                backend_state: backend_state_for_task,
                updates: updates.clone(),
            })
            .await;
            let _ = updates
                .send(WorkerUpdate::Exited {
                    run_id: run_id_for_task,
                    generation,
                })
                .await;
        });
        workers
            .get_mut(&run_id)
            .expect("http artifact cancel worker placeholder must exist")
            .join_handle = Some(join_handle);
        Ok(backend_state.clone())
    }
}

impl AdapterExecutor for LiveAdapterExecutor {
    fn dispatch(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        input: &Value,
    ) -> std::result::Result<DispatchResult, AdapterFault> {
        match adapter {
            AdapterConfig::OpenAiCompatibleText {
                api_url,
                api_key,
                model,
            } => dispatch_openai_text(
                self,
                api_url,
                api_key.as_deref(),
                model,
                offer,
                binding,
                input,
            ),
            AdapterConfig::HttpJobArtifact {
                create_url,
                status_url: _,
                cancel_url: _,
                bearer_token,
                poll_interval_ms,
            } => {
                let backend_state = self.spawn_http_artifact_create_worker(
                    create_url,
                    bearer_token.as_deref(),
                    *poll_interval_ms,
                    offer,
                    binding,
                    input,
                )?;
                Ok(DispatchResult::Running {
                    events: Vec::new(),
                    backend_state,
                })
            }
        }
    }

    fn reconcile(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<ReconcileResult, AdapterFault> {
        match adapter {
            AdapterConfig::OpenAiCompatibleText { .. } => {
                reconcile_local_text(self, binding, backend_state)
            }
            AdapterConfig::HttpJobArtifact {
                create_url: _,
                status_url,
                cancel_url: _,
                bearer_token,
                poll_interval_ms,
            } => reconcile_http_job(
                self,
                status_url,
                bearer_token.as_deref(),
                *poll_interval_ms,
                offer,
                binding,
                backend_state,
            ),
        }
    }

    fn cancel(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        backend_state: &Value,
        allow_send: bool,
    ) -> std::result::Result<CancelResult, AdapterFault> {
        match adapter {
            AdapterConfig::OpenAiCompatibleText { .. } => {
                cancel_local_text(self, binding, backend_state)
            }
            AdapterConfig::HttpJobArtifact {
                create_url: _,
                status_url: _,
                cancel_url,
                bearer_token,
                poll_interval_ms: _,
            } => cancel_http_job(
                self,
                cancel_url.as_deref(),
                bearer_token.as_deref(),
                offer,
                binding,
                backend_state,
                allow_send,
            ),
        }
    }

    fn reserve_cancel(
        &self,
        adapter: &AdapterConfig,
        offer: &ConfiguredOffer,
        _binding: &RuntimeCreateBinding,
        backend_state: &Value,
    ) -> std::result::Result<CancelReservation, AdapterFault> {
        match adapter {
            AdapterConfig::OpenAiCompatibleText { .. } => reserve_local_text_cancel(backend_state),
            AdapterConfig::HttpJobArtifact { cancel_url, .. } => {
                reserve_http_job_cancel(backend_state, cancel_url.is_some(), offer)
            }
        }
    }
}

fn dispatch_openai_text(
    executor: &LiveAdapterExecutor,
    api_url: &str,
    api_key: Option<&str>,
    model: &str,
    offer: &ConfiguredOffer,
    binding: &RuntimeCreateBinding,
    input: &Value,
) -> std::result::Result<DispatchResult, AdapterFault> {
    let prompt = validate_text_prompt(input)?;
    let backend_state =
        executor.spawn_local_text_worker(api_url, api_key, model, offer, binding, prompt)?;
    Ok(DispatchResult::Running {
        events: vec![EventSeed {
            kind: "dispatched",
            data: json!({
                "offer_id": offer.id
            }),
        }],
        backend_state,
    })
}

fn validate_text_prompt(input: &Value) -> std::result::Result<&str, AdapterFault> {
    let schema = input.get("schema").and_then(Value::as_str);
    if schema != Some("elastos.model.input.text/v1") {
        return Err(AdapterFault::context(
            "model input is invalid",
            "text offer input schema must be elastos.model.input.text/v1",
        ));
    }
    input.get("prompt").and_then(Value::as_str).ok_or_else(|| {
        AdapterFault::context("model input is invalid", "text offer input missing prompt")
    })
}

pub(crate) fn is_local_text_backend_state(value: &Value) -> bool {
    value
        .get("schema")
        .and_then(Value::as_str)
        .map(|schema| schema == LOCAL_TEXT_BACKEND_STATE_SCHEMA)
        .unwrap_or(false)
}

pub(crate) fn serialize_local_text_backend_state(
    cancel_requested: bool,
) -> std::result::Result<Value, AdapterFault> {
    serde_json::to_value(LocalTextBackendState {
        schema: LOCAL_TEXT_BACKEND_STATE_SCHEMA.to_string(),
        cancel_requested,
    })
    .map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("failed to encode local text backend state: {err}"),
        )
    })
}

fn parse_local_text_backend_state(
    backend_state: &Value,
) -> std::result::Result<LocalTextBackendState, AdapterFault> {
    let state =
        serde_json::from_value::<LocalTextBackendState>(backend_state.clone()).map_err(|_| {
            AdapterFault::malformed(
                "model backend returned invalid data",
                "local text backend state is invalid",
            )
        })?;
    if state.schema != LOCAL_TEXT_BACKEND_STATE_SCHEMA {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "local text backend state schema is invalid",
        ));
    }
    Ok(state)
}

fn reconcile_local_text(
    executor: &LiveAdapterExecutor,
    binding: &RuntimeCreateBinding,
    backend_state: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let state = parse_local_text_backend_state(backend_state)?;
    let run_id = deterministic_run_id(binding);
    if executor.has_local_text_worker(run_id.as_str()) {
        return Ok(ReconcileResult::StillRunning {
            events: Vec::new(),
            backend_state: serialize_local_text_backend_state(state.cancel_requested)?,
            status: if state.cancel_requested {
                RunStatus::Reconciling
            } else {
                RunStatus::Running
            },
        });
    }
    Ok(ReconcileResult::Terminal {
        events: Vec::new(),
        status: RunStatus::SettlementUnknown,
        output: None,
        error: Some(RunError {
            class: ErrorClass::SettlementUnknown,
            code: "settlement_unknown".to_string(),
            message: "model backend settlement is unknown".to_string(),
        }),
    })
}

fn reserve_local_text_cancel(
    backend_state: &Value,
) -> std::result::Result<CancelReservation, AdapterFault> {
    let _state = parse_local_text_backend_state(backend_state)?;
    Ok(CancelReservation {
        backend_state: serialize_local_text_backend_state(true)?,
        allow_send: false,
    })
}

fn cancel_local_text(
    executor: &LiveAdapterExecutor,
    binding: &RuntimeCreateBinding,
    backend_state: &Value,
) -> std::result::Result<CancelResult, AdapterFault> {
    let _state = parse_local_text_backend_state(backend_state)?;
    let run_id = deterministic_run_id(binding);
    let mut workers = executor.workers.lock().unwrap();
    let Some(record) = workers.get_mut(run_id.as_str()) else {
        return Ok(CancelResult::SettlementUnknown { events: Vec::new() });
    };
    let WorkerControl::LocalText { cancel_tx } = &record.control else {
        return Ok(CancelResult::SettlementUnknown { events: Vec::new() });
    };
    let _ = cancel_tx.send(true);
    Ok(CancelResult::Reconciling {
        events: Vec::new(),
        backend_state: serialize_local_text_backend_state(true)?,
    })
}

async fn run_local_text_worker(mut task: LocalTextWorkerTask) {
    let result = match run_local_text_worker_inner(&mut task).await {
        Ok(result) => result,
        Err(fault) => ReconcileResult::Terminal {
            events: Vec::new(),
            status: match fault.error.class {
                ErrorClass::Cancelled => RunStatus::Cancelled,
                ErrorClass::SettlementUnknown => RunStatus::SettlementUnknown,
                _ => RunStatus::Failed,
            },
            output: None,
            error: Some(fault.error),
        },
    };
    let _ = send_worker_apply_update(
        &task.run_id,
        task.generation,
        WorkerApplyGuard::None,
        result,
        &task.updates,
    )
    .await;
}

async fn run_http_artifact_create_worker(task: HttpArtifactCreateWorkerTask) {
    let result = match run_http_artifact_create_worker_inner(
        &task.create_url,
        task.bearer_token.as_deref(),
        task.poll_interval_ms,
        &task.offer,
        &task.binding,
        &task.input,
    )
    .await
    {
        Ok(result) => result,
        Err(fault) => {
            fault.log();
            worker_settlement_unknown_result()
        }
    };
    let _ = send_worker_apply_update(
        &task.run_id,
        task.generation,
        WorkerApplyGuard::None,
        result,
        &task.updates,
    )
    .await;
}

async fn run_http_artifact_status_worker(task: HttpArtifactStatusWorkerTask) {
    let result = match run_http_artifact_status_worker_inner(
        &task.status_url,
        task.bearer_token.as_deref(),
        task.poll_interval_ms,
        &task.offer,
        &task.binding,
        &task.backend_state,
    )
    .await
    {
        Ok(result) => result,
        Err(fault) => {
            fault.log();
            preserve_http_job_active_result(&task.backend_state, task.poll_interval_ms)
                .unwrap_or_else(|_| worker_settlement_unknown_result())
        }
    };
    let _ = send_worker_apply_update(
        &task.run_id,
        task.generation,
        WorkerApplyGuard::HttpArtifactBackendState {
            backend_state: task.backend_state.clone(),
        },
        result,
        &task.updates,
    )
    .await;
}

async fn run_http_artifact_cancel_worker(task: HttpArtifactCancelWorkerTask) {
    let result = match run_http_artifact_cancel_worker_inner(
        &task.cancel_url,
        task.bearer_token.as_deref(),
        &task.offer,
        &task.binding,
        &task.backend_state,
    )
    .await
    {
        Ok(result) => result,
        Err(fault) => {
            fault.log();
            preserve_http_job_active_after_cancel_attempt_result(&task.backend_state)
                .unwrap_or_else(|_| worker_settlement_unknown_result())
        }
    };
    let _ = send_worker_apply_update(
        &task.run_id,
        task.generation,
        WorkerApplyGuard::HttpArtifactBackendState {
            backend_state: task.backend_state.clone(),
        },
        result,
        &task.updates,
    )
    .await;
}

async fn run_http_artifact_create_worker_inner(
    create_url: &str,
    bearer_token: Option<&str>,
    poll_interval_ms: u64,
    offer: &ConfiguredOffer,
    binding: &RuntimeCreateBinding,
    input: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let client = backend_client(offer.policy.runtime_ms_limit)?;
    let request = {
        let mut builder = client
            .post(create_url)
            .header("content-type", "application/json")
            .json(&json!({
                "request_id": binding.request_id,
                "offer_id": offer.id,
                "operation": offer.operation,
                "input": input,
            }));
        if let Some(bearer_token) = bearer_token {
            builder = builder.header("authorization", format!("Bearer {bearer_token}"));
        }
        builder
    };
    let response = request.send().await.map_err(map_reqwest_failure)?;
    if !response.status().is_success() {
        return Err(AdapterFault::backend_failed(
            "model backend failed",
            format!("unexpected backend status {}", response.status().as_u16()),
        ));
    }
    let value = read_bounded_json_response_async(response).await?;
    let job_id = value.get("job_id").and_then(Value::as_str).ok_or_else(|| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend create missing job_id",
        )
    })?;
    validate_job_id(job_id)?;
    let now = now_ms();
    Ok(ReconcileResult::StillRunning {
        events: vec![EventSeed {
            kind: "dispatched",
            data: json!({
                "offer_id": offer.id
            }),
        }],
        backend_state: serialize_http_job_backend_state(&HttpJobBackendState {
            schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
            phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
            job_id: job_id.to_string(),
            next_poll_at_ms: next_poll_at_ms(now, poll_interval_ms),
            cancel_requested: false,
            cancel_sent: false,
            cancel_deadline_ms: None,
        })?,
        status: RunStatus::Running,
    })
}

async fn run_http_artifact_status_worker_inner(
    status_url: &str,
    bearer_token: Option<&str>,
    poll_interval_ms: u64,
    offer: &ConfiguredOffer,
    _binding: &RuntimeCreateBinding,
    backend_state: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let state = parse_http_job_backend_state(backend_state)?;
    let now = now_ms();
    if state.cancel_requested
        && state
            .cancel_deadline_ms
            .map(|deadline| now >= deadline)
            .unwrap_or(false)
    {
        return Ok(worker_settlement_unknown_result());
    }
    let url = status_request_url(status_url, &state.job_id)?;
    let client = backend_client(offer.policy.runtime_ms_limit)?;
    let mut request = client.get(url);
    if let Some(bearer_token) = bearer_token {
        request = request.header("authorization", format!("Bearer {bearer_token}"));
    }
    let response = request.send().await.map_err(map_reqwest_failure)?;
    if !response.status().is_success() {
        return Err(AdapterFault::backend_failed(
            "model backend failed",
            format!("unexpected backend status {}", response.status().as_u16()),
        ));
    }
    let value = read_bounded_json_response_async(response).await?;
    parse_http_job_status_result(value, offer, state, poll_interval_ms)
}

async fn run_local_text_worker_inner(
    task: &mut LocalTextWorkerTask,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let client = backend_client(task.offer.policy.runtime_ms_limit)?;
    let request = {
        let mut builder = client
            .post(&task.api_url)
            .header("content-type", "application/json")
            .json(&json!({
                "model": task.model,
                "stream": true,
                "messages": [
                    { "role": "user", "content": task.prompt }
                ]
            }));
        if let Some(api_key) = task.api_key.as_deref() {
            builder = builder.header("authorization", format!("Bearer {api_key}"));
        }
        builder
    };
    let response = tokio::select! {
        changed = task.cancel_rx.changed() => {
            return handle_local_text_cancel_signal(&task.cancel_rx, changed);
        }
        response = request.send() => response.map_err(map_reqwest_failure)?
    };
    if !response.status().is_success() {
        return Err(AdapterFault::backend_failed(
            "model backend failed",
            format!("unexpected backend status {}", response.status().as_u16()),
        ));
    }

    let mut response = response;
    let mut line = Vec::new();
    let mut event_data = Vec::new();
    let mut stream_state = LocalTextStreamState::new();
    let mut done = false;

    while !done {
        let next = tokio::select! {
            changed = task.cancel_rx.changed() => {
                return handle_local_text_cancel_signal(&task.cancel_rx, changed);
            }
            chunk = response.chunk() => chunk.map_err(map_reqwest_failure)?
        };
        let Some(chunk) = next else {
            return Err(AdapterFault::malformed(
                "model backend returned invalid data",
                "text stream ended before terminal marker",
            ));
        };
        consume_local_text_stream_bytes(&mut stream_state, chunk.len())?;
        for byte in chunk {
            if line.len() >= MAX_LOCAL_TEXT_SSE_LINE_BYTES {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "text stream line exceeds provider limits",
                ));
            }
            line.push(byte);
            if byte != b'\n' {
                continue;
            }
            let mut current_line = std::mem::take(&mut line);
            if current_line.ends_with(b"\n") {
                current_line.pop();
            }
            if current_line.ends_with(b"\r") {
                current_line.pop();
            }
            if current_line.is_empty() {
                if event_data.is_empty() {
                    continue;
                }
                let payload = String::from_utf8(std::mem::take(&mut event_data)).map_err(|_| {
                    AdapterFault::malformed(
                        "model backend returned invalid data",
                        "text stream event must be valid utf-8",
                    )
                })?;
                if payload.trim() == "[DONE]" {
                    done = true;
                    break;
                }
                let delta = extract_stream_text_delta(&payload)?;
                if !delta.is_empty() {
                    append_local_text_delta(&mut stream_state, &task.offer, &delta)?;
                    if stream_state.delta_buffer.len() >= LOCAL_TEXT_DELTA_FLUSH_BYTES {
                        flush_local_text_delta(
                            &task.offer,
                            &task.run_id,
                            task.generation,
                            &task.updates,
                            &mut stream_state.delta_buffer,
                        )
                        .await?;
                    }
                }
                continue;
            }
            if let Some(data) = current_line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                let next_len = event_data
                    .len()
                    .saturating_add(data.len())
                    .saturating_add(1);
                if next_len > MAX_LOCAL_TEXT_SSE_EVENT_BYTES {
                    return Err(AdapterFault::malformed(
                        "model backend returned invalid data",
                        "text stream event exceeds provider limits",
                    ));
                }
                if !event_data.is_empty() {
                    event_data.push(b'\n');
                }
                event_data.extend_from_slice(data);
            }
        }
    }

    flush_local_text_delta(
        &task.offer,
        &task.run_id,
        task.generation,
        &task.updates,
        &mut stream_state.delta_buffer,
    )
    .await?;
    let output = json!({
        "schema": RUN_OUTPUT_TEXT_SCHEMA,
        "text": stream_state.output_text,
    });
    sanitize_output(&output, &task.offer)?;
    Ok(ReconcileResult::Terminal {
        events: Vec::new(),
        status: RunStatus::Completed,
        output: Some(output),
        error: None,
    })
}

fn handle_local_text_cancel_signal(
    cancel_rx: &watch::Receiver<bool>,
    changed: std::result::Result<(), tokio::sync::watch::error::RecvError>,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    match changed {
        Ok(()) => {
            if *cancel_rx.borrow() {
                Ok(local_text_cancelled_result())
            } else {
                Ok(worker_settlement_unknown_result())
            }
        }
        Err(_) => {
            if *cancel_rx.borrow() {
                Ok(local_text_cancelled_result())
            } else {
                Ok(worker_settlement_unknown_result())
            }
        }
    }
}

async fn flush_local_text_delta(
    offer: &ConfiguredOffer,
    run_id: &str,
    generation: u64,
    updates: &mpsc::Sender<WorkerUpdate>,
    delta_buffer: &mut String,
) -> std::result::Result<(), AdapterFault> {
    if delta_buffer.is_empty() {
        return Ok(());
    }
    let delta = std::mem::take(delta_buffer);
    let delta_event = json!({ "text": delta });
    let encoded = serde_json::to_vec(&delta_event).map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("failed to encode text stream event: {err}"),
        )
    })?;
    if encoded.len() as u64 > offer.policy.event_bytes_limit {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "text stream event exceeds configured limits",
        ));
    }
    send_worker_apply_update(
        run_id,
        generation,
        WorkerApplyGuard::None,
        ReconcileResult::StillRunning {
            events: vec![EventSeed {
                kind: "text_delta",
                data: delta_event,
            }],
            backend_state: serialize_local_text_backend_state(false)?,
            status: RunStatus::Running,
        },
        updates,
    )
    .await
}

fn append_local_text_delta(
    state: &mut LocalTextStreamState,
    offer: &ConfiguredOffer,
    delta: &str,
) -> std::result::Result<(), AdapterFault> {
    let mut next_output = state.output_text.clone();
    next_output.push_str(delta);
    let output = json!({
        "schema": RUN_OUTPUT_TEXT_SCHEMA,
        "text": next_output,
    });
    sanitize_output(&output, offer)?;
    state.output_text = next_output;
    state.delta_buffer.push_str(delta);
    Ok(())
}

fn consume_local_text_stream_bytes(
    state: &mut LocalTextStreamState,
    bytes: usize,
) -> std::result::Result<(), AdapterFault> {
    let next_total = state.consumed_response_bytes.saturating_add(bytes as u64);
    if next_total > MAX_BACKEND_BODY_BYTES {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "text stream body exceeds provider limits",
        ));
    }
    state.consumed_response_bytes = next_total;
    Ok(())
}

fn extract_stream_text_delta(payload: &str) -> std::result::Result<String, AdapterFault> {
    let value = serde_json::from_str::<Value>(payload).map_err(|_| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "text stream event must be valid json",
        )
    })?;
    if let Some(text) = value
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
    {
        return Ok(text.to_string());
    }
    if let Some(text) = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
    {
        return Ok(text.to_string());
    }
    Ok(String::new())
}

fn local_text_cancelled_result() -> ReconcileResult {
    ReconcileResult::Terminal {
        events: Vec::new(),
        status: RunStatus::Cancelled,
        output: None,
        error: Some(RunError {
            class: ErrorClass::Cancelled,
            code: "cancelled".to_string(),
            message: "model run was cancelled".to_string(),
        }),
    }
}

fn worker_settlement_unknown_result() -> ReconcileResult {
    ReconcileResult::Terminal {
        events: Vec::new(),
        status: RunStatus::SettlementUnknown,
        output: None,
        error: Some(RunError {
            class: ErrorClass::SettlementUnknown,
            code: "settlement_unknown".to_string(),
            message: "model backend settlement is unknown".to_string(),
        }),
    }
}

async fn send_worker_apply_update(
    run_id: &str,
    generation: u64,
    guard: WorkerApplyGuard,
    result: ReconcileResult,
    updates: &mpsc::Sender<WorkerUpdate>,
) -> std::result::Result<(), AdapterFault> {
    let (acknowledge, ack_rx) = oneshot::channel();
    updates
        .send(WorkerUpdate::Apply {
            run_id: run_id.to_string(),
            generation,
            guard,
            result,
            acknowledge,
        })
        .await
        .map_err(|_| worker_control_lost_fault("worker update channel was dropped"))?;
    match ack_rx.await {
        Ok(WorkerApplyAck::Applied) => Ok(()),
        Ok(WorkerApplyAck::Rejected) => Err(worker_update_rejected_fault()),
        Err(_) => Err(worker_control_lost_fault(
            "worker acknowledgement channel was dropped",
        )),
    }
}

fn worker_update_rejected_fault() -> AdapterFault {
    AdapterFault::context(
        "model run could not continue",
        "worker progress update was rejected",
    )
}

fn worker_control_lost_fault(detail: &'static str) -> AdapterFault {
    AdapterFault {
        error: RunError {
            class: ErrorClass::SettlementUnknown,
            code: "settlement_unknown".to_string(),
            message: "model backend settlement is unknown".to_string(),
        },
        detail: Some(detail.to_string()),
    }
}

fn map_reqwest_failure(err: reqwest::Error) -> AdapterFault {
    if err.is_timeout() {
        return AdapterFault::timeout(
            "model backend timed out",
            format!("backend request timed out: {err}"),
        );
    }
    AdapterFault::transport(
        "model backend transport was interrupted",
        format!("backend request failed: {err}"),
    )
}

async fn read_bounded_json_response_async(
    mut response: reqwest::Response,
) -> std::result::Result<Value, AdapterFault> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_reqwest_failure)? {
        let next_len = bytes.len().saturating_add(chunk.len());
        if next_len > MAX_BACKEND_BODY_BYTES as usize {
            return Err(AdapterFault::malformed(
                "model backend returned invalid data",
                "model backend response exceeds provider limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("invalid json response: {err}"),
        )
    })
}

fn reconcile_http_job(
    executor: &LiveAdapterExecutor,
    status_url: &str,
    bearer_token: Option<&str>,
    poll_interval_ms: u64,
    offer: &ConfiguredOffer,
    binding: &RuntimeCreateBinding,
    backend_state: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    if is_http_job_creating_backend_state(backend_state) {
        let run_id = deterministic_run_id(binding);
        if executor.has_http_artifact_create_worker(run_id.as_str()) {
            return Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: serialize_http_job_creating_backend_state()?,
                status: RunStatus::Running,
            });
        }
        return Ok(ReconcileResult::Terminal {
            events: Vec::new(),
            status: RunStatus::SettlementUnknown,
            output: None,
            error: Some(RunError {
                class: ErrorClass::SettlementUnknown,
                code: "settlement_unknown".to_string(),
                message: "model backend settlement is unknown".to_string(),
            }),
        });
    }
    let state = parse_http_job_backend_state(backend_state)?;
    let now = now_ms();
    if state.cancel_requested
        && state
            .cancel_deadline_ms
            .map(|deadline| now >= deadline)
            .unwrap_or(false)
    {
        return Ok(ReconcileResult::Terminal {
            events: Vec::new(),
            status: RunStatus::SettlementUnknown,
            output: None,
            error: Some(RunError {
                class: ErrorClass::SettlementUnknown,
                code: "settlement_unknown".to_string(),
                message: "model backend settlement is unknown".to_string(),
            }),
        });
    }
    let run_id = deterministic_run_id(binding);
    if executor.has_http_artifact_worker(run_id.as_str(), &state.job_id) {
        return http_job_still_running_result(state);
    }
    if now < state.next_poll_at_ms {
        return http_job_still_running_result(state);
    }
    let backend_state = executor.spawn_http_artifact_status_worker(
        status_url,
        bearer_token,
        poll_interval_ms,
        offer,
        binding,
        backend_state,
    )?;
    Ok(ReconcileResult::StillRunning {
        events: Vec::new(),
        backend_state,
        status: if state.cancel_requested {
            RunStatus::Reconciling
        } else {
            RunStatus::Running
        },
    })
}

fn cancel_http_job(
    executor: &LiveAdapterExecutor,
    cancel_url: Option<&str>,
    bearer_token: Option<&str>,
    offer: &ConfiguredOffer,
    binding: &RuntimeCreateBinding,
    backend_state: &Value,
    allow_send: bool,
) -> std::result::Result<CancelResult, AdapterFault> {
    if is_http_job_creating_backend_state(backend_state) {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend create receipt is not available",
        ));
    }
    let state = parse_http_job_backend_state(backend_state)?;
    if !state.cancel_requested {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend cancel state missing reservation",
        ));
    }
    if state.cancel_deadline_ms.is_none() {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend cancel state missing deadline",
        ));
    }
    if allow_send {
        if let Some(cancel_url) = cancel_url {
            let backend_state = executor.spawn_http_artifact_cancel_worker(
                cancel_url,
                bearer_token,
                offer,
                binding,
                backend_state,
            )?;
            return Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state,
            });
        }
    }
    Ok(CancelResult::Reconciling {
        events: Vec::new(),
        backend_state: serialize_http_job_backend_state(&state)?,
    })
}

async fn run_http_artifact_cancel_worker_inner(
    cancel_url: &str,
    bearer_token: Option<&str>,
    offer: &ConfiguredOffer,
    _binding: &RuntimeCreateBinding,
    backend_state: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let state = parse_http_job_backend_state(backend_state)?;
    let deadline = state.cancel_deadline_ms.ok_or_else(|| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend cancel state missing deadline",
        )
    })?;
    if now_ms() >= deadline {
        return Ok(worker_settlement_unknown_result());
    }
    let client = backend_client(offer.policy.runtime_ms_limit)?;
    let mut request = client
        .post(cancel_url)
        .header("content-type", "application/json")
        .json(&json!({ "job_id": state.job_id }));
    if let Some(bearer_token) = bearer_token {
        request = request.header("authorization", format!("Bearer {bearer_token}"));
    }
    let response = request.send().await.map_err(map_reqwest_failure)?;
    if !response.status().is_success() {
        return Err(AdapterFault::backend_failed(
            "model backend failed",
            format!("unexpected backend status {}", response.status().as_u16()),
        ));
    }
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        let _ = read_bounded_json_response_async(response).await?;
    }
    preserve_http_job_active_after_cancel_attempt_result(backend_state)
}

fn reserve_http_job_cancel(
    backend_state: &Value,
    has_cancel_endpoint: bool,
    offer: &ConfiguredOffer,
) -> std::result::Result<CancelReservation, AdapterFault> {
    if is_http_job_creating_backend_state(backend_state) {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend create receipt is not available",
        ));
    }
    let mut state = parse_http_job_backend_state(backend_state)?;
    let now = now_ms();
    let mut allow_send = false;
    if !state.cancel_requested {
        state.cancel_requested = true;
        state.cancel_deadline_ms =
            Some(now.saturating_add(offer.policy.cancel_settlement_timeout_ms));
        state.cancel_sent = has_cancel_endpoint;
        allow_send = has_cancel_endpoint;
    } else {
        if state.cancel_deadline_ms.is_none() {
            return Err(AdapterFault::malformed(
                "model backend returned invalid data",
                "job backend cancel state missing deadline",
            ));
        }
        if has_cancel_endpoint && !state.cancel_sent {
            state.cancel_sent = true;
        }
    }
    state.next_poll_at_ms = now;
    Ok(CancelReservation {
        backend_state: serialize_http_job_backend_state(&state)?,
        allow_send,
    })
}

#[cfg(test)]
fn absorb_cancel_poll_fault(
    mut state: HttpJobBackendState,
    poll_interval_ms: u64,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let now = now_ms();
    let deadline = state.cancel_deadline_ms.ok_or_else(|| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend cancel state missing deadline",
        )
    })?;
    if now >= deadline {
        return Ok(ReconcileResult::Terminal {
            events: Vec::new(),
            status: RunStatus::SettlementUnknown,
            output: None,
            error: Some(RunError {
                class: ErrorClass::SettlementUnknown,
                code: "settlement_unknown".to_string(),
                message: "model backend settlement is unknown".to_string(),
            }),
        });
    }
    state.next_poll_at_ms = next_poll_at_ms(now, poll_interval_ms).min(deadline);
    Ok(ReconcileResult::StillRunning {
        events: Vec::new(),
        backend_state: serialize_http_job_backend_state(&state)?,
        status: RunStatus::Reconciling,
    })
}

fn sanitize_artifact_output(
    output: &Value,
    offer: &ConfiguredOffer,
) -> std::result::Result<Value, AdapterFault> {
    let object = output.as_object().ok_or_else(|| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "artifact output must be an object",
        )
    })?;
    if object.len() != 2 || !object.contains_key("schema") || !object.contains_key("uri") {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "artifact output must contain only schema and uri",
        ));
    }
    let schema = object
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AdapterFault::malformed(
                "model backend returned invalid data",
                "artifact output missing schema",
            )
        })?;
    match schema {
        RUN_OUTPUT_OBJECT_SCHEMA => {
            let uri = object.get("uri").and_then(Value::as_str).ok_or_else(|| {
                AdapterFault::malformed(
                    "model backend returned invalid data",
                    "object output missing uri",
                )
            })?;
            validate_artifact_uri(uri, "elastos://object/")?;
        }
        RUN_OUTPUT_CONTENT_SCHEMA => {
            let uri = object.get("uri").and_then(Value::as_str).ok_or_else(|| {
                AdapterFault::malformed(
                    "model backend returned invalid data",
                    "content output missing uri",
                )
            })?;
            validate_artifact_uri(uri, "elastos://content/")?;
        }
        _ => {
            return Err(AdapterFault::malformed(
                "model backend returned invalid data",
                "unsupported artifact output schema",
            ))
        }
    }
    sanitize_output(output, offer)?;
    Ok(output.clone())
}

pub fn sanitize_output(
    output: &Value,
    offer: &ConfiguredOffer,
) -> std::result::Result<(), AdapterFault> {
    if contains_forbidden_output_field(output) {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "output contains forbidden authority or topology fields",
        ));
    }
    let encoded = serde_json::to_vec(output).map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("failed to encode output: {err}"),
        )
    })?;
    if encoded.len() as u64 > offer.policy.inline_output_bytes_limit {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "output exceeds inline_output_bytes_limit",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HttpJobBackendState {
    schema: String,
    phase: String,
    job_id: String,
    next_poll_at_ms: u64,
    cancel_requested: bool,
    cancel_sent: bool,
    cancel_deadline_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpJobStatusProgress {
    phase: String,
    completed: u64,
    total: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpJobStatusResponse {
    state: String,
    #[serde(default)]
    progress: Option<HttpJobStatusProgress>,
    #[serde(default)]
    output: Option<Value>,
}

fn parse_http_job_backend_state(
    value: &Value,
) -> std::result::Result<HttpJobBackendState, AdapterFault> {
    let state: HttpJobBackendState = serde_json::from_value(value.clone()).map_err(|_| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend state is invalid",
        )
    })?;
    if state.schema != HTTP_JOB_BACKEND_STATE_SCHEMA {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend state schema is invalid",
        ));
    }
    if state.phase != HTTP_JOB_BACKEND_STATE_ACTIVE {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend state phase is invalid",
        ));
    }
    validate_job_id(&state.job_id)?;
    Ok(state)
}

pub(crate) fn is_http_job_creating_backend_state(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 2
        && object.get("schema").and_then(Value::as_str) == Some(HTTP_JOB_BACKEND_STATE_SCHEMA)
        && object.get("phase").and_then(Value::as_str) == Some(HTTP_JOB_BACKEND_STATE_CREATING)
}

fn serialize_http_job_creating_backend_state() -> std::result::Result<Value, AdapterFault> {
    serde_json::to_value(json!({
        "schema": HTTP_JOB_BACKEND_STATE_SCHEMA,
        "phase": HTTP_JOB_BACKEND_STATE_CREATING,
    }))
    .map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("failed to encode backend state: {err}"),
        )
    })
}

fn serialize_http_job_backend_state(
    state: &HttpJobBackendState,
) -> std::result::Result<Value, AdapterFault> {
    serde_json::to_value(state).map_err(|err| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            format!("failed to encode backend state: {err}"),
        )
    })
}

fn http_job_still_running_result(
    state: HttpJobBackendState,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    Ok(ReconcileResult::StillRunning {
        events: Vec::new(),
        backend_state: serialize_http_job_backend_state(&state)?,
        status: if state.cancel_requested {
            RunStatus::Reconciling
        } else {
            RunStatus::Running
        },
    })
}

fn bounded_http_job_next_poll_at_ms(
    state: &HttpJobBackendState,
    poll_interval_ms: u64,
) -> std::result::Result<u64, AdapterFault> {
    let next_poll = next_poll_at_ms(now_ms(), poll_interval_ms);
    if state.cancel_requested {
        let deadline = state.cancel_deadline_ms.ok_or_else(|| {
            AdapterFault::malformed(
                "model backend returned invalid data",
                "job backend cancel state missing deadline",
            )
        })?;
        return Ok(next_poll.min(deadline));
    }
    Ok(next_poll)
}

fn preserve_http_job_active_result(
    backend_state: &Value,
    poll_interval_ms: u64,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let mut state = parse_http_job_backend_state(backend_state)?;
    let now = now_ms();
    if state.cancel_requested
        && state
            .cancel_deadline_ms
            .map(|deadline| now >= deadline)
            .unwrap_or(false)
    {
        return Ok(worker_settlement_unknown_result());
    }
    state.next_poll_at_ms = bounded_http_job_next_poll_at_ms(&state, poll_interval_ms)?;
    http_job_still_running_result(state)
}

fn preserve_http_job_active_after_cancel_attempt_result(
    backend_state: &Value,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let mut state = parse_http_job_backend_state(backend_state)?;
    let now = now_ms();
    if state.cancel_requested
        && state
            .cancel_deadline_ms
            .map(|deadline| now >= deadline)
            .unwrap_or(false)
    {
        return Ok(worker_settlement_unknown_result());
    }
    state.next_poll_at_ms = now;
    http_job_still_running_result(state)
}

fn parse_http_job_status_result(
    value: Value,
    offer: &ConfiguredOffer,
    mut state: HttpJobBackendState,
    poll_interval_ms: u64,
) -> std::result::Result<ReconcileResult, AdapterFault> {
    let response: HttpJobStatusResponse = serde_json::from_value(value).map_err(|_| {
        AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend returned invalid status response",
        )
    })?;
    match response.state.as_str() {
        "queued" | "running" => {
            if response.output.is_some() {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "job backend running status must not include output",
                ));
            }
            let mut events = Vec::new();
            if let Some(progress) = response.progress {
                validate_http_job_progress(&progress)?;
                events.push(EventSeed {
                    kind: "progress",
                    data: json!({
                        "phase": progress.phase,
                        "completed": progress.completed,
                        "total": progress.total,
                    }),
                });
            }
            state.next_poll_at_ms = bounded_http_job_next_poll_at_ms(&state, poll_interval_ms)?;
            Ok(ReconcileResult::StillRunning {
                events,
                backend_state: serialize_http_job_backend_state(&state)?,
                status: if state.cancel_requested {
                    RunStatus::Reconciling
                } else {
                    RunStatus::Running
                },
            })
        }
        "cancelled" => {
            if response.progress.is_some() || response.output.is_some() {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "job backend cancelled status must not include payload",
                ));
            }
            Ok(ReconcileResult::Terminal {
                events: Vec::new(),
                status: RunStatus::Cancelled,
                output: None,
                error: Some(RunError {
                    class: ErrorClass::Cancelled,
                    code: "cancelled".to_string(),
                    message: "model run was cancelled".to_string(),
                }),
            })
        }
        "failed" => {
            if response.progress.is_some() || response.output.is_some() {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "job backend failed status must not include payload",
                ));
            }
            Ok(ReconcileResult::Terminal {
                events: Vec::new(),
                status: RunStatus::Failed,
                output: None,
                error: Some(RunError {
                    class: ErrorClass::BackendFailed,
                    code: "backend_failed".to_string(),
                    message: "model backend failed".to_string(),
                }),
            })
        }
        "completed" => {
            if response.progress.is_some() {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "job backend completed status must not include progress",
                ));
            }
            let output =
                sanitize_artifact_output(response.output.as_ref().unwrap_or(&Value::Null), offer)?;
            Ok(ReconcileResult::Terminal {
                events: Vec::new(),
                status: RunStatus::Completed,
                output: Some(output),
                error: None,
            })
        }
        "settlement_unknown" => {
            if response.progress.is_some() || response.output.is_some() {
                return Err(AdapterFault::malformed(
                    "model backend returned invalid data",
                    "job backend settlement_unknown must not include payload",
                ));
            }
            Ok(worker_settlement_unknown_result())
        }
        _ => Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend returned unsupported state",
        )),
    }
}

fn validate_http_job_progress(
    progress: &HttpJobStatusProgress,
) -> std::result::Result<(), AdapterFault> {
    if progress.phase.is_empty()
        || progress.phase.trim() != progress.phase
        || progress.phase.len() > MAX_HTTP_JOB_PROGRESS_PHASE_BYTES
        || progress.phase.chars().any(|ch| ch.is_control())
    {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend returned invalid progress phase",
        ));
    }
    if progress.total > MAX_HTTP_JOB_PROGRESS_COUNT
        || progress.completed > MAX_HTTP_JOB_PROGRESS_COUNT
        || progress.completed > progress.total
    {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend returned invalid progress counts",
        ));
    }
    Ok(())
}

fn validate_job_id(job_id: &str) -> std::result::Result<(), AdapterFault> {
    if job_id.trim().is_empty()
        || job_id.trim() != job_id
        || job_id.len() > MAX_BACKEND_JOB_ID_BYTES
        || job_id.chars().any(|ch| ch.is_control())
    {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "job backend returned invalid job_id",
        ));
    }
    Ok(())
}

fn validate_artifact_uri(uri: &str, prefix: &str) -> std::result::Result<(), AdapterFault> {
    if uri.trim() != uri || !uri.starts_with(prefix) || uri.len() <= prefix.len() {
        return Err(AdapterFault::malformed(
            "model backend returned invalid data",
            "artifact output uri is invalid",
        ));
    }
    Ok(())
}

fn contains_forbidden_output_field(value: &Value) -> bool {
    const FORBIDDEN_KEYS: &[&str] = &[
        "grant",
        "effect",
        "capability",
        "backend_url",
        "process_path",
        "path",
        "url",
        "headers",
        "authorization",
        "token",
        "credential",
        "credentials",
        "carrier_route",
        "endpoint_did",
        "port",
        "job_id",
        "principal_id",
        "session_id",
        "grant_id",
    ];
    match value {
        Value::Object(object) => object.iter().any(|(key, nested)| {
            FORBIDDEN_KEYS.contains(&key.as_str()) || contains_forbidden_output_field(nested)
        }),
        Value::Array(values) => values.iter().any(contains_forbidden_output_field),
        _ => false,
    }
}

fn status_request_url(status_url: &str, job_id: &str) -> std::result::Result<String, AdapterFault> {
    let mut url = Url::parse(status_url).map_err(|_| {
        AdapterFault::context(
            "model input is invalid",
            "job backend status_url is invalid",
        )
    })?;
    url.query_pairs_mut().append_pair("job_id", job_id);
    Ok(url.into())
}

fn next_poll_at_ms(now_ms: u64, poll_interval_ms: u64) -> u64 {
    now_ms.saturating_add(poll_interval_ms)
}

fn bound_detail(detail: String) -> String {
    if detail.len() <= MAX_BACKEND_LOG_DETAIL_BYTES {
        return detail;
    }
    let truncated = String::from_utf8_lossy(&detail.as_bytes()[..MAX_BACKEND_LOG_DETAIL_BYTES]);
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OfferPolicy, MAX_POLL_INTERVAL_MS};
    use crate::contract::RUNTIME_CREATE_BINDING_SCHEMA;
    use std::io::{self, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::thread;
    use std::time::Instant;

    #[derive(Clone)]
    struct HttpResponseSpec {
        status_line: &'static str,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    }

    struct TestServer {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
        shutdown: Arc<AtomicBool>,
        join: Option<thread::JoinHandle<()>>,
    }

    struct RunningRuntime {
        handle: Handle,
        stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
        join: Option<thread::JoinHandle<()>>,
    }

    impl RunningRuntime {
        fn start() -> Self {
            let (handle_tx, handle_rx) = std_mpsc::channel();
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
            let join = thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                handle_tx.send(runtime.handle().clone()).unwrap();
                runtime.block_on(async {
                    let _ = stop_rx.await;
                });
            });
            let handle = handle_rx.recv().unwrap();
            Self {
                handle,
                stop_tx: Some(stop_tx),
                join: Some(join),
            }
        }
    }

    impl Drop for RunningRuntime {
        fn drop(&mut self) {
            if let Some(stop_tx) = self.stop_tx.take() {
                let _ = stop_tx.send(());
            }
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.base_url.strip_prefix("http://").unwrap_or(""));
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn start_server(responses: Vec<HttpResponseSpec>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let requests_clone = Arc::clone(&requests);
        let shutdown_clone = Arc::clone(&shutdown);
        let join = thread::spawn(move || {
            for spec in responses {
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(pair) => break pair,
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            if shutdown_clone.load(Ordering::Relaxed) {
                                return;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(err) => panic!("test server accept failed: {err}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                let request = read_request(&mut stream);
                requests_clone.lock().unwrap().push(request);
                let mut response = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    spec.status_line,
                    spec.body.len()
                );
                for (name, value) in &spec.headers {
                    response.push_str(name);
                    response.push_str(": ");
                    response.push_str(value);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(&spec.body).unwrap();
                stream.flush().unwrap();
            }
        });
        TestServer {
            base_url,
            requests,
            shutdown,
            join: Some(join),
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn offer() -> ConfiguredOffer {
        ConfiguredOffer {
            id: "offer-1".to_string(),
            title: "Offer".to_string(),
            operation: "image.generate".to_string(),
            input_modalities: vec!["application/json".to_string()],
            output_modalities: vec!["application/json".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 1,
                input_bytes_limit: 4096,
                inline_output_bytes_limit: 4096,
                event_bytes_limit: 2048,
                runtime_ms_limit: 5000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 20,
            },
            adapter: AdapterConfig::HttpJobArtifact {
                create_url: "http://invalid/create".to_string(),
                status_url: "http://invalid/status".to_string(),
                cancel_url: Some("http://invalid/cancel".to_string()),
                bearer_token: Some("secret".to_string()),
                poll_interval_ms: 5,
            },
            enabled: true,
        }
    }

    fn binding() -> RuntimeCreateBinding {
        RuntimeCreateBinding {
            schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
            principal_id: "principal-1".to_string(),
            session_id: "session-1".to_string(),
            capsule_id: "assistant".to_string(),
            grant_id: "grant-1".to_string(),
            request_id: "request-1".to_string(),
            offer_id: "offer-1".to_string(),
            operation: "image.generate".to_string(),
            input_hash: "hash-1".to_string(),
        }
    }

    fn openai_offer(api_url: &str) -> ConfiguredOffer {
        ConfiguredOffer {
            id: "offer-1".to_string(),
            title: "Offer".to_string(),
            operation: "image.generate".to_string(),
            input_modalities: vec!["application/json".to_string()],
            output_modalities: vec!["application/json".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 1,
                input_bytes_limit: 4096,
                inline_output_bytes_limit: 4096,
                event_bytes_limit: 2048,
                runtime_ms_limit: 5000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 20,
            },
            adapter: AdapterConfig::OpenAiCompatibleText {
                api_url: api_url.to_string(),
                api_key: Some("secret".to_string()),
                model: "gpt-test".to_string(),
            },
            enabled: true,
        }
    }

    fn text_input(prompt: &str) -> Value {
        json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": prompt,
        })
    }

    fn sse_body(events: &[&str], done: bool) -> Vec<u8> {
        let mut body = Vec::new();
        for event in events {
            body.extend_from_slice(b"data: ");
            body.extend_from_slice(event.as_bytes());
            body.extend_from_slice(b"\n\n");
        }
        if done {
            body.extend_from_slice(b"data: [DONE]\n\n");
        }
        body
    }

    fn wait_for_request_count(server: &TestServer, expected: usize) {
        let started = Instant::now();
        loop {
            if server.requests.lock().unwrap().len() >= expected {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_millis(250),
                "timed out waiting for {expected} request(s); saw {}",
                server.requests.lock().unwrap().len()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn assert_no_request_while_registry_gate_is_held(server: &TestServer) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(100) {
            assert_eq!(
                server.requests.lock().unwrap().len(),
                0,
                "local text worker contacted backend before ownership was recorded"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn duplicate_local_text_dispatch_creates_no_unowned_or_duplicate_worker() {
        let server = start_server(vec![
            HttpResponseSpec {
                status_line: "200 OK",
                body: sse_body(&[r#"{"choices":[{"delta":{"content":"ready"}}]}"#], true),
                headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            },
            HttpResponseSpec {
                status_line: "200 OK",
                body: sse_body(&[r#"{"choices":[{"delta":{"content":"shadow"}}]}"#], true),
                headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            },
        ]);
        let runtime = RunningRuntime::start();
        let (update_tx, mut update_rx) = mpsc::channel(8);
        let executor = LiveAdapterExecutor::new(runtime.handle.clone(), update_tx);
        let offer = openai_offer(&format!("{}/chat", server.base_url));
        let input = text_input("hello");
        let run_binding = binding();
        let run_id = deterministic_run_id(&run_binding);
        let registry_guard = executor.workers.lock().unwrap();
        let (started_tx, started_rx) = std_mpsc::channel();
        let (result_tx, result_rx) = std_mpsc::channel();
        for _ in 0..2 {
            let executor = executor.clone();
            let offer = offer.clone();
            let input = input.clone();
            let run_binding = run_binding.clone();
            let started_tx = started_tx.clone();
            let result_tx = result_tx.clone();
            let url = format!("{}/chat", server.base_url);
            thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = dispatch_openai_text(
                    &executor,
                    &url,
                    Some("secret"),
                    "gpt-test",
                    &offer,
                    &run_binding,
                    &input,
                )
                .map(|_| ());
                result_tx.send(result).unwrap();
            });
        }
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        assert_no_request_while_registry_gate_is_held(&server);
        assert!(
            update_rx.try_recv().is_err(),
            "worker must not emit updates before ownership is recorded"
        );
        drop(registry_guard);
        wait_for_request_count(&server, 1);

        let first = result_rx.recv().unwrap();
        let second = result_rx.recv().unwrap();
        let results = [first, second];
        let success_count = results.iter().filter(|result| result.is_ok()).count();
        let duplicate_count = results
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    Err(fault) if fault.error.class == ErrorClass::ContextRejected
                )
            })
            .count();
        assert_eq!(success_count, 1);
        assert_eq!(duplicate_count, 1);

        let generation = executor
            .workers
            .lock()
            .unwrap()
            .get(&run_id)
            .unwrap()
            .generation;
        let mut saw_terminal = 0usize;
        let mut saw_exited = 0usize;
        loop {
            let update = runtime
                .handle
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(1), update_rx.recv()).await
                })
                .unwrap()
                .unwrap();
            match update {
                WorkerUpdate::Apply {
                    run_id: update_run_id,
                    generation: update_generation,
                    result,
                    acknowledge,
                    ..
                } => {
                    assert_eq!(update_run_id, run_id);
                    assert_eq!(update_generation, generation);
                    acknowledge
                        .send(WorkerApplyAck::Applied)
                        .expect("coordinator must accept owned worker update");
                    if let ReconcileResult::Terminal { status, output, .. } = result {
                        saw_terminal += 1;
                        assert_eq!(status, RunStatus::Completed);
                        assert_eq!(
                            output,
                            Some(json!({
                                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                                "text": "ready",
                            }))
                        );
                    }
                }
                WorkerUpdate::Exited {
                    run_id: update_run_id,
                    generation: update_generation,
                } => {
                    assert_eq!(update_run_id, run_id);
                    assert_eq!(update_generation, generation);
                    saw_exited += 1;
                    break;
                }
            }
        }
        assert_eq!(saw_terminal, 1);
        assert_eq!(saw_exited, 1);
        runtime.handle.block_on(executor.shutdown_workers());
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn local_text_acknowledged_coalescing_preserves_exact_final_text() {
        let server = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: sse_body(
                &[
                    &format!(
                        r#"{{"choices":[{{"delta":{{"content":"{}"}}}}]}}"#,
                        "a".repeat(5_000)
                    ),
                    &format!(
                        r#"{{"choices":[{{"delta":{{"content":"{}"}}}}]}}"#,
                        "b".repeat(5_000)
                    ),
                    &format!(
                        r#"{{"choices":[{{"delta":{{"content":"{}"}}}}]}}"#,
                        "c".repeat(10)
                    ),
                ],
                true,
            ),
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        }]);
        let mut offer = openai_offer(&format!("{}/chat", server.base_url));
        offer.policy.event_bytes_limit = 16 * 1024;
        offer.policy.inline_output_bytes_limit = 16 * 1024;
        let (update_tx, mut update_rx) = mpsc::channel(8);
        let (result_tx, result_rx) = std_mpsc::channel();
        let url = format!("{}/chat", server.base_url);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = runtime.block_on(async move {
                let mut task = LocalTextWorkerTask {
                    run_id: "run-coalesced".to_string(),
                    generation: 1,
                    api_url: url,
                    api_key: Some("secret".to_string()),
                    model: "gpt-test".to_string(),
                    offer,
                    prompt: "hello".to_string(),
                    cancel_rx,
                    updates: update_tx,
                };
                run_local_text_worker_inner(&mut task).await
            });
            result_tx.send(result).unwrap();
        });
        let recv_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let first_update = match recv_runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), update_rx.recv()).await
        }) {
            Ok(Some(update)) => update,
            Ok(None) => match result_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(result)) => panic!(
                    "first coalesced delta update channel closed; worker returned result: {result:?}"
                ),
                Ok(Err(fault)) => panic!(
                    "first coalesced delta update channel closed; worker fault: {fault:?}"
                ),
                Err(err) => panic!(
                    "first coalesced delta update channel closed; worker result unavailable: {err}"
                ),
            },
            Err(_) => match result_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(result)) => panic!(
                    "timed out waiting for first coalesced delta update; worker returned result: {result:?}"
                ),
                Ok(Err(fault)) => panic!(
                    "timed out waiting for first coalesced delta update; worker fault: {fault:?}"
                ),
                Err(err) => panic!(
                    "timed out waiting for first coalesced delta update; worker result unavailable: {err}"
                ),
            },
        };
        let _keep_cancel_sender_alive = cancel_tx;
        match first_update {
            WorkerUpdate::Apply {
                result: ReconcileResult::StillRunning { events, status, .. },
                acknowledge,
                ..
            } => {
                assert_eq!(status, RunStatus::Running);
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].kind, "text_delta");
                assert_eq!(
                    events[0].data,
                    json!({"text": format!("{}{}", "a".repeat(5_000), "b".repeat(5_000))})
                );
                acknowledge
                    .send(WorkerApplyAck::Applied)
                    .expect("first delta ack should succeed");
            }
            other => panic!("unexpected first coalesced update: {other:?}"),
        }
        let second_update = update_rx
            .blocking_recv()
            .expect("terminal flush delta update must arrive");
        match second_update {
            WorkerUpdate::Apply {
                result: ReconcileResult::StillRunning { events, status, .. },
                acknowledge,
                ..
            } => {
                assert_eq!(status, RunStatus::Running);
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].kind, "text_delta");
                assert_eq!(events[0].data, json!({"text":"cccccccccc"}));
                acknowledge
                    .send(WorkerApplyAck::Applied)
                    .expect("terminal flush ack should succeed");
            }
            other => panic!("unexpected terminal flush update: {other:?}"),
        }
        let result = result_rx.recv().unwrap().unwrap();
        let ReconcileResult::Terminal {
            status,
            output,
            error,
            ..
        } = result
        else {
            panic!("expected completed result");
        };
        assert_eq!(status, RunStatus::Completed);
        assert!(error.is_none());
        assert_eq!(
            output,
            Some(json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": format!("{}{}{}", "a".repeat(5_000), "b".repeat(5_000), "c".repeat(10)),
            }))
        );
        assert!(update_rx.try_recv().is_err());
    }

    #[test]
    fn local_text_apply_rejection_fails_closed_before_completion() {
        let mut offer = openai_offer("http://example.invalid/chat");
        offer.policy.event_bytes_limit = 16 * 1024;
        offer.policy.inline_output_bytes_limit = 16 * 1024;
        let (update_tx, mut update_rx) = mpsc::channel(8);
        let (result_tx, result_rx) = std_mpsc::channel();
        let mut delta_buffer = "rejectable delta".to_string();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = runtime.block_on(async move {
                flush_local_text_delta(&offer, "run-rejected", 1, &update_tx, &mut delta_buffer)
                    .await
            });
            result_tx.send(result).unwrap();
        });
        let update = update_rx
            .blocking_recv()
            .expect("rejectable delta update must arrive");
        match update {
            WorkerUpdate::Apply {
                result: ReconcileResult::StillRunning { status, events, .. },
                acknowledge,
                ..
            } => {
                assert_eq!(status, RunStatus::Running);
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].kind, "text_delta");
                assert_eq!(events[0].data, json!({"text":"rejectable delta"}));
                acknowledge
                    .send(WorkerApplyAck::Rejected)
                    .expect("rejection ack should reach worker");
            }
            other => panic!("expected rejected delta update, got {other:?}"),
        }
        let fault = result_rx.recv().unwrap().unwrap_err();
        assert_eq!(fault.error.class, ErrorClass::ContextRejected);
        assert_eq!(fault.error.code, "context_rejected");
        assert_eq!(fault.error.message, "model run could not continue");
        assert_eq!(
            fault.detail.as_deref(),
            Some("worker progress update was rejected")
        );
        assert!(update_rx.try_recv().is_err());
    }

    #[test]
    fn local_text_accounting_rejections_leave_state_byte_identical() {
        let mut offer = openai_offer("http://example.invalid/chat");
        offer.policy.inline_output_bytes_limit = 64;
        let mut state = LocalTextStreamState {
            output_text: "stable".to_string(),
            delta_buffer: "queued".to_string(),
            consumed_response_bytes: 17,
        };
        let before_output = state.output_text.as_bytes().to_vec();
        let before_delta = state.delta_buffer.as_bytes().to_vec();
        let before_bytes = state.consumed_response_bytes;
        let output_fault =
            append_local_text_delta(&mut state, &offer, &"x".repeat(128)).unwrap_err();
        assert_eq!(output_fault.error.class, ErrorClass::ResponseMalformed);
        assert_eq!(state.output_text.as_bytes(), before_output.as_slice());
        assert_eq!(state.delta_buffer.as_bytes(), before_delta.as_slice());
        assert_eq!(state.consumed_response_bytes, before_bytes);

        let mut aggregate_state = LocalTextStreamState {
            output_text: "stable".to_string(),
            delta_buffer: "queued".to_string(),
            consumed_response_bytes: MAX_BACKEND_BODY_BYTES - 4,
        };
        let before_aggregate = serde_json::to_vec(&json!({
            "output_text": aggregate_state.output_text,
            "delta_buffer": aggregate_state.delta_buffer,
            "consumed_response_bytes": aggregate_state.consumed_response_bytes,
        }))
        .unwrap();
        let aggregate_fault = consume_local_text_stream_bytes(&mut aggregate_state, 5).unwrap_err();
        assert_eq!(aggregate_fault.error.class, ErrorClass::ResponseMalformed);
        assert_eq!(
            aggregate_fault.detail.as_deref(),
            Some("text stream body exceeds provider limits")
        );
        let after_aggregate = serde_json::to_vec(&json!({
            "output_text": aggregate_state.output_text,
            "delta_buffer": aggregate_state.delta_buffer,
            "consumed_response_bytes": aggregate_state.consumed_response_bytes,
        }))
        .unwrap();
        assert_eq!(after_aggregate, before_aggregate);
    }

    #[test]
    fn truncated_local_text_stream_is_not_completed() {
        let server = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: sse_body(&[r#"{"choices":[{"delta":{"content":"partial"}}]}"#], false),
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        }]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (update_tx, mut update_rx) = mpsc::channel(8);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let mut task = LocalTextWorkerTask {
            run_id: "run-truncated".to_string(),
            generation: 1,
            api_url: format!("{}/chat", server.base_url),
            api_key: Some("secret".to_string()),
            model: "gpt-test".to_string(),
            offer: openai_offer(&format!("{}/chat", server.base_url)),
            prompt: "hello".to_string(),
            cancel_rx,
            updates: update_tx,
        };
        let result = runtime.block_on(run_local_text_worker_inner(&mut task));
        let fault = result.unwrap_err();
        assert_eq!(fault.error.class, ErrorClass::ResponseMalformed);
        assert!(update_rx.try_recv().is_err());
        assert!(update_rx.try_recv().is_err());
    }

    #[test]
    fn local_text_control_channel_loss_settles_unknown() {
        let server = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: Vec::new(),
            headers: vec![
                ("Content-Type".to_string(), "text/event-stream".to_string()),
                ("Transfer-Encoding".to_string(), "chunked".to_string()),
            ],
        }]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (update_tx, mut update_rx) = mpsc::channel(8);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        drop(cancel_tx);
        let mut task = LocalTextWorkerTask {
            run_id: "run-control-loss".to_string(),
            generation: 1,
            api_url: format!("{}/chat", server.base_url),
            api_key: Some("secret".to_string()),
            model: "gpt-test".to_string(),
            offer: openai_offer(&format!("{}/chat", server.base_url)),
            prompt: "hello".to_string(),
            cancel_rx,
            updates: update_tx,
        };
        let result = runtime.block_on(run_local_text_worker_inner(&mut task));
        let ReconcileResult::Terminal {
            status,
            output,
            error,
            ..
        } = result.unwrap()
        else {
            panic!("expected settlement_unknown terminal result");
        };
        assert_eq!(status, RunStatus::SettlementUnknown);
        assert!(output.is_none());
        let error = error.expect("control loss must carry settlement_unknown");
        assert_eq!(error.class, ErrorClass::SettlementUnknown);
        assert_eq!(error.code, "settlement_unknown");
        assert!(update_rx.try_recv().is_err());
    }

    #[test]
    fn reconcile_http_job_encodes_query_and_suppresses_polling() {
        let server = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: br#"{"state":"running"}"#.to_vec(),
            headers: Vec::new(),
        }]);
        let runtime = RunningRuntime::start();
        let executor = LiveAdapterExecutor::new(runtime.handle.clone(), mpsc::channel(1).0);
        let offer = offer();
        let state = HttpJobBackendState {
            schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
            phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
            job_id: "job plus/slash?".to_string(),
            next_poll_at_ms: now_ms().saturating_add(MAX_POLL_INTERVAL_MS),
            cancel_requested: false,
            cancel_sent: false,
            cancel_deadline_ms: None,
        };
        let result = reconcile_http_job(
            &executor,
            &format!("{}/status", server.base_url),
            None,
            5,
            &offer,
            &binding(),
            &serialize_http_job_backend_state(&state).unwrap(),
        )
        .unwrap();
        let ReconcileResult::StillRunning { .. } = result else {
            panic!("expected suppressed poll");
        };
        assert!(server.requests.lock().unwrap().is_empty());

        let state = HttpJobBackendState {
            next_poll_at_ms: 0,
            ..state
        };
        let runtime = RunningRuntime::start();
        let result = runtime
            .handle
            .block_on(run_http_artifact_status_worker_inner(
                &format!("{}/status", server.base_url),
                None,
                5,
                &offer,
                &binding(),
                &serialize_http_job_backend_state(&state).unwrap(),
            ))
            .unwrap();
        let ReconcileResult::StillRunning {
            backend_state,
            status,
            ..
        } = result
        else {
            panic!("expected running poll");
        };
        assert_eq!(status, RunStatus::Running);
        assert!(backend_state.get("job_id").is_some());
        let requests = server.requests.lock().unwrap();
        assert!(requests[0].contains("GET /status?job_id=job+plus%2Fslash%3F "));
    }

    #[test]
    fn sanitize_output_recursively_rejects_forbidden_fields() {
        let fault = sanitize_output(
            &json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "ok",
                "nested": {
                    "grant": { "id": "bad" }
                }
            }),
            &offer(),
        )
        .unwrap_err();
        assert_eq!(fault.error.class, ErrorClass::ResponseMalformed);
    }

    #[test]
    fn reconcile_http_job_cancelled_returns_canonical_cancelled_error() {
        let server = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: br#"{"state":"cancelled"}"#.to_vec(),
            headers: Vec::new(),
        }]);
        let runtime = RunningRuntime::start();
        let result = runtime
            .handle
            .block_on(run_http_artifact_status_worker_inner(
                &format!("{}/status", server.base_url),
                None,
                5,
                &offer(),
                &binding(),
                &serialize_http_job_backend_state(&HttpJobBackendState {
                    schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                    phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                    job_id: "job-9".to_string(),
                    next_poll_at_ms: 0,
                    cancel_requested: false,
                    cancel_sent: false,
                    cancel_deadline_ms: None,
                })
                .unwrap(),
            ))
            .unwrap();

        let ReconcileResult::Terminal {
            status,
            error,
            events,
            output,
        } = result
        else {
            panic!("expected cancelled terminal reconcile result");
        };
        assert_eq!(status, RunStatus::Cancelled);
        assert!(output.is_none());
        assert!(events.is_empty());
        let error = error.expect("cancelled reconcile must carry canonical error");
        assert_eq!(error.class, ErrorClass::Cancelled);
        assert_eq!(error.code, "cancelled");
        assert_eq!(error.message, "model run was cancelled");
    }

    #[test]
    fn reserve_http_job_cancel_sends_once_and_preserves_deadline() {
        let offer = offer();
        let first = reserve_http_job_cancel(
            &serialize_http_job_backend_state(&HttpJobBackendState {
                schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                job_id: "job-9".to_string(),
                next_poll_at_ms: 77,
                cancel_requested: false,
                cancel_sent: false,
                cancel_deadline_ms: None,
            })
            .unwrap(),
            true,
            &offer,
        )
        .unwrap();
        assert!(first.allow_send);
        assert_eq!(first.backend_state["cancel_requested"], json!(true));
        assert_eq!(first.backend_state["cancel_sent"], json!(true));
        let deadline = first.backend_state["cancel_deadline_ms"].as_u64().unwrap();

        let second = reserve_http_job_cancel(&first.backend_state, true, &offer).unwrap();
        assert!(!second.allow_send);
        assert_eq!(second.backend_state["cancel_deadline_ms"], json!(deadline));
        assert_eq!(second.backend_state["cancel_sent"], json!(true));
    }

    #[test]
    fn reserve_http_job_cancel_rejects_missing_deadline_after_reservation() {
        let offer = offer();
        let fault = reserve_http_job_cancel(
            &serialize_http_job_backend_state(&HttpJobBackendState {
                schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                job_id: "job-9".to_string(),
                next_poll_at_ms: 0,
                cancel_requested: true,
                cancel_sent: true,
                cancel_deadline_ms: None,
            })
            .unwrap(),
            true,
            &offer,
        )
        .unwrap_err();
        assert_eq!(fault.error.class, ErrorClass::ResponseMalformed);
    }

    #[test]
    fn cancel_reserved_ambiguous_poll_fault_retries_before_deadline() {
        let deadline = now_ms().saturating_add(1_000);
        let result = absorb_cancel_poll_fault(
            HttpJobBackendState {
                schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                job_id: "job-9".to_string(),
                next_poll_at_ms: 0,
                cancel_requested: true,
                cancel_sent: true,
                cancel_deadline_ms: Some(deadline),
            },
            25,
        )
        .unwrap();
        let ReconcileResult::StillRunning {
            backend_state,
            status,
            ..
        } = result
        else {
            panic!("expected reconciling result");
        };
        assert_eq!(status, RunStatus::Reconciling);
        assert_eq!(backend_state["cancel_deadline_ms"], json!(deadline));
        let next_poll = backend_state["next_poll_at_ms"].as_u64().unwrap();
        assert!(next_poll <= deadline);
        assert!(next_poll >= now_ms());
    }

    #[test]
    fn cancel_reserved_ambiguous_poll_fault_settles_unknown_at_deadline() {
        let result = absorb_cancel_poll_fault(
            HttpJobBackendState {
                schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                job_id: "job-9".to_string(),
                next_poll_at_ms: 0,
                cancel_requested: true,
                cancel_sent: true,
                cancel_deadline_ms: Some(now_ms().saturating_sub(1)),
            },
            25,
        )
        .unwrap();
        let ReconcileResult::Terminal {
            status,
            error,
            events,
            ..
        } = result
        else {
            panic!("expected terminal settlement unknown");
        };
        assert_eq!(status, RunStatus::SettlementUnknown);
        assert_eq!(error.unwrap().class, ErrorClass::SettlementUnknown);
        assert!(events.is_empty());
    }

    #[test]
    fn dispatch_openai_text_does_not_follow_redirects() {
        let target = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: br#"{"choices":[{"message":{"content":"redirected"}}]}"#.to_vec(),
            headers: Vec::new(),
        }]);
        let redirect = start_server(vec![HttpResponseSpec {
            status_line: "302 Found",
            body: br#"{"error":"redirect"}"#.to_vec(),
            headers: vec![(
                "Location".to_string(),
                format!("{}/redirected", target.base_url),
            )],
        }]);
        let runtime = RunningRuntime::start();
        let executor = LiveAdapterExecutor::new(runtime.handle.clone(), mpsc::channel(4).0);
        let openai_offer = openai_offer(&format!("{}/chat", redirect.base_url));

        let fault = dispatch_openai_text(
            &executor,
            &format!("{}/chat", redirect.base_url),
            Some("secret"),
            "gpt-test",
            &openai_offer,
            &binding(),
            &text_input("hello"),
        )
        .unwrap();

        assert!(matches!(fault, DispatchResult::Running { .. }));
        assert_eq!(target.requests.lock().unwrap().len(), 0);
    }

    #[test]
    fn reconcile_http_job_does_not_follow_redirects() {
        let target = start_server(vec![HttpResponseSpec {
            status_line: "200 OK",
            body: br#"{"state":"completed","output":{"schema":"elastos.model.output.text/v1","text":"redirected"}}"#.to_vec(),
            headers: Vec::new(),
        }]);
        let redirect = start_server(vec![HttpResponseSpec {
            status_line: "302 Found",
            body: br#"{"error":"redirect"}"#.to_vec(),
            headers: vec![(
                "Location".to_string(),
                format!("{}/status-redirect", target.base_url),
            )],
        }]);
        let runtime = RunningRuntime::start();

        let fault = runtime
            .handle
            .block_on(run_http_artifact_status_worker_inner(
                &format!("{}/status", redirect.base_url),
                Some("sentinel-bearer"),
                5,
                &offer(),
                &binding(),
                &serialize_http_job_backend_state(&HttpJobBackendState {
                    schema: HTTP_JOB_BACKEND_STATE_SCHEMA.to_string(),
                    phase: HTTP_JOB_BACKEND_STATE_ACTIVE.to_string(),
                    job_id: "job-9".to_string(),
                    next_poll_at_ms: 0,
                    cancel_requested: false,
                    cancel_sent: false,
                    cancel_deadline_ms: None,
                })
                .unwrap(),
            ))
            .unwrap_err();

        assert_eq!(redirect.requests.lock().unwrap().len(), 1);
        assert_eq!(target.requests.lock().unwrap().len(), 0);
        let public_error = serde_json::to_string(&fault.error).unwrap();
        assert!(!public_error.contains("/status-redirect"));
        assert!(!public_error.contains(&target.base_url));
    }
}
