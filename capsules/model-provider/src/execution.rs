use crate::adapters::{LiveAdapterExecutor, WorkerApplyAck, WorkerUpdate};
use crate::contract::{
    ok_response, InitRequest, OffersListRequest, ProviderEnvelope, ProviderFault,
    ProviderOperation, RunsCancelRequest, RunsCreateRequest, RunsEventsRequest, RunsGetRequest,
    StatusResponse, PROVIDER_ID, PROVIDER_PROTOCOL_VERSION,
};
use crate::state::ModelProviderState;
use serde_json::Value;
use std::thread;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, oneshot};

const REQUEST_CHANNEL_CAPACITY: usize = 8;
const UPDATE_CHANNEL_CAPACITY: usize = 32;

enum CoordinatorCommand {
    Request {
        envelope: ProviderEnvelope,
        respond_to: oneshot::Sender<Value>,
    },
    Eof,
}

pub struct ProviderCoordinatorHandle {
    requests: mpsc::Sender<CoordinatorCommand>,
    join: Option<thread::JoinHandle<()>>,
}

impl ProviderCoordinatorHandle {
    pub fn start() -> Self {
        let (request_tx, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        let join = thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build provider coordinator runtime");
            let handle = runtime.handle().clone();
            let (update_tx, update_rx) = mpsc::channel(UPDATE_CHANNEL_CAPACITY);
            runtime.block_on(async move {
                let mut coordinator = ProviderCoordinator {
                    provider: None,
                    handle,
                    requests: request_rx,
                    updates: update_rx,
                    update_tx,
                };
                coordinator.run().await;
            });
        });
        Self {
            requests: request_tx,
            join: Some(join),
        }
    }

    pub fn request(&self, envelope: ProviderEnvelope) -> anyhow::Result<Value> {
        let (respond_to, response_rx) = oneshot::channel();
        self.requests
            .blocking_send(CoordinatorCommand::Request {
                envelope,
                respond_to,
            })
            .map_err(|_| anyhow::anyhow!("provider coordinator is unavailable"))?;
        response_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("provider coordinator response was dropped"))
    }

    pub fn shutdown_on_eof(&mut self) {
        let _ = self.requests.blocking_send(CoordinatorCommand::Eof);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ProviderCoordinator {
    provider: Option<ModelProviderState<LiveAdapterExecutor>>,
    handle: tokio::runtime::Handle,
    requests: mpsc::Receiver<CoordinatorCommand>,
    updates: mpsc::Receiver<WorkerUpdate>,
    update_tx: mpsc::Sender<WorkerUpdate>,
}

impl ProviderCoordinator {
    async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(update) = self.updates.recv() => {
                    self.handle_update(update).await;
                }
                command = self.requests.recv() => {
                    let Some(command) = command else {
                        self.shutdown().await;
                        break;
                    };
                    if self.handle_command(command).await {
                        break;
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: CoordinatorCommand) -> bool {
        match command {
            CoordinatorCommand::Request {
                envelope,
                respond_to,
            } => {
                let shutdown = matches!(envelope.operation, ProviderOperation::Shutdown);
                let response = match self.route_request(envelope).await {
                    Ok(value) => value,
                    Err(err) => {
                        err.log();
                        crate::contract::error_response(err.code(), err.message())
                    }
                };
                let _ = respond_to.send(response);
                if shutdown {
                    self.shutdown().await;
                    return true;
                }
                false
            }
            CoordinatorCommand::Eof => {
                self.shutdown().await;
                true
            }
        }
    }

    async fn route_request(&mut self, request: ProviderEnvelope) -> Result<Value, ProviderFault> {
        match request.operation {
            ProviderOperation::Init => {
                let init = serde_json::from_value::<InitRequest>(request.value)
                    .map_err(|_| ProviderFault::invalid_request("invalid init request body"))?;
                if init.op != "init" {
                    return Err(ProviderFault::invalid_request("invalid init request op"));
                }
                let adapter = LiveAdapterExecutor::new(self.handle.clone(), self.update_tx.clone());
                let mut provider = ModelProviderState::from_init(init.config, adapter)?;
                provider.settle_active_local_text_runs_unknown()?;
                provider.settle_active_http_job_creates_unknown()?;
                self.provider = Some(provider);
                self.status_response()
            }
            ProviderOperation::Status => self.status_response(),
            ProviderOperation::Shutdown => Ok(ok_response(serde_json::json!({
                "shutdown": true
            }))),
            ProviderOperation::OffersList => {
                let provider = self
                    .provider
                    .as_ref()
                    .ok_or_else(ProviderFault::not_initialized)?;
                let request =
                    serde_json::from_value::<OffersListRequest>(request.value).map_err(|_| {
                        ProviderFault::invalid_request("invalid offers_list request body")
                    })?;
                provider.handle_offers_list(request)
            }
            ProviderOperation::RunsCreate => {
                let provider = self
                    .provider
                    .as_mut()
                    .ok_or_else(ProviderFault::not_initialized)?;
                let request =
                    serde_json::from_value::<RunsCreateRequest>(request.value).map_err(|_| {
                        ProviderFault::invalid_request("invalid runs_create request body")
                    })?;
                provider.handle_runs_create(request)
            }
            ProviderOperation::RunsGet => {
                let provider = self
                    .provider
                    .as_mut()
                    .ok_or_else(ProviderFault::not_initialized)?;
                let request = serde_json::from_value::<RunsGetRequest>(request.value)
                    .map_err(|_| ProviderFault::invalid_request("invalid runs_get request body"))?;
                provider.handle_runs_get(request)
            }
            ProviderOperation::RunsEvents => {
                let provider = self
                    .provider
                    .as_mut()
                    .ok_or_else(ProviderFault::not_initialized)?;
                let request =
                    serde_json::from_value::<RunsEventsRequest>(request.value).map_err(|_| {
                        ProviderFault::invalid_request("invalid runs_events request body")
                    })?;
                provider.handle_runs_events(request)
            }
            ProviderOperation::RunsCancel => {
                let provider = self
                    .provider
                    .as_mut()
                    .ok_or_else(ProviderFault::not_initialized)?;
                let request =
                    serde_json::from_value::<RunsCancelRequest>(request.value).map_err(|_| {
                        ProviderFault::invalid_request("invalid runs_cancel request body")
                    })?;
                provider.handle_runs_cancel(request)
            }
            ProviderOperation::Unsupported(op) => Err(ProviderFault::unsupported_operation(&op)),
        }
    }

    fn status_response(&self) -> Result<Value, ProviderFault> {
        Ok(ok_response(
            serde_json::to_value(StatusResponse {
                provider: PROVIDER_ID.to_string(),
                protocol_version: PROVIDER_PROTOCOL_VERSION.to_string(),
                offers_ready: self
                    .provider
                    .as_ref()
                    .map(ModelProviderState::ready_offer_count)
                    .unwrap_or(0),
            })
            .map_err(|_| ProviderFault::internal("failed to serialize provider status"))?,
        ))
    }

    async fn handle_update(&mut self, update: WorkerUpdate) {
        let Some(provider) = self.provider.as_mut() else {
            if let WorkerUpdate::Apply { acknowledge, .. } = update {
                let _ = acknowledge.send(WorkerApplyAck::Rejected);
            }
            return;
        };
        match update {
            WorkerUpdate::Apply {
                run_id,
                generation,
                guard,
                result,
                acknowledge,
            } => {
                if !provider
                    .adapters()
                    .is_current_worker_generation(run_id.as_str(), generation)
                {
                    let _ = acknowledge.send(WorkerApplyAck::Rejected);
                    return;
                }
                let ack =
                    match provider.apply_worker_reconcile_result(run_id.as_str(), guard, result) {
                        Ok(()) => WorkerApplyAck::Applied,
                        Err(err) => {
                            err.log();
                            WorkerApplyAck::Rejected
                        }
                    };
                let _ = acknowledge.send(ack);
            }
            WorkerUpdate::Exited { run_id, generation } => {
                let Some(record) = provider
                    .adapters()
                    .remove_worker_if_current(run_id.as_str(), generation)
                else {
                    return;
                };
                provider.adapters().await_worker_record(record).await;
                if let Err(err) = provider.settle_local_text_run_unknown(run_id.as_str()) {
                    err.log();
                }
                if let Err(err) = provider.settle_http_job_create_run_unknown(run_id.as_str()) {
                    err.log();
                }
            }
        }
    }

    async fn shutdown(&mut self) {
        let Some(provider) = self.provider.as_mut() else {
            return;
        };
        let run_ids = provider.adapters().active_text_run_ids();
        for run_id in run_ids {
            if let Err(err) = provider.settle_local_text_run_unknown(run_id.as_str()) {
                err.log();
            }
        }
        let run_ids = provider.adapters().active_http_create_run_ids();
        for run_id in run_ids {
            if let Err(err) = provider.settle_http_job_create_run_unknown(run_id.as_str()) {
                err.log();
            }
        }
        provider.adapters().shutdown_workers().await;
        while let Ok(update) = self.updates.try_recv() {
            self.handle_update(update).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::serialize_local_text_backend_state;
    use crate::config::{AdapterConfig, ConfiguredOffer, OfferPolicy};
    use crate::contract::{
        model_input_hash, RunEvent, RuntimeAccessBinding, RuntimeCreateBinding,
        RUNTIME_ACCESS_BINDING_SCHEMA, RUNTIME_CREATE_BINDING_SCHEMA, RUN_EVENT_SCHEMA,
        RUN_OUTPUT_TEXT_SCHEMA,
    };
    use crate::journal::{deterministic_run_id, RunJournal, StoredRun};
    use serde_json::{json, Value};
    use std::io::{self, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    const PROMPT_RETURN_BOUND: Duration = Duration::from_millis(250);
    const SHUTDOWN_RETURN_BOUND: Duration = Duration::from_millis(500);
    const WAIT_TIMEOUT: Duration = Duration::from_secs(2);
    const FIXTURE_EVENT_TIMEOUT: Duration = Duration::from_secs(15);
    const TEST_LOCAL_TEXT_STREAM_BYTES_LIMIT: usize = 4 * 1024 * 1024;

    struct ResponseAction {
        status_line: &'static str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        stalled_prefix: Option<Vec<u8>>,
        stalled_suffix: Option<Vec<u8>>,
        hold_open: Option<Arc<AtomicBool>>,
        stalled_body_started: Option<Arc<AtomicBool>>,
        stalled_response_entered: Option<std_mpsc::Sender<()>>,
    }

    struct TestServer {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
        shutdown: Arc<AtomicBool>,
        join: Option<thread::JoinHandle<()>>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.base_url.strip_prefix("http://").unwrap_or(""));
            if let Some(join) = self.join.take() {
                if let Err(err) = join.join() {
                    if !thread::panicking() {
                        std::panic::resume_unwind(err);
                    }
                }
            }
        }
    }

    fn sse_action(events: &[String], done: bool) -> ResponseAction {
        let mut body = Vec::new();
        for event in events {
            body.extend_from_slice(b"data: ");
            body.extend_from_slice(event.as_bytes());
            body.extend_from_slice(b"\n\n");
        }
        if done {
            body.extend_from_slice(b"data: [DONE]\n\n");
        }
        ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body,
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }
    }

    fn stalled_sse_action() -> (ResponseAction, Arc<AtomicBool>, Arc<AtomicBool>) {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "text/event-stream".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "keep-alive".to_string()),
                ],
                body: Vec::new(),
                stalled_prefix: Some(b"10\r\ndata: ".to_vec()),
                stalled_suffix: None,
                hold_open: Some(Arc::clone(&release)),
                stalled_body_started: Some(Arc::clone(&started)),
                stalled_response_entered: None,
            },
            release,
            started,
        )
    }

    fn redirect_action(location: String) -> ResponseAction {
        ResponseAction {
            status_line: "302 Found",
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Location".to_string(), location),
            ],
            body: br#"{"error":"redirect"}"#.to_vec(),
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }
    }

    fn start_server(actions: Vec<ResponseAction>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let requests_clone = Arc::clone(&requests);
        let shutdown_clone = Arc::clone(&shutdown);
        let join = thread::spawn(move || {
            let mut actions = actions.into_iter();
            loop {
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
                stream
                    .set_nonblocking(false)
                    .expect("accepted test stream must switch to blocking mode");
                if shutdown_clone.load(Ordering::Relaxed) {
                    return;
                }
                let Some(action) = actions.next() else {
                    if shutdown_clone.load(Ordering::Relaxed) {
                        return;
                    }
                    panic!("unexpected test server request");
                };
                let request = read_request(&mut stream);
                requests_clone.lock().unwrap().push(request);
                write_response(&mut stream, &action);
                if let Some(release) = action.hold_open {
                    while !release.load(Ordering::Relaxed)
                        && !shutdown_clone.load(Ordering::Relaxed)
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                    if !shutdown_clone.load(Ordering::Relaxed) {
                        if let Some(stalled_suffix) = &action.stalled_suffix {
                            stream.write_all(stalled_suffix).unwrap();
                            stream.flush().unwrap();
                        }
                    }
                }
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

    fn write_response(stream: &mut TcpStream, action: &ResponseAction) {
        if action.stalled_prefix.is_some() {
            let mut response = format!("HTTP/1.1 {}\r\n", action.status_line);
            for (name, value) in &action.headers {
                response.push_str(name);
                response.push_str(": ");
                response.push_str(value);
                response.push_str("\r\n");
            }
            response.push_str("\r\n");
            stream.write_all(response.as_bytes()).unwrap();
            stream
                .write_all(
                    action
                        .stalled_prefix
                        .as_deref()
                        .expect("stalled response must define a prefix"),
                )
                .unwrap();
            stream.flush().unwrap();
            if let Some(started) = &action.stalled_body_started {
                started.store(true, Ordering::Relaxed);
            }
            if let Some(entered) = action.stalled_response_entered.as_ref() {
                let _ = entered.send(());
            }
            return;
        }
        let mut response = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            action.status_line,
            action.body.len()
        );
        for (name, value) in &action.headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        stream.write_all(response.as_bytes()).unwrap();
        write_body_allowing_expected_disconnect(stream, &action.body);
    }

    fn write_body_allowing_expected_disconnect(stream: &mut TcpStream, body: &[u8]) {
        match stream.write_all(body) {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::NotConnected
                ) =>
            {
                return;
            }
            Err(err) => panic!("test server body write failed: {err}"),
        }
        match stream.flush() {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::NotConnected
                ) => {}
            Err(err) => panic!("test server body flush failed: {err}"),
        }
    }

    fn temp_root(label: &str) -> String {
        crate::test_support::temp_root_path("model-provider-execution", label)
            .to_string_lossy()
            .to_string()
    }

    fn local_text_offer(base_url: &str) -> ConfiguredOffer {
        ConfiguredOffer {
            id: "local-text".to_string(),
            title: "Local Text".to_string(),
            operation: "text.generate".to_string(),
            input_modalities: vec!["text/plain".to_string()],
            output_modalities: vec!["text/plain".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 4,
                input_bytes_limit: 8 * 1024,
                inline_output_bytes_limit: 8 * 1024,
                event_bytes_limit: 8 * 1024,
                runtime_ms_limit: 30_000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 20,
            },
            adapter: AdapterConfig::OpenAiCompatibleText {
                api_url: format!("{base_url}/chat"),
                api_key: Some("sentinel-openai-key".to_string()),
                model: "sentinel-model".to_string(),
            },
            enabled: true,
        }
    }

    fn artifact_offer(base_url: &str) -> ConfiguredOffer {
        ConfiguredOffer {
            id: "artifact-job".to_string(),
            title: "Artifact Job".to_string(),
            operation: "image.generate".to_string(),
            input_modalities: vec!["application/json".to_string()],
            output_modalities: vec!["application/json".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 4,
                input_bytes_limit: 8 * 1024,
                inline_output_bytes_limit: 8 * 1024,
                event_bytes_limit: 8 * 1024,
                runtime_ms_limit: 30_000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 20,
            },
            adapter: AdapterConfig::HttpJobArtifact {
                create_url: format!("{base_url}/create"),
                status_url: format!("{base_url}/status"),
                cancel_url: Some(format!("{base_url}/cancel")),
                bearer_token: Some("sentinel-artifact-token".to_string()),
                poll_interval_ms: 25,
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

    fn artifact_input(prompt: &str) -> Value {
        json!({
            "schema": "elastos.model.input.image/v1",
            "prompt": prompt,
        })
    }

    fn create_binding(
        request_id: &str,
        offer: &ConfiguredOffer,
        input: &Value,
    ) -> RuntimeCreateBinding {
        RuntimeCreateBinding {
            schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            capsule_id: "assistant".to_string(),
            grant_id: "grant:test".to_string(),
            request_id: request_id.to_string(),
            offer_id: offer.id.clone(),
            operation: offer.operation.clone(),
            input_hash: model_input_hash(input).unwrap(),
        }
    }

    fn access_binding(binding: &RuntimeCreateBinding) -> RuntimeAccessBinding {
        let run_id = deterministic_run_id(binding);
        RuntimeAccessBinding {
            schema: RUNTIME_ACCESS_BINDING_SCHEMA.to_string(),
            principal_id: binding.principal_id.clone(),
            session_id: binding.session_id.clone(),
            capsule_id: binding.capsule_id.clone(),
            grant_id: binding.grant_id.clone(),
            request_id: binding.request_id.clone(),
            run_id,
        }
    }

    fn init_request(root: &str, offers: Vec<ConfiguredOffer>) -> ProviderEnvelope {
        ProviderEnvelope {
            operation: ProviderOperation::Init,
            value: json!({
                "op": "init",
                "config": {
                    "base_path": root,
                    "extra": {
                        "provider_id": "model-provider",
                        "offers": offers,
                    }
                }
            }),
        }
    }

    fn send_request(
        provider: &ProviderCoordinatorHandle,
        operation: ProviderOperation,
        value: Value,
    ) -> Value {
        provider
            .request(ProviderEnvelope { operation, value })
            .unwrap()
    }

    fn spawn_request(
        provider: &ProviderCoordinatorHandle,
        envelope: ProviderEnvelope,
    ) -> std_mpsc::Receiver<anyhow::Result<Value>> {
        let requests = provider.requests.clone();
        let (result_tx, result_rx) = std_mpsc::channel();
        thread::spawn(move || {
            let (respond_to, response_rx) = oneshot::channel();
            let result = requests
                .blocking_send(CoordinatorCommand::Request {
                    envelope,
                    respond_to,
                })
                .map_err(|_| anyhow::anyhow!("provider coordinator is unavailable"))
                .and_then(|_| {
                    response_rx
                        .blocking_recv()
                        .map_err(|_| anyhow::anyhow!("provider coordinator response was dropped"))
                });
            let _ = result_tx.send(result);
        });
        result_rx
    }

    fn init_provider(
        provider: &ProviderCoordinatorHandle,
        root: &str,
        offers: Vec<ConfiguredOffer>,
    ) {
        let response = provider.request(init_request(root, offers)).unwrap();
        assert_eq!(response["status"], "ok");
    }

    fn create_run(
        provider: &ProviderCoordinatorHandle,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        input: &Value,
    ) -> Value {
        send_request(
            provider,
            ProviderOperation::RunsCreate,
            json!({
                "op": "runs_create",
                "offer_id": offer.id,
                "operation": offer.operation,
                "input": input,
                "runtime_binding": binding,
            }),
        )
    }

    fn get_run(
        provider: &ProviderCoordinatorHandle,
        run_id: &str,
        binding: &RuntimeAccessBinding,
    ) -> Value {
        send_request(
            provider,
            ProviderOperation::RunsGet,
            json!({
                "op": "runs_get",
                "run_id": run_id,
                "runtime_binding": binding,
            }),
        )
    }

    fn events_page(
        provider: &ProviderCoordinatorHandle,
        run_id: &str,
        binding: &RuntimeAccessBinding,
        after_sequence: u64,
    ) -> Value {
        send_request(
            provider,
            ProviderOperation::RunsEvents,
            json!({
                "op": "runs_events",
                "run_id": run_id,
                "after_sequence": after_sequence,
                "runtime_binding": binding,
            }),
        )
    }

    fn cancel_run(
        provider: &ProviderCoordinatorHandle,
        run_id: &str,
        binding: &RuntimeAccessBinding,
    ) -> Value {
        send_request(
            provider,
            ProviderOperation::RunsCancel,
            json!({
                "op": "runs_cancel",
                "run_id": run_id,
                "runtime_binding": binding,
            }),
        )
    }

    fn shutdown_request(provider: &ProviderCoordinatorHandle) -> Value {
        send_request(
            provider,
            ProviderOperation::Shutdown,
            json!({
                "op": "shutdown",
            }),
        )
    }

    fn wait_for_terminal(
        provider: &ProviderCoordinatorHandle,
        run_id: &str,
        binding: &RuntimeAccessBinding,
    ) -> Value {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let response = get_run(provider, run_id, binding);
            let status = response["data"]["status"].as_str().unwrap();
            if matches!(
                status,
                "completed" | "failed" | "cancelled" | "settlement_unknown"
            ) {
                return response;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for terminal run state; last response: {response}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn load_run(root: &str, run_id: &str) -> StoredRun {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let journal = match RunJournal::open(crate::config::journal_root(root, None).unwrap()) {
                Ok(journal) => journal,
                Err(err)
                    if {
                        let detail = format!("{err:?}");
                        detail.contains("filename mismatch") && detail.contains(".json.tmp-")
                    } =>
                {
                    assert!(
                        Instant::now() < deadline,
                        "timed out loading run {run_id}; journal open never settled: {err:?}"
                    );
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(err) => panic!("failed to open run journal for {run_id}: {err:?}"),
            };
            match journal.load_run(run_id) {
                Ok(run) => return run,
                Err(err)
                    if {
                        let detail = format!("{err:?}");
                        detail.contains("filename mismatch") && detail.contains(".json.tmp-")
                    } =>
                {
                    assert!(
                        Instant::now() < deadline,
                        "timed out loading run {run_id}; journal temp file never settled: {err:?}"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("failed to load run {run_id}: {err:?}"),
            }
        }
    }

    fn seed_active_artifact_run(
        root: &str,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        job_id: &str,
    ) -> String {
        seed_active_artifact_run_with_next_poll(root, offer, binding, job_id, 0)
    }

    fn seed_active_artifact_run_with_next_poll(
        root: &str,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        job_id: &str,
        next_poll_at_ms: u64,
    ) -> String {
        let run_id = deterministic_run_id(binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer.summary(),
            offer.execution_binding_hash().unwrap(),
            crate::journal::now_ms(),
        );
        run.status = crate::contract::RunStatus::Running;
        run.backend_state = Some(json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "phase": "active",
            "job_id": job_id,
            "next_poll_at_ms": next_poll_at_ms,
            "cancel_requested": false,
            "cancel_sent": false,
            "cancel_deadline_ms": null,
        }));
        run.events = vec![
            RunEvent {
                schema: RUN_EVENT_SCHEMA.to_string(),
                sequence: 1,
                kind: "prepared".to_string(),
                data: json!({
                    "offer_id": offer.id,
                    "operation": offer.operation,
                }),
                terminal: false,
            },
            RunEvent {
                schema: RUN_EVENT_SCHEMA.to_string(),
                sequence: 2,
                kind: "dispatched".to_string(),
                data: json!({ "offer_id": offer.id }),
                terminal: false,
            },
        ];
        run.next_sequence = 3;
        let journal = RunJournal::open(crate::config::journal_root(root, None).unwrap()).unwrap();
        journal.store_run(&run).unwrap();
        run_id
    }

    fn seed_reserved_artifact_run(
        root: &str,
        offer: &ConfiguredOffer,
        binding: &RuntimeCreateBinding,
        job_id: &str,
        cancel_sent: bool,
        deadline_ms: u64,
    ) -> String {
        let run_id = seed_active_artifact_run(root, offer, binding, job_id);
        let journal = RunJournal::open(crate::config::journal_root(root, None).unwrap()).unwrap();
        let mut run = journal.load_run(&run_id).unwrap();
        run.status = crate::contract::RunStatus::Reconciling;
        run.backend_state = Some(json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "phase": "active",
            "job_id": job_id,
            "next_poll_at_ms": 0,
            "cancel_requested": true,
            "cancel_sent": cancel_sent,
            "cancel_deadline_ms": deadline_ms,
        }));
        run.updated_at_ms = crate::journal::now_ms();
        journal.store_run(&run).unwrap();
        run_id
    }

    fn artifact_offer_with_cancel_timeout(
        base_url: &str,
        cancel_settlement_timeout_ms: u64,
    ) -> ConfiguredOffer {
        let mut offer = artifact_offer(base_url);
        offer.policy.cancel_settlement_timeout_ms = cancel_settlement_timeout_ms;
        offer
    }

    fn artifact_offer_with_poll_and_cancel_timeout(
        base_url: &str,
        poll_interval_ms: u64,
        cancel_settlement_timeout_ms: u64,
    ) -> ConfiguredOffer {
        let mut offer = artifact_offer(base_url);
        if let AdapterConfig::HttpJobArtifact {
            poll_interval_ms: current_poll_interval_ms,
            ..
        } = &mut offer.adapter
        {
            *current_poll_interval_ms = poll_interval_ms;
        }
        offer.policy.cancel_settlement_timeout_ms = cancel_settlement_timeout_ms;
        offer
    }

    fn wait_for_flag(flag: &Arc<AtomicBool>) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        while !flag.load(Ordering::Relaxed) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for stalled body signal"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_one_shot(receiver: &std_mpsc::Receiver<()>, description: &str) {
        receiver
            .recv_timeout(FIXTURE_EVENT_TIMEOUT)
            .unwrap_or_else(|_| panic!("timed out waiting for {description}"));
    }

    fn wait_for_request_count(server: &TestServer, expected: usize) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            if server.requests.lock().unwrap().len() == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {expected} test server request(s)"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_run_state(
        root: &str,
        run_id: &str,
        description: &str,
        predicate: impl Fn(&StoredRun) -> bool,
    ) -> StoredRun {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let journal = match RunJournal::open(crate::config::journal_root(root, None).unwrap()) {
                Ok(journal) => journal,
                Err(err)
                    if {
                        let detail = format!("{err:?}");
                        detail.contains("filename mismatch") && detail.contains(".json.tmp-")
                    } =>
                {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for {description}; journal open never settled: {err:?}"
                    );
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(err) => panic!("timed out waiting for {description}; open failed: {err:?}"),
            };
            let run = match journal.load_run(run_id) {
                Ok(run) => run,
                Err(err)
                    if {
                        let detail = format!("{err:?}");
                        detail.contains("filename mismatch") && detail.contains(".json.tmp-")
                    } =>
                {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for {description}; journal temp file never settled: {err:?}"
                    );
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(err) => panic!("timed out waiting for {description}; load failed: {err:?}"),
            };
            if predicate(&run) {
                return run;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}; last run: {:?}",
                run
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn json_action(value: Value) -> ResponseAction {
        ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(&value).unwrap(),
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }
    }

    fn stalled_json_action() -> (ResponseAction, Arc<AtomicBool>, Arc<AtomicBool>) {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let body = br#"{"job_id":"job-123"}"#.to_vec();
        let prefix_len = 11;
        let prefix = body[..prefix_len].to_vec();
        let suffix = body[prefix_len..].to_vec();
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "keep-alive".to_string()),
                ],
                body,
                stalled_prefix: Some(
                    [
                        format!("{prefix_len:X}\r\n").into_bytes(),
                        prefix,
                        b"\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                stalled_suffix: Some(
                    [
                        format!("{:X}\r\n", suffix.len()).into_bytes(),
                        suffix,
                        b"\r\n0\r\n\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                hold_open: Some(Arc::clone(&release)),
                stalled_body_started: Some(Arc::clone(&started)),
                stalled_response_entered: None,
            },
            release,
            started,
        )
    }

    fn stalled_status_json_action() -> (ResponseAction, Arc<AtomicBool>, Arc<AtomicBool>) {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let body = br#"{"state":"running"}"#.to_vec();
        let prefix_len = 10;
        let prefix = body[..prefix_len].to_vec();
        let suffix = body[prefix_len..].to_vec();
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "keep-alive".to_string()),
                ],
                body,
                stalled_prefix: Some(
                    [
                        format!("{prefix_len:X}\r\n").into_bytes(),
                        prefix,
                        b"\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                stalled_suffix: Some(
                    [
                        format!("{:X}\r\n", suffix.len()).into_bytes(),
                        suffix,
                        b"\r\n0\r\n\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                hold_open: Some(Arc::clone(&release)),
                stalled_body_started: Some(Arc::clone(&started)),
                stalled_response_entered: None,
            },
            release,
            started,
        )
    }

    fn stalled_cancel_json_action() -> (ResponseAction, Arc<AtomicBool>, std_mpsc::Receiver<()>) {
        let release = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = std_mpsc::channel();
        let body = br#"{}"#.to_vec();
        let prefix_len = 1;
        let prefix = body[..prefix_len].to_vec();
        let suffix = body[prefix_len..].to_vec();
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "keep-alive".to_string()),
                ],
                body,
                stalled_prefix: Some(
                    [
                        format!("{prefix_len:X}\r\n").into_bytes(),
                        prefix,
                        b"\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                stalled_suffix: Some(
                    [
                        format!("{:X}\r\n", suffix.len()).into_bytes(),
                        suffix,
                        b"\r\n0\r\n\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                hold_open: Some(Arc::clone(&release)),
                stalled_body_started: None,
                stalled_response_entered: Some(entered_tx),
            },
            release,
            entered_rx,
        )
    }

    fn truncated_cancel_json_action() -> (ResponseAction, Arc<AtomicBool>) {
        let started = Arc::new(AtomicBool::new(false));
        let body = br#"{}"#.to_vec();
        let prefix_len = 1;
        let prefix = body[..prefix_len].to_vec();
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "close".to_string()),
                ],
                body,
                stalled_prefix: Some(
                    [
                        format!("{prefix_len:X}\r\n").into_bytes(),
                        prefix,
                        b"\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                stalled_suffix: None,
                hold_open: None,
                stalled_body_started: Some(Arc::clone(&started)),
                stalled_response_entered: None,
            },
            started,
        )
    }

    fn truncated_status_json_action() -> (ResponseAction, Arc<AtomicBool>) {
        let started = Arc::new(AtomicBool::new(false));
        let body = br#"{"state":"running"}"#.to_vec();
        let prefix_len = 10;
        let prefix = body[..prefix_len].to_vec();
        (
            ResponseAction {
                status_line: "200 OK",
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Transfer-Encoding".to_string(), "chunked".to_string()),
                    ("Connection".to_string(), "close".to_string()),
                ],
                body,
                stalled_prefix: Some(
                    [
                        format!("{prefix_len:X}\r\n").into_bytes(),
                        prefix,
                        b"\r\n".to_vec(),
                    ]
                    .concat(),
                ),
                stalled_suffix: None,
                hold_open: None,
                stalled_body_started: Some(Arc::clone(&started)),
                stalled_response_entered: None,
            },
            started,
        )
    }

    fn truncated_create_json_action() -> ResponseAction {
        let body = br#"{"job_id":"job-123"}"#.to_vec();
        let prefix_len = 11;
        let prefix = body[..prefix_len].to_vec();
        ResponseAction {
            status_line: "200 OK",
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Transfer-Encoding".to_string(), "chunked".to_string()),
                ("Connection".to_string(), "close".to_string()),
            ],
            body,
            stalled_prefix: Some(
                [
                    format!("{prefix_len:X}\r\n").into_bytes(),
                    prefix,
                    b"\r\n".to_vec(),
                ]
                .concat(),
            ),
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }
    }

    fn oversized_json_action(bytes: usize) -> ResponseAction {
        ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: vec![b'x'; bytes],
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }
    }

    fn assert_no_private_leakage(serialized: &[String], disallowed: &[&str]) {
        for value in serialized {
            for marker in disallowed {
                assert!(
                    !value.contains(marker),
                    "public response leaked private marker {marker:?}: {value}"
                );
            }
        }
    }

    fn wait_for_retry_request(
        provider: &ProviderCoordinatorHandle,
        server: &TestServer,
        run_id: &str,
        binding: &RuntimeAccessBinding,
        expected_request_count: usize,
    ) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        while server.requests.lock().unwrap().len() < expected_request_count {
            let _ = get_run(provider, run_id, binding);
            assert!(
                Instant::now() < deadline,
                "timed out waiting for retry request {expected_request_count}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_provider_exit(provider: &mut ProviderCoordinatorHandle) {
        if let Some(join) = provider.join.take() {
            let _ = join.join();
        }
    }

    #[test]
    fn stalled_text_run_returns_promptly_and_other_requests_continue() {
        let seed = sse_action(
            &[json!({"choices":[{"delta":{"content":"seed"}}]}).to_string()],
            true,
        );
        let (stalled, release, stalled_started) = stalled_sse_action();
        let server = start_server(vec![seed, stalled]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("prompt");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let seed_input = text_input("seed");
        let seed_binding = create_binding("request:seed", &offer, &seed_input);
        let seed_run = create_run(&provider, &offer, &seed_binding, &seed_input);
        let seed_run_id = seed_run["data"]["run_id"].as_str().unwrap().to_string();
        let seed_terminal =
            wait_for_terminal(&provider, &seed_run_id, &access_binding(&seed_binding));
        assert_eq!(seed_terminal["data"]["status"], "completed");

        let stalled_input = text_input("stall");
        let stalled_binding = create_binding("request:stall", &offer, &stalled_input);
        let started = Instant::now();
        let stalled_run = create_run(&provider, &offer, &stalled_binding, &stalled_input);
        assert!(
            started.elapsed() < PROMPT_RETURN_BOUND,
            "stalled create must return promptly, took {:?}",
            started.elapsed()
        );
        let stalled_run_id = stalled_run["data"]["run_id"].as_str().unwrap().to_string();
        assert_eq!(stalled_run["data"]["status"], "running");
        wait_for_flag(&stalled_started);
        assert_eq!(
            load_run(&root, &stalled_run_id).status,
            crate::contract::RunStatus::Running
        );

        let offers = send_request(
            &provider,
            ProviderOperation::OffersList,
            json!({"op":"offers_list"}),
        );
        assert_eq!(offers["status"], "ok");
        assert_eq!(offers["data"]["offers"].as_array().unwrap().len(), 1);

        let other_run = get_run(&provider, &seed_run_id, &access_binding(&seed_binding));
        assert_eq!(other_run["data"]["status"], "completed");

        release.store(true, Ordering::Relaxed);
        provider.shutdown_on_eof();
    }

    #[test]
    fn stalled_artifact_create_returns_promptly_and_other_requests_continue() {
        let (stalled, release, stalled_started) = stalled_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 5_000);
        let root = temp_root("artifact-create-prompt");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("stall");
        let binding = create_binding("request:artifact-create-prompt", &offer, &input);
        let started = Instant::now();
        let create_response = create_run(&provider, &offer, &binding, &input);
        assert!(
            started.elapsed() < PROMPT_RETURN_BOUND,
            "stalled artifact create must return promptly, took {:?}",
            started.elapsed()
        );
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(create_response["data"]["status"], "running");
        wait_for_flag(&stalled_started);
        let stored = load_run(&root, &run_id);
        assert_eq!(stored.status, crate::contract::RunStatus::Running);
        assert_eq!(stored.events.len(), 1);
        assert_eq!(stored.events[0].kind, "prepared");

        let offers = send_request(
            &provider,
            ProviderOperation::OffersList,
            json!({"op":"offers_list"}),
        );
        assert_eq!(offers["status"], "ok");
        assert_eq!(offers["data"]["offers"].as_array().unwrap().len(), 1);

        let same_run = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(same_run["data"]["status"], "running");

        release.store(true, Ordering::Relaxed);
        provider.shutdown_on_eof();
    }

    #[test]
    fn stalled_artifact_status_does_not_block_unrelated_status_request() {
        let (stalled, release, stalled_started) = stalled_status_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-prompt");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-prompt", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let stalled_get = spawn_request(
            &provider,
            ProviderEnvelope {
                operation: ProviderOperation::RunsGet,
                value: json!({
                    "op": "runs_get",
                    "run_id": run_id,
                    "runtime_binding": access_binding(&binding),
                }),
            },
        );
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        let status_rx = spawn_request(
            &provider,
            ProviderEnvelope {
                operation: ProviderOperation::Status,
                value: json!({"op":"status"}),
            },
        );
        let status_response = status_rx
            .recv_timeout(PROMPT_RETURN_BOUND)
            .unwrap_or_else(|_| panic!("unrelated status request must return promptly"));
        assert!(
            started.elapsed() < PROMPT_RETURN_BOUND,
            "unrelated status request must return promptly, took {:?}",
            started.elapsed()
        );
        let status_response = status_response.unwrap();
        assert_eq!(status_response["status"], "ok");
        assert_eq!(
            status_response["data"]["provider"],
            crate::contract::PROVIDER_ID
        );

        release.store(true, Ordering::Relaxed);
        let stalled_response = stalled_get
            .recv_timeout(WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("stalled get must finish after release"))
            .unwrap();
        assert_eq!(stalled_response["status"], "ok");

        provider.shutdown_on_eof();
    }

    #[test]
    fn duplicate_artifact_status_polls_create_one_request_and_one_worker() {
        let (stalled, release, stalled_started) = stalled_status_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-duplicate");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-duplicate", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let first = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(first["data"]["status"], "running");
        wait_for_flag(&stalled_started);

        let second = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(second["data"]["status"], "running");
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        release.store(true, Ordering::Relaxed);
        let run = wait_for_run_state(&root, &run_id, "artifact status poll durability", |run| {
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("next_poll_at_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        });
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(run.events.len(), 2);
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        provider.shutdown_on_eof();
    }

    #[test]
    fn eof_with_stalled_artifact_status_preserves_active_run_promptly() {
        let (stalled, release, stalled_started) = stalled_status_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-eof");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-eof", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "running");
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        provider.shutdown_on_eof();
        assert!(
            started.elapsed() < SHUTDOWN_RETURN_BOUND,
            "artifact status EOF shutdown must return promptly, took {:?}",
            started.elapsed()
        );

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("job_id"))
                .and_then(Value::as_str),
            Some("job-123")
        );

        release.store(true, Ordering::Relaxed);
        drop(server);
    }

    #[test]
    fn shutdown_request_with_stalled_artifact_status_preserves_active_run_promptly() {
        let (stalled, release, stalled_started) = stalled_status_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-shutdown");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-shutdown", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "running");
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        let shutdown = send_request(
            &provider,
            ProviderOperation::Shutdown,
            json!({"op":"shutdown"}),
        );
        assert!(
            started.elapsed() < SHUTDOWN_RETURN_BOUND,
            "artifact status shutdown must return promptly, took {:?}",
            started.elapsed()
        );
        assert_eq!(shutdown["status"], "ok");
        assert_eq!(shutdown["data"]["shutdown"], json!(true));

        provider.shutdown_on_eof();
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("job_id"))
                .and_then(Value::as_str),
            Some("job-123")
        );

        release.store(true, Ordering::Relaxed);
        drop(server);
    }

    #[test]
    fn artifact_status_terminal_output_is_durable_and_exact() {
        let server = start_server(vec![json_action(json!({
            "state": "completed",
            "output": {
                "schema": "elastos.model.output.object/v1",
                "uri": "elastos://object/object:artifact-output"
            }
        }))]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-terminal");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-terminal", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");

        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        assert_eq!(terminal["data"]["status"], "completed");
        assert_eq!(
            terminal["data"]["terminal"]["output"],
            json!({
                "schema": "elastos.model.output.object/v1",
                "uri": "elastos://object/object:artifact-output"
            })
        );
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Completed);
        assert_eq!(run.events.last().unwrap().kind, "output");

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_progress_appends_one_canonical_progress_event() {
        let server = start_server(vec![json_action(json!({
            "state": "running",
            "progress": {
                "phase": "rendering",
                "completed": 2,
                "total": 5,
            }
        }))]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-progress");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-progress", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "running");
        let run = wait_for_run_state(&root, &run_id, "artifact status progress event", |run| {
            run.events.len() == 3
        });
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(run.events.last().unwrap().kind, "progress");
        assert_eq!(
            run.events.last().unwrap().data,
            json!({
                "phase": "rendering",
                "completed": 2,
                "total": 5,
            })
        );

        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let events = page["data"]["events"].as_array().unwrap();
        let progress = events
            .iter()
            .filter(|event| event["kind"] == "progress")
            .collect::<Vec<_>>();
        assert_eq!(progress.len(), 1);
        assert_eq!(
            progress[0]["data"],
            json!({
                "phase": "rendering",
                "completed": 2,
                "total": 5,
            })
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_malformed_progress_preserves_active_state_and_private_details_stay_hidden() {
        let server = start_server(vec![
            json_action(json!({
                "state": "running",
                "progress": {
                    "phase": "rendering",
                    "completed": 6,
                    "total": 5,
                }
            })),
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-bad-progress");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-bad-progress", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(run.events.len(), 2);

        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(&public, &["job-123", "http://", "invalid progress counts"]);

        provider.shutdown_on_eof();
    }

    #[test]
    fn stale_status_update_cannot_overwrite_cancel_reservation() {
        let (stalled, release, stalled_started) = stalled_status_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-cancel");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-cancel", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        wait_for_flag(&stalled_started);

        let journal = RunJournal::open(crate::config::journal_root(&root, None).unwrap()).unwrap();
        let mut reserved = journal.load_run(&run_id).unwrap();
        reserved.status = crate::contract::RunStatus::Reconciling;
        let backend_state = reserved
            .backend_state
            .as_mut()
            .and_then(Value::as_object_mut)
            .expect("active artifact backend state must exist");
        backend_state.insert("cancel_requested".to_string(), Value::Bool(true));
        backend_state.insert("cancel_sent".to_string(), Value::Bool(false));
        backend_state.insert(
            "cancel_deadline_ms".to_string(),
            json!(crate::journal::now_ms().saturating_add(1_000)),
        );
        reserved.updated_at_ms = crate::journal::now_ms();
        journal.store_run(&reserved).unwrap();
        release.store(true, Ordering::Relaxed);

        let run = wait_for_run_state(
            &root,
            &run_id,
            "cancel reservation survives stale status poll",
            |run| run.status == crate::contract::RunStatus::Reconciling,
        );
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("cancel_requested"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        provider.shutdown_on_eof();
    }

    #[test]
    fn restart_of_active_artifact_status_polls_existing_job_id_without_create() {
        let server = start_server(vec![json_action(json!({"state": "running"}))]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-restart");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-restart", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "running");
        wait_for_request_count(&server, 1);
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /status?job_id=job-123 "));
        assert!(!requests[0].contains("/create"));

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_redirect_preserves_active_state_and_private_location_stays_hidden() {
        let redirect_target = "http://127.0.0.1:9/private-status";
        let server = start_server(vec![redirect_action(redirect_target.to_string())]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-redirect");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-redirect", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        let spawned_next_poll = load_run(&root, &run_id)
            .backend_state
            .as_ref()
            .and_then(|state| state.get("next_poll_at_ms"))
            .and_then(Value::as_u64)
            .unwrap();
        let run = wait_for_run_state(&root, &run_id, "redirect-preserved artifact state", |run| {
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("next_poll_at_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > spawned_next_poll
        });
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(&public, &["private-status", "127.0.0.1:9", "Location"]);

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_truncated_body_preserves_active_state_and_private_details_stay_hidden() {
        let (truncated, started) = truncated_status_json_action();
        let server = start_server(vec![
            truncated,
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-truncated");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-truncated", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        wait_for_flag(&started);
        wait_for_request_count(&server, 1);
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(run.events.len(), 2);

        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &[
                "job-123",
                "http://",
                "backend request timed out",
                "error decoding response body",
            ],
        );
        assert!(!server.requests.lock().unwrap().is_empty());

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_timeout_preserves_active_state_and_private_details_stay_hidden() {
        let (stalled, release, started) = stalled_status_json_action();
        let server = start_server(vec![
            stalled,
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-timeout");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-timeout", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        wait_for_flag(&started);
        thread::sleep(Duration::from_millis(700));
        release.store(true, Ordering::Relaxed);
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("job_id"))
                .and_then(Value::as_str),
            Some("job-123")
        );

        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &[
                "job-123",
                "http://",
                "backend request timed out",
                "timed out",
                "error decoding response body",
            ],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_status_oversized_body_preserves_active_state_and_private_details_stay_hidden() {
        let server = start_server(vec![
            oversized_json_action(5 * 1024 * 1024),
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-status-oversized");
        let input = artifact_input("status");
        let binding = create_binding("request:artifact-status-oversized", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let initial = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(initial["data"]["status"], "running");
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(run.events.len(), 2);

        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &["job-123", "http://", "response exceeds provider limit"],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn stalled_artifact_cancel_does_not_block_get_or_events() {
        let (stalled, release, started_flag) = stalled_cancel_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-nonblocking");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-nonblocking", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let started = Instant::now();
        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        let cancel_elapsed = started.elapsed();
        assert_eq!(cancel_response["data"]["status"], "reconciling");
        assert!(
            cancel_elapsed < PROMPT_RETURN_BOUND,
            "cancel must return promptly, took {:?}",
            cancel_elapsed
        );
        wait_for_one_shot(&started_flag, "stalled cancel response entry");
        wait_for_request_count(&server, 1);

        let get_started = Instant::now();
        let run_response = get_run(&provider, &run_id, &access_binding(&binding));
        let get_elapsed = get_started.elapsed();
        assert_eq!(run_response["data"]["status"], "reconciling");
        assert!(
            get_elapsed < PROMPT_RETURN_BOUND,
            "runs_get must remain prompt during stalled cancel, took {:?}",
            get_elapsed
        );

        let events_started = Instant::now();
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let events_elapsed = events_started.elapsed();
        assert_eq!(page["data"]["run_id"], json!(run_id));
        assert!(
            events_elapsed < PROMPT_RETURN_BOUND,
            "runs_events must remain prompt during stalled cancel, took {:?}",
            events_elapsed
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        release.store(true, Ordering::Relaxed);
        provider.shutdown_on_eof();
    }

    #[test]
    fn duplicate_artifact_cancel_sends_one_post_for_durable_reservation() {
        let (stalled, release, started_flag) = stalled_cancel_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-duplicate");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-duplicate", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let first = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(first["data"]["status"], "reconciling");
        wait_for_one_shot(&started_flag, "stalled cancel response entry");
        wait_for_request_count(&server, 1);

        let second = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(second["data"]["status"], "reconciling");
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_requested"],
            json!(true)
        );
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_sent"],
            json!(true)
        );

        release.store(true, Ordering::Relaxed);
        provider.shutdown_on_eof();
    }

    #[test]
    fn restart_of_reserved_artifact_cancel_does_not_resend_post() {
        let mut server = start_server(vec![json_action(json!({"state":"running"}))]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-restart");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-restart", &offer, &input);
        let run_id = seed_reserved_artifact_run(
            &root,
            &offer,
            &binding,
            "job-123",
            true,
            crate::journal::now_ms().saturating_add(1_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "reconciling");
        wait_for_request_count(&server, 1);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("GET /status?job_id=job-123 HTTP/1.1\r\n"));
        assert!(!request.contains("/cancel"));
        assert!(!request.contains("/create"));
        drop(requests);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_requested"],
            json!(true)
        );
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_sent"],
            json!(true)
        );
        assert_eq!(run.events.last().unwrap().kind, "dispatched");

        provider.shutdown_on_eof();
        server.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(server.base_url.strip_prefix("http://").unwrap_or(""));
        server
            .join
            .take()
            .expect("test server join handle must exist")
            .join()
            .expect("test server must not panic");
    }

    #[test]
    fn current_reserved_artifact_status_can_settle_cancelled() {
        let server = start_server(vec![
            json_action(json!({})),
            json_action(json!({"state":"cancelled"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-settled");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-settled", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(cancel_response["data"]["status"], "reconciling");

        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        assert_eq!(terminal["data"]["status"], "cancelled");
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Cancelled);
        assert_eq!(run.events.last().unwrap().kind, "cancelled");

        provider.shutdown_on_eof();
    }

    #[test]
    fn cancel_attempt_with_short_remaining_deadline_still_allows_one_status_proof() {
        let server = start_server(vec![
            json_action(json!({})),
            json_action(json!({"state":"cancelled"})),
        ]);
        let offer = artifact_offer_with_poll_and_cancel_timeout(&server.base_url, 60_000, 5_000);
        let root = temp_root("artifact-cancel-short-deadline");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-short-deadline", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(cancel_response["data"]["status"], "reconciling");

        let after_cancel = load_run(&root, &run_id);
        let deadline = after_cancel
            .backend_state
            .as_ref()
            .and_then(|state| state.get("cancel_deadline_ms"))
            .and_then(Value::as_u64)
            .unwrap();
        let next_poll = after_cancel
            .backend_state
            .as_ref()
            .and_then(|state| state.get("next_poll_at_ms"))
            .and_then(Value::as_u64)
            .unwrap();
        assert!(
            next_poll < deadline,
            "cancel transport attempt must leave one status proof before deadline: next_poll={next_poll}, deadline={deadline}"
        );

        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        assert_eq!(terminal["data"]["status"], "cancelled");
        assert_eq!(server.requests.lock().unwrap().len(), 2);

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_cancel_redirect_preserves_reconciling_and_private_location_stays_hidden() {
        let redirect_target = "http://127.0.0.1:9/private-cancel";
        let server = start_server(vec![redirect_action(redirect_target.to_string())]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-redirect");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-redirect", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "reconciling");
        let run = wait_for_run_state(&root, &run_id, "cancel redirect preserved state", |run| {
            run.status == crate::contract::RunStatus::Reconciling
                && run
                    .backend_state
                    .as_ref()
                    .and_then(|state| state.get("cancel_requested"))
                    .and_then(Value::as_bool)
                    == Some(true)
        });
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(&public, &["private-cancel", "127.0.0.1:9", "Location"]);

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_cancel_timeout_preserves_reconciling_and_private_details_stay_hidden() {
        let (stalled, release, started_flag) = stalled_cancel_json_action();
        let server = start_server(vec![
            stalled,
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 5_000);
        let root = temp_root("artifact-cancel-timeout");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-timeout", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "reconciling");
        wait_for_one_shot(&started_flag, "stalled cancel response entry");
        thread::sleep(Duration::from_millis(700));
        release.store(true, Ordering::Relaxed);
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("POST /cancel "));
        assert!(requests[1].starts_with("GET /status?job_id=job-123 "));

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state
                .as_ref()
                .and_then(|state| state.get("cancel_requested"))
                .and_then(Value::as_bool),
            Some(true)
        );
        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &[
                "job-123",
                "http://",
                "backend request timed out",
                "timed out",
                "error decoding response body",
            ],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_cancel_oversized_body_preserves_reconciling_and_private_details_stay_hidden() {
        let server = start_server(vec![
            oversized_json_action(5 * 1024 * 1024),
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-oversized");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-oversized", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "reconciling");
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &["job-123", "http://", "response exceeds provider limit"],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_cancel_truncated_body_preserves_reconciling_and_private_details_stay_hidden() {
        let (truncated, started_flag) = truncated_cancel_json_action();
        let server = start_server(vec![
            truncated,
            json_action(json!({"state":"running"})),
            json_action(json!({"state":"running"})),
        ]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 1_000);
        let root = temp_root("artifact-cancel-truncated");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-truncated", &offer, &input);
        let run_id = seed_active_artifact_run_with_next_poll(
            &root,
            &offer,
            &binding,
            "job-123",
            crate::journal::now_ms().saturating_add(60_000),
        );
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "reconciling");
        wait_for_flag(&started_flag);
        wait_for_retry_request(&provider, &server, &run_id, &access_binding(&binding), 2);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        let public = vec![
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap(),
            serde_json::to_string(&events_page(
                &provider,
                &run_id,
                &access_binding(&binding),
                0,
            ))
            .unwrap(),
        ];
        assert_no_private_leakage(
            &public,
            &[
                "job-123",
                "http://",
                "backend request timed out",
                "error decoding response body",
            ],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn eof_with_stalled_artifact_cancel_preserves_reconciling_promptly() {
        let (stalled, _release, started_flag) = stalled_cancel_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 5_000);
        let root = temp_root("artifact-cancel-eof");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-eof", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(cancel_response["data"]["status"], "reconciling");
        wait_for_one_shot(&started_flag, "stalled cancel response entry");

        let started = Instant::now();
        provider.shutdown_on_eof();
        let elapsed = started.elapsed();
        assert!(
            elapsed < SHUTDOWN_RETURN_BOUND,
            "EOF shutdown must return promptly, took {:?}",
            elapsed
        );
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_requested"],
            json!(true)
        );
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_sent"],
            json!(true)
        );
    }

    #[test]
    fn shutdown_request_with_stalled_artifact_cancel_preserves_reconciling_promptly() {
        let (stalled, _release, started_flag) = stalled_cancel_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer_with_cancel_timeout(&server.base_url, 5_000);
        let root = temp_root("artifact-cancel-shutdown");
        let input = artifact_input("cancel");
        let binding = create_binding("request:artifact-cancel-shutdown", &offer, &input);
        let run_id = seed_active_artifact_run(&root, &offer, &binding, "job-123");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(cancel_response["data"]["status"], "reconciling");
        wait_for_one_shot(&started_flag, "stalled cancel response entry");

        let started = Instant::now();
        let response = shutdown_request(&provider);
        let elapsed = started.elapsed();
        assert_eq!(response["status"], "ok");
        assert!(
            elapsed < SHUTDOWN_RETURN_BOUND,
            "shutdown request must return promptly, took {:?}",
            elapsed
        );
        wait_for_provider_exit(&mut provider);

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Reconciling);
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_requested"],
            json!(true)
        );
        assert_eq!(
            run.backend_state.as_ref().unwrap()["cancel_sent"],
            json!(true)
        );
    }

    #[test]
    fn text_stream_deltas_are_ordered_and_terminal_output_is_exact() {
        let server = start_server(vec![sse_action(
            &[
                json!({"choices":[{"delta":{"content":"hello"}}]}).to_string(),
                json!({"choices":[{"delta":{"content":" "}}]}).to_string(),
                json!({"choices":[{"delta":{"content":"world"}}]}).to_string(),
            ],
            true,
        )]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("delta-order");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("hello world");
        let binding = create_binding("request:deltas", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        assert_eq!(terminal["data"]["status"], "completed");
        assert_eq!(
            terminal["data"]["terminal"]["output"],
            json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "hello world",
            })
        );

        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let events = page["data"]["events"].as_array().unwrap();
        let kinds = events
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["prepared", "dispatched", "text_delta", "output"]
        );
        assert_eq!(events[2]["data"], json!({"text":"hello world"}));

        provider.shutdown_on_eof();
    }

    #[test]
    fn local_cancel_returns_promptly_and_settles_cancelled() {
        let (stalled, release, stalled_started) = stalled_sse_action();
        let server = start_server(vec![stalled]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("cancel");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("cancel me");
        let binding = create_binding("request:cancel", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        let cancel_response = cancel_run(&provider, &run_id, &access_binding(&binding));
        assert!(
            started.elapsed() < PROMPT_RETURN_BOUND,
            "local cancel must return promptly, took {:?}",
            started.elapsed()
        );
        assert_eq!(cancel_response["data"]["status"], "reconciling");

        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        assert_eq!(terminal["data"]["status"], "cancelled");
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::Cancelled);
        assert_eq!(run.events.last().unwrap().kind, "cancelled");

        release.store(true, Ordering::Relaxed);
        provider.shutdown_on_eof();
    }

    #[test]
    fn shutdown_request_settles_active_text_run_unknown_promptly() {
        let (stalled, release, stalled_started) = stalled_sse_action();
        let server = start_server(vec![stalled]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("shutdown");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("shutdown");
        let binding = create_binding("request:shutdown", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        let shutdown = send_request(
            &provider,
            ProviderOperation::Shutdown,
            json!({"op":"shutdown"}),
        );
        assert!(
            started.elapsed() < SHUTDOWN_RETURN_BOUND,
            "shutdown must return promptly, took {:?}",
            started.elapsed()
        );
        assert_eq!(shutdown["status"], "ok");
        assert_eq!(shutdown["data"]["shutdown"], json!(true));

        provider.shutdown_on_eof();
        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::SettlementUnknown);

        release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn eof_settles_active_text_run_unknown_promptly() {
        let (stalled, release, stalled_started) = stalled_sse_action();
        let server = start_server(vec![stalled]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("eof");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("eof");
        let binding = create_binding("request:eof", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        provider.shutdown_on_eof();
        assert!(
            started.elapsed() < SHUTDOWN_RETURN_BOUND,
            "EOF shutdown must return promptly, took {:?}",
            started.elapsed()
        );

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::SettlementUnknown);

        release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn eof_during_stalled_artifact_create_settles_unknown_promptly() {
        let (stalled, release, stalled_started) = stalled_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-eof");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("stall");
        let binding = create_binding("request:artifact-create-eof", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_for_flag(&stalled_started);

        let started = Instant::now();
        provider.shutdown_on_eof();
        assert!(
            started.elapsed() < SHUTDOWN_RETURN_BOUND,
            "artifact create EOF shutdown must return promptly, took {:?}",
            started.elapsed()
        );

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::SettlementUnknown);
        assert_eq!(run.events.last().unwrap().kind, "settlement_unknown");

        release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn restart_of_active_local_text_settles_unknown() {
        let server = start_server(Vec::new());
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("restart");
        let input = text_input("restart");
        let binding = create_binding("request:restart", &offer, &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer.summary(),
            offer.execution_binding_hash().unwrap(),
            crate::journal::now_ms(),
        );
        run.status = crate::contract::RunStatus::Running;
        run.backend_state = Some(serialize_local_text_backend_state(false).unwrap());
        run.events = vec![
            RunEvent {
                schema: RUN_EVENT_SCHEMA.to_string(),
                sequence: 1,
                kind: "prepared".to_string(),
                data: json!({}),
                terminal: false,
            },
            RunEvent {
                schema: RUN_EVENT_SCHEMA.to_string(),
                sequence: 2,
                kind: "dispatched".to_string(),
                data: json!({"offer_id": offer.id}),
                terminal: false,
            },
        ];
        run.next_sequence = 3;
        let journal = RunJournal::open(crate::config::journal_root(&root, None).unwrap()).unwrap();
        journal.store_run(&run).unwrap();

        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);
        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "settlement_unknown");

        provider.shutdown_on_eof();
    }

    #[test]
    fn restart_of_artifact_create_without_receipt_settles_unknown_without_redispatch() {
        let server = start_server(Vec::new());
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-restart");
        let input = artifact_input("restart");
        let binding = create_binding("request:artifact-create-restart", &offer, &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer.summary(),
            offer.execution_binding_hash().unwrap(),
            crate::journal::now_ms(),
        );
        run.status = crate::contract::RunStatus::Running;
        run.backend_state = Some(json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "phase": "creating",
        }));
        run.events = vec![RunEvent {
            schema: RUN_EVENT_SCHEMA.to_string(),
            sequence: 1,
            kind: "prepared".to_string(),
            data: json!({
                "offer_id": offer.id,
                "operation": offer.operation,
            }),
            terminal: false,
        }];
        run.next_sequence = 2;
        let journal = RunJournal::open(crate::config::journal_root(&root, None).unwrap()).unwrap();
        journal.store_run(&run).unwrap();

        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);
        let response = get_run(&provider, &run_id, &access_binding(&binding));
        assert_eq!(response["data"]["status"], "settlement_unknown");
        assert_eq!(server.requests.lock().unwrap().len(), 0);

        provider.shutdown_on_eof();
    }

    #[test]
    fn duplicate_artifact_create_dispatch_creates_one_request_and_one_worker() {
        let (stalled, release, stalled_started) = stalled_json_action();
        let server = start_server(vec![stalled]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-duplicate");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("duplicate");
        let binding = create_binding("request:artifact-create-duplicate", &offer, &input);
        let first = create_run(&provider, &offer, &binding, &input);
        let second = create_run(&provider, &offer, &binding, &input);
        wait_for_flag(&stalled_started);
        assert_eq!(first["data"]["run_id"], second["data"]["run_id"]);
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        release.store(true, Ordering::Relaxed);
        let run_id = first["data"]["run_id"].as_str().unwrap().to_string();
        let run = wait_for_run_state(
            &root,
            &run_id,
            "artifact create dispatched receipt",
            |run| run.events.len() == 2,
        );
        let kinds = run
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["prepared", "dispatched"]);
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        provider.shutdown_on_eof();
    }

    #[test]
    fn http_artifact_create_worker_exit_without_receipt_settles_unknown_without_restart() {
        let server = start_server(vec![truncated_create_json_action()]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-exited");
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let handle = runtime.handle().clone();
        let (update_tx, update_rx) = mpsc::channel(8);
        let (_request_tx, request_rx) = mpsc::channel(1);
        let adapter = LiveAdapterExecutor::new(handle.clone(), update_tx.clone());
        let mut provider = crate::state::ModelProviderState::from_init(
            serde_json::from_value::<crate::config::BridgeProviderConfig>(
                init_request(&root, vec![offer.clone()]).value["config"].clone(),
            )
            .unwrap(),
            adapter,
        )
        .unwrap();

        let input = artifact_input("exit");
        let binding = create_binding("request:artifact-create-exited", &offer, &input);
        let response = provider
            .handle_runs_create(
                serde_json::from_value::<crate::contract::RunsCreateRequest>(json!({
                    "op": "runs_create",
                    "offer_id": offer.id,
                    "operation": offer.operation,
                    "input": input,
                    "runtime_binding": binding,
                }))
                .unwrap(),
            )
            .unwrap();
        let run_id = response["data"]["run_id"].as_str().unwrap().to_string();
        assert!(provider.adapters().is_current_worker_generation(&run_id, 1));

        let mut coordinator = ProviderCoordinator {
            provider: Some(provider),
            handle,
            requests: request_rx,
            updates: update_rx,
            update_tx,
        };
        runtime.block_on(async {
            let apply = tokio::time::timeout(WAIT_TIMEOUT, coordinator.updates.recv())
                .await
                .expect("timed out waiting for create worker apply update")
                .expect("create worker apply update was dropped");
            match &apply {
                WorkerUpdate::Apply {
                    run_id: update_run_id,
                    generation,
                    ..
                } => {
                    assert_eq!(update_run_id, &run_id);
                    assert_eq!(*generation, 1);
                }
                WorkerUpdate::Exited { .. } => {
                    panic!("create worker must report apply result before exit");
                }
            }
            coordinator.handle_update(apply).await;

            let exited = tokio::time::timeout(WAIT_TIMEOUT, coordinator.updates.recv())
                .await
                .expect("timed out waiting for create worker exit update")
                .expect("create worker exit update was dropped");
            match &exited {
                WorkerUpdate::Exited {
                    run_id: update_run_id,
                    generation,
                } => {
                    assert_eq!(update_run_id, &run_id);
                    assert_eq!(*generation, 1);
                }
                WorkerUpdate::Apply { .. } => {
                    panic!("create worker emitted unexpected second apply update");
                }
            }
            coordinator.handle_update(exited).await;
        });

        let run = load_run(&root, &run_id);
        assert_eq!(run.status, crate::contract::RunStatus::SettlementUnknown);
        assert_eq!(
            run.events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["prepared", "settlement_unknown"]
        );
    }

    #[test]
    fn artifact_create_receipt_is_durable_before_worker_exit_and_private_fields_do_not_leak() {
        let server = start_server(vec![json_action(json!({"job_id":"job-123"}))]);
        let offer = artifact_offer_with_poll_and_cancel_timeout(&server.base_url, 60_000, 20);
        let root = temp_root("artifact-create-receipt");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("receipt");
        let binding = create_binding("request:artifact-create-receipt", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let run = wait_for_run_state(&root, &run_id, "artifact create durable receipt", |run| {
            run.events.len() == 2 && run.status == crate::contract::RunStatus::Running
        });
        let public_run =
            serde_json::to_string(&get_run(&provider, &run_id, &access_binding(&binding))).unwrap();
        let public_events = serde_json::to_string(&events_page(
            &provider,
            &run_id,
            &access_binding(&binding),
            0,
        ))
        .unwrap();

        let kinds = run
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["prepared", "dispatched"]);
        assert_eq!(run.status, crate::contract::RunStatus::Running);
        assert_eq!(
            run.backend_state,
            Some(json!({
                "schema": "elastos.model.provider-http-job-state/v1",
                "phase": "active",
                "job_id": "job-123",
                "next_poll_at_ms": run.backend_state.as_ref().unwrap()["next_poll_at_ms"].clone(),
                "cancel_requested": false,
                "cancel_sent": false,
                "cancel_deadline_ms": null,
            }))
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert!(!public_run.contains("job-123"));
        assert!(!public_run.contains(&server.base_url));
        assert!(!public_run.contains("sentinel-artifact-token"));
        assert!(!public_events.contains("job-123"));
        assert!(!public_events.contains(&server.base_url));
        assert!(!public_events.contains("sentinel-artifact-token"));

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_create_transport_or_body_loss_after_request_settles_unknown_without_leakage() {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let body = br#"{"job_id":"backend-private-job"}"#.to_vec();
        let prefix_len = 11;
        let server = start_server(vec![ResponseAction {
            status_line: "200 OK",
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Transfer-Encoding".to_string(), "chunked".to_string()),
                ("Connection".to_string(), "keep-alive".to_string()),
            ],
            body: body.clone(),
            stalled_prefix: Some(
                [
                    format!("{prefix_len:X}\r\n").into_bytes(),
                    body[..prefix_len].to_vec(),
                    b"\r\n".to_vec(),
                ]
                .concat(),
            ),
            stalled_suffix: None,
            hold_open: Some(Arc::clone(&release)),
            stalled_body_started: Some(Arc::clone(&started)),
            stalled_response_entered: None,
        }]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-transport-loss");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("transport loss");
        let binding = create_binding("request:artifact-create-transport-loss", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_for_flag(&started);
        release.store(true, Ordering::Relaxed);

        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let public_run = serde_json::to_string(&terminal).unwrap();
        let public_events = serde_json::to_string(&page).unwrap();
        assert_eq!(terminal["data"]["status"], "settlement_unknown");
        assert_eq!(
            terminal["data"]["terminal"]["error"]["class"],
            "settlement_unknown"
        );
        assert_no_private_leakage(
            &[public_run, public_events],
            &[
                &server.base_url,
                "sentinel-artifact-token",
                "backend-private-job",
            ],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_create_malformed_json_2xx_settles_unknown_without_leakage() {
        let server = start_server(vec![ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: br#"{"broken":"malformed-sentinel""#.to_vec(),
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-malformed");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("malformed");
        let binding = create_binding("request:artifact-create-malformed", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        assert_eq!(terminal["data"]["status"], "settlement_unknown");
        assert_eq!(
            terminal["data"]["terminal"]["error"]["class"],
            "settlement_unknown"
        );
        assert_no_private_leakage(
            &[
                serde_json::to_string(&terminal).unwrap(),
                serde_json::to_string(&page).unwrap(),
            ],
            &[
                &server.base_url,
                "sentinel-artifact-token",
                "malformed-sentinel",
            ],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_create_missing_or_invalid_job_id_2xx_settles_unknown_without_leakage() {
        for (label, body, marker) in [
            (
                "missing",
                serde_json::to_vec(&json!({"note":"backend-missing-job-marker"})).unwrap(),
                "backend-missing-job-marker",
            ),
            (
                "invalid",
                serde_json::to_vec(&json!({"job_id":" backend-invalid-job"})).unwrap(),
                "backend-invalid-job",
            ),
        ] {
            let server = start_server(vec![ResponseAction {
                status_line: "200 OK",
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body,
                stalled_prefix: None,
                stalled_suffix: None,
                hold_open: None,
                stalled_body_started: None,
                stalled_response_entered: None,
            }]);
            let offer = artifact_offer(&server.base_url);
            let root = temp_root(&format!("artifact-create-{label}-job-id"));
            let mut provider = ProviderCoordinatorHandle::start();
            init_provider(&provider, &root, vec![offer.clone()]);

            let input = artifact_input(label);
            let binding = create_binding(
                &format!("request:artifact-create-{label}-job-id"),
                &offer,
                &input,
            );
            let create_response = create_run(&provider, &offer, &binding, &input);
            let run_id = create_response["data"]["run_id"]
                .as_str()
                .unwrap()
                .to_string();
            let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
            let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
            assert_eq!(terminal["data"]["status"], "settlement_unknown");
            assert_eq!(
                terminal["data"]["terminal"]["error"]["class"],
                "settlement_unknown"
            );
            assert_no_private_leakage(
                &[
                    serde_json::to_string(&terminal).unwrap(),
                    serde_json::to_string(&page).unwrap(),
                ],
                &[&server.base_url, "sentinel-artifact-token", marker],
            );

            provider.shutdown_on_eof();
        }
    }

    #[test]
    fn artifact_create_oversized_2xx_settles_unknown_without_leakage() {
        let marker = "X".repeat(5 * 1024 * 1024);
        let server = start_server(vec![ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: marker.as_bytes().to_vec(),
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }]);
        let offer = artifact_offer(&server.base_url);
        let root = temp_root("artifact-create-oversized");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("oversized");
        let binding = create_binding("request:artifact-create-oversized", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        assert_eq!(terminal["data"]["status"], "settlement_unknown");
        assert_eq!(
            terminal["data"]["terminal"]["error"]["class"],
            "settlement_unknown"
        );
        assert_no_private_leakage(
            &[
                serde_json::to_string(&terminal).unwrap(),
                serde_json::to_string(&page).unwrap(),
            ],
            &[&server.base_url, "sentinel-artifact-token", &marker[..128]],
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn missing_done_signal_fails_closed() {
        let server = start_server(vec![sse_action(
            &[json!({"choices":[{"delta":{"content":"partial"}}]}).to_string()],
            false,
        )]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("missing-done");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("partial");
        let binding = create_binding("request:missing-done", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));

        assert_eq!(terminal["data"]["status"], "failed");
        assert_eq!(
            terminal["data"]["terminal"]["error"]["class"],
            "response_malformed"
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn oversized_output_fails_before_completion() {
        let server = start_server(vec![sse_action(
            &[
                json!({"choices":[{"delta":{"content":"ok"}}]}).to_string(),
                json!({"choices":[{"delta":{"content":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}}]}).to_string(),
            ],
            true,
        )]);
        let mut offer = local_text_offer(&server.base_url);
        offer.policy.inline_output_bytes_limit = 64;
        let root = temp_root("output-limit");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("overflow");
        let binding = create_binding("request:output-limit", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));

        assert_eq!(terminal["data"]["status"], "failed");
        assert_eq!(
            terminal["data"]["terminal"]["error"]["class"],
            "response_malformed"
        );
        let run = load_run(&root, &run_id);
        let kinds = run
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["prepared", "dispatched", "failed"]);
        assert!(!kinds.contains(&"text_delta"));
        assert!(run.output.is_none());

        provider.shutdown_on_eof();
    }

    #[test]
    fn oversized_stream_bytes_fail_closed() {
        let filler = "data: {\"choices\":[{\"delta\":{}}]}\n\n";
        let repeats = (TEST_LOCAL_TEXT_STREAM_BYTES_LIMIT / filler.len()) + 2;
        let body = filler.repeat(repeats).into_bytes();
        let server = start_server(vec![ResponseAction {
            status_line: "200 OK",
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body,
            stalled_prefix: None,
            stalled_suffix: None,
            hold_open: None,
            stalled_body_started: None,
            stalled_response_entered: None,
        }]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("stream-bytes");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("oversized stream");
        let binding = create_binding("request:stream-bytes", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));

        assert_eq!(terminal["data"]["status"], "failed");
        let run = load_run(&root, &run_id);
        let kinds = run
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["prepared", "dispatched", "failed"]);

        provider.shutdown_on_eof();
    }

    #[test]
    fn coalescing_keeps_token_stream_within_durable_event_budget() {
        let events = (0..254)
            .map(|_| json!({"choices":[{"delta":{"content":"x"}}]}).to_string())
            .collect::<Vec<_>>();
        let server = start_server(vec![sse_action(&events, true)]);
        let offer = local_text_offer(&server.base_url);
        let root = temp_root("event-budget");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("event budget");
        let binding = create_binding("request:event-budget", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));

        assert_eq!(terminal["data"]["status"], "completed");
        let run = load_run(&root, &run_id);
        assert_eq!(
            run.output,
            Some(json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "x".repeat(254),
            }))
        );
        let kinds = run
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["prepared", "dispatched", "text_delta", "output"]
        );
        let text_delta_events = run
            .events
            .iter()
            .filter(|event| event.kind == "text_delta")
            .collect::<Vec<_>>();
        assert_eq!(text_delta_events.len(), 1);
        assert_eq!(
            text_delta_events[0].data,
            json!({ "text": "x".repeat(254) })
        );

        provider.shutdown_on_eof();
    }

    #[test]
    fn redirect_is_not_followed_and_location_does_not_leak() {
        let target = start_server(vec![sse_action(
            &[json!({"choices":[{"delta":{"content":"redirected"}}]}).to_string()],
            true,
        )]);
        let redirect = start_server(vec![redirect_action(format!(
            "{}/redirected",
            target.base_url
        ))]);
        let offer = local_text_offer(&redirect.base_url);
        let root = temp_root("redirect");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = text_input("redirect");
        let binding = create_binding("request:redirect", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let public_run = serde_json::to_string(&terminal).unwrap();
        let public_events = serde_json::to_string(&page).unwrap();

        assert_eq!(terminal["data"]["status"], "failed");
        assert_eq!(redirect.requests.lock().unwrap().len(), 1);
        assert_eq!(target.requests.lock().unwrap().len(), 0);
        assert!(!public_run.contains("/redirected"));
        assert!(!public_run.contains(&target.base_url));
        assert!(!public_run.contains("sentinel-openai-key"));
        assert!(!public_events.contains("/redirected"));
        assert!(!public_events.contains(&target.base_url));

        provider.shutdown_on_eof();
    }

    #[test]
    fn artifact_create_redirect_is_not_followed_and_location_does_not_leak() {
        let target = start_server(vec![json_action(json!({"job_id":"redirected-job"}))]);
        let redirect = start_server(vec![redirect_action(format!(
            "{}/redirected",
            target.base_url
        ))]);
        let offer = artifact_offer(&redirect.base_url);
        let root = temp_root("artifact-create-redirect");
        let mut provider = ProviderCoordinatorHandle::start();
        init_provider(&provider, &root, vec![offer.clone()]);

        let input = artifact_input("redirect");
        let binding = create_binding("request:artifact-create-redirect", &offer, &input);
        let create_response = create_run(&provider, &offer, &binding, &input);
        let run_id = create_response["data"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = wait_for_terminal(&provider, &run_id, &access_binding(&binding));
        let page = events_page(&provider, &run_id, &access_binding(&binding), 0);
        let public_run = serde_json::to_string(&terminal).unwrap();
        let public_events = serde_json::to_string(&page).unwrap();

        assert_eq!(terminal["data"]["status"], "settlement_unknown");
        assert_eq!(redirect.requests.lock().unwrap().len(), 1);
        assert_eq!(target.requests.lock().unwrap().len(), 0);
        assert!(!public_run.contains("/redirected"));
        assert!(!public_run.contains(&target.base_url));
        assert!(!public_run.contains("sentinel-artifact-token"));
        assert!(!public_events.contains("/redirected"));
        assert!(!public_events.contains(&target.base_url));
        assert!(!public_events.contains("sentinel-artifact-token"));

        provider.shutdown_on_eof();
    }
}
