#![cfg(unix)]

#[path = "../src/test_support.rs"]
mod test_support;

use elastos_model_contract::{model_input_hash, RUNTIME_CREATE_BINDING_SCHEMA};
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_DEADLINE: Duration = Duration::from_secs(5);
const MAX_STDERR_BYTES: usize = 64 * 1024;

struct ProviderProcess {
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

fn start_local_llama_run(label: &str) -> (ProviderProcess, PathBuf) {
    let root = test_support::temp_root_path("model-provider-process", label);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (engine, model, events) =
        test_support::write_fake_llama_server(&root, "healthy_with_subtree");
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
                "context_size": 256,
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

    let input = json!({
        "schema": "elastos.model.input.text/v1",
        "prompt": "process-lifecycle"
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
    wait_for_event(&events, "start:");
    wait_for_event(&events, "parent:");
    wait_for_event(&events, "subtree:");
    (provider, events)
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
