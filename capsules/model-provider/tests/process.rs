#![cfg(unix)]

#[path = "../src/test_support.rs"]
mod test_support;

use elastos_model_contract::{model_input_hash, RUNTIME_CREATE_BINDING_SCHEMA};
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

static LOCAL_ENGINE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

const PROCESS_DEADLINE: Duration = Duration::from_secs(5);
const MAX_STDERR_BYTES: usize = 64 * 1024;

struct ProviderProcess {
    _local_engine_test: Option<std::sync::MutexGuard<'static, ()>>,
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<Value>,
    stderr: Receiver<std::io::Result<(Vec<u8>, bool)>>,
}

impl ProviderProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_model-provider"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = (|| -> std::io::Result<_> {
                let mut captured = Vec::new();
                let mut overflow = false;
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stderr.read(&mut buffer)?;
                    if count == 0 {
                        return Ok((captured, overflow));
                    }
                    let retained = count.min(MAX_STDERR_BYTES - captured.len());
                    captured.extend_from_slice(&buffer[..retained]);
                    overflow |= retained < count;
                    // Drain after the cap too, so diagnostics cannot block the child.
                }
            })();
            let _ = stderr_tx.send(result);
        });
        let (response_tx, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if response_tx.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            _local_engine_test: None,
            stdin: child.stdin.take(),
            child,
            responses,
            stderr: stderr_rx,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &request).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        self.responses.recv_timeout(PROCESS_DEADLINE).unwrap()
    }

    fn hard_kill(&mut self) {
        let pid = self.child.id() as libc::pid_t;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        self.stdin.take();
        let status = wait_for_child_exit(&mut self.child)
            .unwrap_or_else(|| panic!("model provider {pid} remained alive"));
        assert!(!status.success());
        assert!(!process_exists(pid), "model provider {pid} remained alive");
    }

    fn shutdown(&mut self) {
        let response = self.request(json!({"op": "shutdown"}));
        assert_eq!(response["status"], "ok");
        self.stdin.take();
        let pid = self.child.id() as libc::pid_t;
        let status = wait_for_child_exit(&mut self.child)
            .unwrap_or_else(|| panic!("model provider {pid} remained alive"));
        assert!(status.success());
        assert!(!process_exists(pid), "model provider {pid} remained alive");
    }
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        self.stdin.take();
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn init_local_llama(label: &str, oversized: bool) -> (ProviderProcess, PathBuf) {
    let root = test_support::temp_root_path("model-provider-process", label);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (engine, model, events) =
        test_support::write_fake_llama_server(&root, "healthy_with_subtree");
    if oversized {
        std::fs::write(&model, test_support::fake_gguf_metadata(512, 131_072, 1, 1)).unwrap();
    }
    let offer = json!({
        "id": "local-text",
        "title": "Local Text",
        "operation": "text.generate",
        "input_modalities": ["text/plain"],
        "output_modalities": ["text/plain"],
        "policy": {
            "concurrency_limit": 1,
            "input_bytes_limit": 8192,
            "inline_output_bytes_limit": 8192,
            "event_bytes_limit": 8192,
            "runtime_ms_limit": 30000,
            "retention_secs": 60,
            "cancel_settlement_timeout_ms": 20
        },
        "adapter": {
            "kind": "local_llama_cpp_text",
            "engine": {
                "path": engine,
                "sha256": test_support::sha256_file(&engine)
            },
            "model": {
                "path": model,
                "sha256": test_support::sha256_file(&model)
            },
            "settings": {
                "context_size": if oversized { 32768 } else { 256 },
                "parallel": 1,
                "threads": 1,
                "batch_threads": 1,
                "gpu_layers": 0,
                "health_timeout_ms": 2000,
                "shutdown_timeout_ms": 250,
                "enable_thinking": false
            }
        },
        "enabled": true
    });
    let mut provider = ProviderProcess::start();
    let init = provider.request(json!({
        "op": "init",
        "config": {
            "base_path": root,
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": "",
            "extra": {
                "provider_id": "model-provider",
                "journal_dir": root.join("journal"),
                "offers": [offer]
            }
        }
    }));
    assert_eq!(init["status"], "ok", "unexpected init response: {init}");

    (provider, events)
}

fn create_local_run(provider: &mut ProviderProcess, label: &str) -> Value {
    create_local_prompt(provider, label, "process-lifecycle")
}

fn create_local_prompt(provider: &mut ProviderProcess, label: &str, prompt: &str) -> Value {
    let input = json!({
        "schema": "elastos.model.input.text/v1",
        "prompt": prompt
    });
    let create = provider.request(json!({
        "op": "runs_create",
        "offer_id": "local-text",
        "operation": "text.generate",
        "input": input,
        "runtime_binding": {
            "schema": RUNTIME_CREATE_BINDING_SCHEMA,
            "principal_id": "person:local:test",
            "session_id": "session:test",
            "capsule_id": "assistant",
            "grant_id": "grant:test",
            "request_id": format!("request:{label}"),
            "offer_id": "local-text",
            "operation": "text.generate",
            "input_hash": model_input_hash(&input).unwrap()
        }
    }));
    assert_eq!(
        create["status"], "ok",
        "unexpected create response: {create}"
    );
    create
}

fn start_local_llama_run(label: &str) -> (ProviderProcess, PathBuf) {
    let lease = LOCAL_ENGINE_TEST.lock().unwrap();
    let (mut provider, events) = init_local_llama(label, false);
    provider._local_engine_test = Some(lease);
    create_local_run(&mut provider, label);
    wait_for_event(&events, "start:");
    wait_for_event(&events, "parent:");
    wait_for_event(&events, "subtree:");
    (provider, events)
}

fn cancel_local_run(provider: &mut ProviderProcess, created: &Value) {
    let response = provider.request(json!({
        "op":"runs_cancel", "run_id":created["data"]["run_id"],
        "runtime_binding": {
            "schema":"elastos.model.runtime-access-binding/v1",
            "principal_id":"person:local:test", "session_id":"session:test",
            "capsule_id":"assistant", "grant_id":"grant:test", "request_id":"request:cancel",
            "run_id":created["data"]["run_id"]
        }
    }));
    assert_eq!(response["status"], "ok");
}

fn terminal_local_run(provider: &mut ProviderProcess, created: &Value) -> Value {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let view = provider.request(json!({
            "op":"runs_get", "run_id":created["data"]["run_id"],
            "runtime_binding": {
                "schema":"elastos.model.runtime-access-binding/v1",
                "principal_id":"person:local:test", "session_id":"session:test",
                "capsule_id":"assistant", "grant_id":"grant:test", "request_id":"request:read",
                "run_id":created["data"]["run_id"]
            }
        }));
        if matches!(
            view["data"]["status"].as_str(),
            Some("failed" | "completed" | "cancelled" | "settlement_unknown")
        ) {
            return view;
        }
        assert!(Instant::now() < deadline, "run failed to settle: {view}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn two_roots_refuse_busy_and_recover_after_owner_crash() {
    let (mut first, first_events) = start_local_llama_run("lease-first");
    let (mut second, second_events) = init_local_llama("lease-second", false);
    let created = create_local_run(&mut second, "lease-busy");
    let refused = terminal_local_run(&mut second, &created);
    assert_eq!(refused["data"]["terminal"]["error"]["code"], "model_busy");
    assert_eq!(
        refused["data"]["terminal"]["error"]["message"],
        "Model is busy."
    );
    assert!(event_lines(&second_events).is_empty());
    let public = serde_json::to_string(&refused).unwrap();
    assert!(!public.contains("lease-first") && !public.contains("elastos-local-model"));
    first.hard_kill();
    assert!(wait_for_process_exit(engine_pid(&first_events)));
    assert!(wait_for_process_exit(guard_pid(&first_events)));
    assert!(wait_for_process_exit(subtree_pid(&first_events)));
    let created = create_local_run(&mut second, "lease-recovered");
    let recovered = terminal_local_run(&mut second, &created);
    assert_eq!(recovered["data"]["status"], "completed", "{recovered}");
    second.shutdown();
    assert!(wait_for_process_exit(engine_pid(&second_events)));
}

#[test]
fn queue_saturation_and_waiting_cancellation_preserve_active_engine() {
    let _lease = LOCAL_ENGINE_TEST.lock().unwrap();
    let (mut provider, events) = init_local_llama("queue", false);
    let first = create_local_prompt(&mut provider, "queue-active", "stall");
    wait_for_event(&events, "request:stall");
    let mut waiting = Vec::new();
    for index in 0..8 {
        let created = create_local_run(&mut provider, &format!("queue-{index}"));
        assert_eq!(created["data"]["status"], "running");
        waiting.push(created);
    }
    let excess = create_local_run(&mut provider, "queue-full");
    assert_eq!(excess["data"]["terminal"]["error"]["code"], "model_busy");
    for created in &waiting {
        cancel_local_run(&mut provider, created);
        assert_eq!(
            terminal_local_run(&mut provider, created)["data"]["status"],
            "cancelled"
        );
    }
    assert_eq!(
        event_lines(&events)
            .iter()
            .filter(|event| event.starts_with("start:"))
            .count(),
        1
    );
    assert!(process_exists(engine_pid(&events)));
    cancel_local_run(&mut provider, &first);
    assert_eq!(
        terminal_local_run(&mut provider, &first)["data"]["status"],
        "settlement_unknown"
    );
    assert!(wait_for_process_exit(engine_pid(&events)));
    provider.shutdown();
}

#[test]
fn insufficient_memory_refuses_before_any_engine_process() {
    let _lease = LOCAL_ENGINE_TEST.lock().unwrap();
    let (mut provider, events) = init_local_llama("memory-refused", true);
    let created = create_local_run(&mut provider, "memory-refused");
    let refused = terminal_local_run(&mut provider, &created);
    provider.shutdown();
    assert_eq!(
        refused["data"]["terminal"]["error"]["code"], "model_memory_unavailable",
        "{refused}"
    );
    assert!(event_lines(&events).is_empty());
}

fn wait_for_event(path: &Path, prefix: &str) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !event_lines(path)
        .iter()
        .any(|line| line.starts_with(prefix))
    {
        assert!(Instant::now() < deadline, "missing process event {prefix}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn event_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn engine_pid(path: &Path) -> libc::pid_t {
    recorded_pid(path, "start:")
}

fn guard_pid(path: &Path) -> libc::pid_t {
    recorded_pid(path, "parent:")
}

fn subtree_pid(path: &Path) -> libc::pid_t {
    recorded_pid(path, "subtree:")
}

fn recorded_pid(path: &Path, prefix: &str) -> libc::pid_t {
    event_lines(path)
        .into_iter()
        .find_map(|line| line.strip_prefix(prefix)?.parse().ok())
        .unwrap()
}

fn process_exists(pid: libc::pid_t) -> bool {
    (unsafe { libc::kill(pid, 0) }) == 0
}

fn wait_for_child_exit(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        match child.try_wait().unwrap() {
            Some(status) => return Some(status),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => return None,
        }
    }
}

fn wait_for_process_exit(pid: libc::pid_t) -> bool {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    !process_exists(pid)
}

fn assert_guard_and_engine_stopped(events: &Path) {
    let pids = [guard_pid(events), engine_pid(events), subtree_pid(events)];
    let stopped = pids.map(wait_for_process_exit);
    for (pid, stopped) in pids.into_iter().zip(stopped) {
        if !stopped {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        assert!(stopped, "model process {pid} remained alive");
    }
}

#[test]
fn hard_killed_provider_reaps_local_llama_engine() {
    let (mut provider, events) = start_local_llama_run("hard-kill");

    provider.hard_kill();

    assert_guard_and_engine_stopped(&events);
}

#[test]
fn normal_provider_shutdown_reaps_local_llama_engine() {
    let (mut provider, events) = start_local_llama_run("shutdown");

    provider.shutdown();

    assert_guard_and_engine_stopped(&events);
    assert_eq!(
        event_lines(&events)
            .iter()
            .filter(|line| line.as_str() == "term")
            .count(),
        1
    );
    let (stderr, overflow) = provider
        .stderr
        .recv_timeout(PROCESS_DEADLINE)
        .unwrap()
        .unwrap();
    assert!(!overflow, "provider stderr exceeded fixture limit");
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        !stderr.contains("local engine closure"),
        "normal shutdown did not confirm engine closure: {stderr}"
    );
}

#[test]
fn wedged_guard_forces_local_llama_group_cleanup() {
    let (mut provider, events) = start_local_llama_run("wedged-guard");
    let guard = guard_pid(&events);
    assert_eq!(unsafe { libc::kill(guard, libc::SIGSTOP) }, 0);

    provider.shutdown();

    assert_guard_and_engine_stopped(&events);
}

#[test]
fn unexpected_guard_exit_forces_local_llama_group_cleanup() {
    let (mut provider, events) = start_local_llama_run("guard-exit");
    let guard = guard_pid(&events);
    assert_eq!(unsafe { libc::kill(guard, libc::SIGKILL) }, 0);

    provider.shutdown();

    assert_guard_and_engine_stopped(&events);
}

#[test]
fn production_provider_refuses_every_external_adapter_before_socket_and_after_restart() {
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    sink.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", sink.local_addr().unwrap());
    let root = test_support::temp_root_path("model-provider-process", "external-paused");
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let policy = json!({
        "concurrency_limit": 1, "input_bytes_limit": 8192,
        "inline_output_bytes_limit": 8192, "event_bytes_limit": 8192,
        "runtime_ms_limit": 30000, "retention_secs": 60,
        "cancel_settlement_timeout_ms": 20
    });
    let hosted = json!({
        "backend_provider_label": "Fixture Provider", "selection_mode": "pinned",
        "privacy_policy_ref": "fixture:privacy:v1", "terms_ref": "fixture:terms:v1",
        "upstream_routing_fallback_assertion": "operator_asserted_disabled",
        "model_privacy": ""
    });
    let adapters = [
        (
            "decisions",
            "decision.evaluate",
            json!({"kind":"open_router_decisions",
            "api_url":format!("{endpoint}/decisions"),"api_key":"fixture-key","model":"fixture/jev",
            "hosted":hosted}),
        ),
        (
            "chat",
            "text.generate",
            json!({"kind":"open_ai_compatible_text",
            "api_url":format!("{endpoint}/chat"),"api_key":"fixture-key","model":"fixture/text",
            "hosted":hosted}),
        ),
        (
            "responses",
            "text.generate",
            json!({"kind":"open_ai_responses_text",
            "api_url":format!("{endpoint}/responses"),"api_key":"fixture-key","model":"fixture/text",
            "hosted":hosted}),
        ),
        (
            "job",
            "image.generate",
            json!({"kind":"http_job_artifact",
            "create_url":format!("{endpoint}/create"),"status_url":format!("{endpoint}/status"),
            "cancel_url":format!("{endpoint}/cancel"),"bearer_token":"fixture-key",
            "poll_interval_ms":1000}),
        ),
    ];
    let offers = adapters.iter().map(|(id, operation, adapter)| json!({
        "id":id,"title":id,"operation":operation,
        "input_modalities":if *id=="chat" || *id=="responses" {json!(["text/plain"])} else {json!(["application/json"])},
        "output_modalities":if *id=="chat" || *id=="responses" {json!(["text/plain"])} else {json!(["application/json"])},
        "policy":policy,"adapter":adapter,"enabled":true
    })).collect::<Vec<_>>();
    let init = json!({"op":"init","config":{
        "base_path":root,"allowed_paths":[],"read_only":false,"encryption_key":"",
        "extra":{"provider_id":"model-provider","journal_dir":root.join("journal"),"offers":offers}
    }});
    for pass in 0..2 {
        let mut provider = ProviderProcess::start();
        let started = provider.request(init.clone());
        assert_eq!(
            started["status"], "ok",
            "existing offers must survive startup: {started}"
        );
        let listed = provider.request(json!({"op":"offers_list"}));
        for (id, expected) in [
            (
                "chat",
                json!([
                    elastos_model_contract::TEXT_INPUT_V1_SCHEMA,
                    elastos_model_contract::TEXT_INPUT_V2_SCHEMA
                ]),
            ),
            (
                "responses",
                json!([elastos_model_contract::TEXT_INPUT_V1_SCHEMA]),
            ),
        ] {
            let offer = listed["data"]["offers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|offer| offer["id"] == id)
                .unwrap();
            assert_eq!(offer["input_schemas"], expected);
        }
        // Trace the production offer reply through the canonical Assistant selector.
        // Load as a data module so this fixture also works on older Node runtimes.
        let ui = Command::new("node")
            .args(["--input-type=module", "-e", r#"
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const source = readFileSync('../assistant/browser/model-contract.js', 'utf8');
const contract = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const reply = JSON.parse(process.argv[1]);
const messages = [{role:'system',content:'Be concise.'},{role:'user',content:'first'},
  {role:'agent',content:'reply'},{role:'user',content:'last'}];
const rows = contract.textOfferRows(contract.eligibleTextOffers(reply));
for (const id of ['chat','responses']) {
  const row = rows.find(row => row.offerId === id);
  const body = contract.textRunCreateBody({offer:row,messages,requestId:'fixture'});
  assert.equal(body.input.schema, id === 'chat' ? contract.MODEL_TEXT_INPUT_V2_SCHEMA : contract.MODEL_TEXT_INPUT_SCHEMA);
  if (id === 'chat') assert.deepEqual(body.input.messages, messages.map(m=>({...m,role:m.role==='agent'?'assistant':m.role})));
  else assert.equal(body.input.prompt, contract.transcriptPrompt(messages));
}
"#])
            .arg(listed.to_string())
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Node is required for the Assistant contract fixture");
        assert!(
            ui.status.success(),
            "Assistant offer negotiation: {}",
            String::from_utf8_lossy(&ui.stderr)
        );
        for (id, operation, _) in &adapters {
            let input = json!({});
            let request_id = format!("request:external-paused:{pass}:{id}");
            let create = json!({"op":"runs_create","offer_id":id,"operation":operation,
            "input":input,"runtime_binding":{
                "schema":RUNTIME_CREATE_BINDING_SCHEMA,"principal_id":"person:local:test",
                "session_id":"session:test","capsule_id":"assistant","grant_id":"grant:test",
                "request_id":request_id,"offer_id":id,"operation":operation,
                "input_hash":model_input_hash(&input).unwrap()
            }});
            let reply = provider.request(create.clone());
            assert!(
                reply
                    .to_string()
                    .contains("Hosted external HTTPS is paused."),
                "{id}: {reply}"
            );
            let retry = provider.request(create);
            assert!(
                retry
                    .to_string()
                    .contains("Hosted external HTTPS is paused."),
                "retry {id}: {retry}"
            );
        }
        provider.shutdown();
    }
    assert!(matches!(sink.accept(), Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
}
