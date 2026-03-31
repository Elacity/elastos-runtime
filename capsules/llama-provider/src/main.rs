//! ElastOS llama-provider Capsule
//!
//! Manages a llama-server subprocess for local LLM inference.
//! Eager port bind during init, lazy model loading on first request.
//! Wire protocol: line-delimited JSON over stdin/stdout.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const MAX_RESTARTS: u32 = 3;

// ── Protocol types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    ChatCompletions {
        #[serde(default)]
        messages: Vec<Message>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        temperature: Option<f64>,
        #[serde(default)]
        max_tokens: Option<u64>,
    },
    Status,
    Health,
    ListModels,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: serde_json::Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn error(code: &str, message: &str) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

// ── State machine ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServerState {
    Cold,
    Starting,
    Ready,
    Error,
}

// ── Config ──────────────────────────────────────────────────────────

struct LlamaConfig {
    model_path: PathBuf,
    n_ctx: u32,
    n_gpu_layers: i32,
    threads: Option<u32>,
}

// ── Provider ────────────────────────────────────────────────────────

struct LlamaProvider {
    state: ServerState,
    port: u16,
    config: Option<LlamaConfig>,
    child: Option<Child>,
    llama_binary: Option<PathBuf>,
    restart_count: u32,
}

impl LlamaProvider {
    fn new() -> Self {
        Self {
            state: ServerState::Cold,
            port: 0,
            config: None,
            child: None,
            llama_binary: None,
            restart_count: 0,
        }
    }

    fn endpoint(&self) -> Option<String> {
        if self.port > 0 {
            Some(format!("http://127.0.0.1:{}", self.port))
        } else {
            None
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::ChatCompletions {
                messages,
                model,
                temperature,
                max_tokens,
            } => self.chat_completions(messages, model, temperature, max_tokens),
            Request::Status => self.status(),
            Request::Health => self.health(),
            Request::ListModels => self.list_models(),
            Request::Shutdown => self.shutdown(),
        }
    }

    fn init(&mut self, config: serde_json::Value) -> Response {
        let extra = config.get("extra").unwrap_or(&config);

        // Find llama-server binary
        let binary = find_llama_binary();
        if binary.is_none() {
            return Response::error(
                "binary_not_found",
                "llama-server not found. Run: elastos setup --with llama-server",
            );
        }
        self.llama_binary = binary;

        // Find model file
        let model_path = resolve_model_path(extra);
        let model_path = match model_path {
            Some(p) => p,
            None => {
                return Response::error(
                    "model_not_found",
                    "No model GGUF found. Run: elastos setup --with model-qwen3.5-0.8b",
                );
            }
        };

        let n_ctx = extra.get("n_ctx").and_then(|v| v.as_u64()).unwrap_or(4096) as u32;
        let n_gpu_layers = extra
            .get("n_gpu_layers")
            .and_then(|v| v.as_i64())
            .unwrap_or(99) as i32;
        let threads = extra
            .get("threads")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        self.config = Some(LlamaConfig {
            model_path,
            n_ctx,
            n_gpu_layers,
            threads,
        });

        // Deterministic port if provided; otherwise bind a free one.
        let requested_port = extra
            .get("port")
            .and_then(|v| v.as_u64())
            .and_then(|v| if v > u16::MAX as u64 { None } else { Some(v as u16) })
            .filter(|p| *p > 0);
        match bind_port(requested_port) {
            Ok(port) => self.port = port,
            Err(e) => {
                return Response::error("port_bind_failed", &format!("Failed to bind port: {}", e));
            }
        }

        // Eager start: spawn llama-server immediately so the endpoint is live.
        // Model loading happens in the background — llama-server accepts connections
        // while loading and returns 503 until ready.
        self.restart_count = 0;
        if let Err(e) = self.start_server() {
            return Response::error("start_failed", &e);
        }

        Response::ok(serde_json::json!({
            "provider": "llama-provider",
            "state": self.state,
            "port": self.port,
            "endpoint": self.endpoint(),
        }))
    }

    fn chat_completions(
        &mut self,
        messages: Vec<Message>,
        model: Option<String>,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Response {
        // Lazy start: Cold → Starting → Ready
        if self.state == ServerState::Cold || self.state == ServerState::Error {
            if let Err(e) = self.start_server() {
                return Response::error("start_failed", &e);
            }
        }

        if self.state == ServerState::Starting {
            if let Err(e) = self.wait_for_ready() {
                return Response::error("health_timeout", &e);
            }
        }

        if self.state != ServerState::Ready {
            // Check if process died
            if self.check_process_died() {
                if self.restart_count < MAX_RESTARTS {
                    self.state = ServerState::Cold;
                    return self.chat_completions(messages, model, temperature, max_tokens);
                }
                return Response::error(
                    "server_crashed",
                    &format!(
                        "llama-server crashed {} times. Check model/GPU compatibility.",
                        self.restart_count
                    ),
                );
            }
            return Response::error("not_ready", "llama-server not ready");
        }

        // Proxy to llama-server
        self.proxy_chat_completions(messages, model, temperature, max_tokens)
    }

    fn start_server(&mut self) -> Result<(), String> {
        let config = self.config.as_ref().ok_or("Not initialized")?;
        let binary = self.llama_binary.as_ref().ok_or("Binary not found")?;

        self.state = ServerState::Starting;
        self.restart_count += 1;

        let mut cmd = Command::new(binary);
        cmd.arg("-m")
            .arg(&config.model_path)
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("-ngl")
            .arg(config.n_gpu_layers.to_string())
            .arg("-c")
            .arg(config.n_ctx.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        // Set LD_LIBRARY_PATH so llama-server finds its shared libraries
        // (libllama.so, libggml.so, etc. live alongside the binary)
        if let Some(binary_dir) = binary.parent() {
            let lib_path = if let Ok(existing) = std::env::var("LD_LIBRARY_PATH") {
                format!("{}:{}", binary_dir.display(), existing)
            } else {
                binary_dir.display().to_string()
            };
            cmd.env("LD_LIBRARY_PATH", lib_path);
        }

        if let Some(threads) = config.threads {
            cmd.arg("-t").arg(threads.to_string());
        }

        match cmd.spawn() {
            Ok(child) => {
                eprintln!(
                    "llama-provider: spawned llama-server pid={} port={} model={}",
                    child.id(),
                    self.port,
                    config.model_path.display()
                );
                self.child = Some(child);
                Ok(())
            }
            Err(e) => {
                self.state = ServerState::Error;
                Err(format!("Failed to spawn llama-server: {}", e))
            }
        }
    }

    fn wait_for_ready(&mut self) -> Result<(), String> {
        let health_url = format!("http://127.0.0.1:{}/health", self.port);
        let start = Instant::now();

        eprintln!(
            "llama-provider: waiting for llama-server health (timeout {}s)...",
            HEALTH_TIMEOUT.as_secs()
        );

        loop {
            if start.elapsed() > HEALTH_TIMEOUT {
                self.state = ServerState::Error;
                return Err(format!(
                    "llama-server health timeout after {}s. Model may be too large for available memory.",
                    HEALTH_TIMEOUT.as_secs()
                ));
            }

            // Check if process died
            if self.check_process_died() {
                self.state = ServerState::Error;
                return Err("llama-server process exited during startup".to_string());
            }

            match ureq::get(&health_url)
                .timeout(Duration::from_secs(5))
                .call()
            {
                Ok(resp) => {
                    if resp.status() == 200 {
                        // Check if model is fully loaded (llama-server returns {"status":"ok"})
                        if let Ok(body) = resp.into_json::<serde_json::Value>() {
                            let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
                            if status == "ok" || status == "no slot available" {
                                self.state = ServerState::Ready;
                                self.restart_count = 0;
                                eprintln!(
                                    "llama-provider: llama-server ready (took {:.1}s)",
                                    start.elapsed().as_secs_f64()
                                );
                                return Ok(());
                            }
                            // "loading model" — keep polling
                        }
                    }
                }
                Err(_) => {
                    // Not up yet, keep polling
                }
            }

            std::thread::sleep(HEALTH_POLL_INTERVAL);
        }
    }

    fn check_process_died(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    eprintln!("llama-provider: llama-server process exited");
                    self.child = None;
                    true
                }
                Ok(None) => false, // Still running
                Err(_) => false,
            }
        } else {
            // No child process
            self.state != ServerState::Cold
        }
    }

    fn proxy_chat_completions(
        &self,
        messages: Vec<Message>,
        model: Option<String>,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Response {
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);

        let model_name = model.unwrap_or_else(|| {
            self.config
                .as_ref()
                .map(|c| {
                    c.model_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_else(|| "default".to_string())
        });

        let mut body = serde_json::json!({
            "model": model_name,
            "messages": messages,
        });

        if let Some(temp) = temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }

        match ureq::post(&url)
            .timeout(HTTP_TIMEOUT)
            .set("Content-Type", "application/json")
            .send_json(&body)
        {
            Ok(resp) => match resp.into_json::<serde_json::Value>() {
                Ok(data) => Response::ok(data),
                Err(e) => {
                    Response::error("parse_error", &format!("Failed to parse response: {}", e))
                }
            },
            Err(ureq::Error::Transport(t)) => {
                let msg = t.to_string();
                if msg.contains("timed out") || msg.contains("Timeout") {
                    Response::error(
                        "timeout",
                        &format!("Inference timed out after {}s", HTTP_TIMEOUT.as_secs()),
                    )
                } else {
                    Response::error("transport_error", &msg)
                }
            }
            Err(ureq::Error::Status(status, resp)) => {
                let body_excerpt = resp.into_string().unwrap_or_default();
                let truncated = if body_excerpt.len() > 200 {
                    format!("{}...", &body_excerpt[..200])
                } else {
                    body_excerpt
                };
                Response::error("inference_error", &format!("{} {}", status, truncated))
            }
        }
    }

    fn status(&self) -> Response {
        Response::ok(serde_json::json!({
            "state": self.state,
            "port": if self.port > 0 { Some(self.port) } else { None },
            "endpoint": self.endpoint(),
            "model": self.config.as_ref().map(|c| c.model_path.display().to_string()),
            "restart_count": self.restart_count,
        }))
    }

    fn health(&mut self) -> Response {
        if self.state != ServerState::Ready {
            return Response::ok(serde_json::json!({
                "healthy": false,
                "state": self.state,
            }));
        }

        // Check subprocess alive
        if self.check_process_died() {
            self.state = ServerState::Cold;
            return Response::ok(serde_json::json!({
                "healthy": false,
                "state": self.state,
                "reason": "process_exited",
            }));
        }

        // Check HTTP health
        let health_url = format!("http://127.0.0.1:{}/health", self.port);
        match ureq::get(&health_url)
            .timeout(Duration::from_secs(5))
            .call()
        {
            Ok(resp) if resp.status() == 200 => Response::ok(serde_json::json!({
                "healthy": true,
                "state": self.state,
            })),
            _ => {
                self.state = ServerState::Error;
                Response::ok(serde_json::json!({
                    "healthy": false,
                    "state": self.state,
                    "reason": "health_check_failed",
                }))
            }
        }
    }

    fn list_models(&self) -> Response {
        let model_info = self.config.as_ref().map(|c| {
            serde_json::json!({
                "path": c.model_path.display().to_string(),
                "filename": c.model_path.file_name().map(|f| f.to_string_lossy().to_string()),
                "n_ctx": c.n_ctx,
                "n_gpu_layers": c.n_gpu_layers,
            })
        });

        Response::ok(serde_json::json!({
            "models": model_info.map(|m| vec![m]).unwrap_or_default(),
        }))
    }

    fn shutdown(&mut self) -> Response {
        if let Some(ref mut child) = self.child {
            eprintln!(
                "llama-provider: shutting down llama-server pid={}",
                child.id()
            );

            // Graceful SIGTERM on Unix, hard kill on Windows
            #[cfg(unix)]
            {
                let pid = child.id().to_string();
                let _ = Command::new("kill").args(["-TERM", &pid]).output();
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }

            // Wait for graceful shutdown
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if start.elapsed() > SHUTDOWN_GRACE {
                            eprintln!("llama-provider: force-killing llama-server");
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => break,
                }
            }
        }
        self.child = None;
        self.state = ServerState::Cold;
        self.restart_count = 0;

        Response::ok(serde_json::json!({"message": "llama-provider shutting down"}))
    }
}

impl Drop for LlamaProvider {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.shutdown();
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn bind_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind failed: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr failed: {}", e))?
        .port();
    // Drop listener — port is now free for llama-server to use.
    // Small TOCTOU window but acceptable since we bind 127.0.0.1.
    drop(listener);
    Ok(port)
}

fn bind_port(requested: Option<u16>) -> Result<u16, String> {
    if let Some(port) = requested {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| format!("requested port {} unavailable: {}", port, e))?;
        drop(listener);
        Ok(port)
    } else {
        bind_free_port()
    }
}

fn find_llama_binary() -> Option<PathBuf> {
    // Check ~/.local/share/elastos/bin/llama-server (may be symlink → llama-server-libs/)
    if let Some(home) = std::env::var_os("HOME") {
        let installed = PathBuf::from(home).join(".local/share/elastos/bin/llama-server");
        if installed.is_file() {
            // Resolve symlinks so we get the real path (needed for LD_LIBRARY_PATH)
            return Some(std::fs::canonicalize(&installed).unwrap_or(installed));
        }
    }

    // Check ELASTOS_DATA_DIR
    if let Ok(data_dir) = std::env::var("ELASTOS_DATA_DIR") {
        let path = PathBuf::from(data_dir).join("bin/llama-server");
        if path.is_file() {
            return Some(std::fs::canonicalize(&path).unwrap_or(path));
        }
    }

    // Check PATH
    if let Ok(output) = Command::new("which").arg("llama-server").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                let p = PathBuf::from(path);
                return Some(std::fs::canonicalize(&p).unwrap_or(p));
            }
        }
    }

    None
}

fn resolve_model_path(extra: &serde_json::Value) -> Option<PathBuf> {
    // Explicit override
    if let Some(path) = extra.get("model_path").and_then(|v| v.as_str()) {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    // Default model directory
    let model_dir = if let Ok(data_dir) = std::env::var("ELASTOS_DATA_DIR") {
        PathBuf::from(data_dir).join("models")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/elastos/models")
    } else {
        return None;
    };

    if !model_dir.is_dir() {
        return None;
    }

    // Check for model profile selection
    let env_profile = std::env::var("LLAMA_MODEL_PROFILE").ok();
    let profile = extra
        .get("model_profile")
        .and_then(|v| v.as_str())
        .or(env_profile.as_deref())
        .unwrap_or("default");

    // Map profile to filename pattern
    let preferred = match profile {
        "large" => vec!["Qwen3.5-9B", "Qwen3-8B", "9B", "8B"],
        "medium" => vec!["Qwen3.5-4B", "Qwen3-4B", "4B"],
        _ => vec!["Qwen3.5-0.8B", "Qwen3-0.8B", "0.8B"],
    };

    // Scan model directory for GGUF files matching profile
    if let Ok(entries) = std::fs::read_dir(&model_dir) {
        let mut gguf_files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|ext| ext == "gguf").unwrap_or(false))
            .collect();
        gguf_files.sort();

        // Try preferred patterns first
        for pattern in &preferred {
            if let Some(f) = gguf_files.iter().find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains(pattern))
                    .unwrap_or(false)
            }) {
                return Some(f.clone());
            }
        }

        // Fall back to any GGUF file
        if let Some(f) = gguf_files.first() {
            return Some(f.clone());
        }
    }

    None
}

// ── Main loop ───────────────────────────────────────────────────────

fn main() {
    eprintln!("llama-provider: starting v{}", PROVIDER_VERSION);
    let mut provider = LlamaProvider::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("llama-provider: stdin error: {}", e);
                break;
            }
        };
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let response = Response::error("parse_error", &e.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };

        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);

        let json = serde_json::to_string(&response).unwrap();
        writeln!(stdout, "{}", json).unwrap();
        stdout.flush().unwrap();

        if is_shutdown {
            break;
        }
    }

    // Ensure subprocess is cleaned up
    provider.shutdown();
    eprintln!("llama-provider: exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_free_port() {
        match bind_free_port() {
            Ok(port) => assert!(port > 0),
            Err(e) => eprintln!("Skipping bind_free_port test (restricted env): {}", e),
        }
    }

    #[test]
    fn test_bind_requested_port() {
        match bind_free_port() {
            Ok(free) => {
                let port = bind_port(Some(free)).expect("expected fixed port bind to succeed");
                assert_eq!(port, free);
            }
            Err(e) => eprintln!("Skipping bind_requested_port test (restricted env): {}", e),
        }
    }

    #[test]
    fn test_init_no_binary() {
        // With no binary or model installed, init should return error
        let mut provider = LlamaProvider::new();
        // Override HOME to ensure we don't find anything
        std::env::set_var("HOME", "/nonexistent");
        let resp = provider.handle(Request::Init {
            config: serde_json::json!({}),
        });
        match resp {
            Response::Error { code, .. } => {
                assert!(
                    code == "binary_not_found" || code == "model_not_found",
                    "Expected binary_not_found or model_not_found, got: {}",
                    code
                );
            }
            Response::Ok { .. } => {
                // If binary happens to be in PATH, that's ok too
            }
        }
    }

    #[test]
    fn test_status_cold() {
        let mut provider = LlamaProvider::new();
        let resp = provider.handle(Request::Status);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["state"], "cold");
                assert!(d["port"].is_null() || d["port"] == 0);
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_health_cold() {
        let mut provider = LlamaProvider::new();
        let resp = provider.handle(Request::Health);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["healthy"], false);
                assert_eq!(d["state"], "cold");
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_list_models_empty() {
        let mut provider = LlamaProvider::new();
        let resp = provider.handle(Request::ListModels);
        match resp {
            Response::Ok { data: Some(d) } => {
                let models = d["models"].as_array().unwrap();
                assert!(models.is_empty());
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_shutdown_no_child() {
        let mut provider = LlamaProvider::new();
        let resp = provider.handle(Request::Shutdown);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert!(d["message"].as_str().unwrap().contains("shutting down"));
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_chat_without_init() {
        let mut provider = LlamaProvider::new();
        let resp = provider.handle(Request::ChatCompletions {
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: None,
            temperature: None,
            max_tokens: None,
        });
        // Should error since not initialized (or succeed if binary is in PATH)
        if let Response::Error { code, .. } = resp {
            assert!(
                code == "start_failed" || code == "binary_not_found",
                "Expected start_failed, got: {}",
                code
            );
        }
    }

    #[test]
    fn test_server_state_serialization() {
        assert_eq!(
            serde_json::to_string(&ServerState::Cold).unwrap(),
            "\"cold\""
        );
        assert_eq!(
            serde_json::to_string(&ServerState::Starting).unwrap(),
            "\"starting\""
        );
        assert_eq!(
            serde_json::to_string(&ServerState::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&ServerState::Error).unwrap(),
            "\"error\""
        );
    }
}
