//! ElastOS AI Provider Capsule
//!
//! Routes LLM requests to supported backends:
//! - local OpenAI-compatible llama-server
//! - Venice cloud
//! - local Codex CLI using existing Codex auth/session state
//! Wire protocol: line-delimited JSON over stdin/stdout.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    ChatCompletions {
        backend: String,
        messages: Vec<Message>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        temperature: Option<f64>,
        #[serde(default)]
        max_tokens: Option<u64>,
    },
    ListBackends,
    Ping,
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

#[derive(Debug, Clone)]
enum BackendTransport {
    Http {
        api_url: String,
        api_key: Option<String>,
    },
    CodexCli {
        command: String,
    },
}

#[derive(Debug, Clone)]
struct BackendConfig {
    transport: BackendTransport,
    default_model: String,
}

impl BackendConfig {
    fn has_api_key(&self) -> bool {
        match &self.transport {
            BackendTransport::Http { api_key, .. } => api_key.is_some(),
            BackendTransport::CodexCli { .. } => false,
        }
    }

    fn transport_name(&self) -> &'static str {
        match self.transport {
            BackendTransport::Http { .. } => "http",
            BackendTransport::CodexCli { .. } => "codex_cli",
        }
    }

    fn target_label(&self) -> String {
        match &self.transport {
            BackendTransport::Http { api_url, .. } => api_url.clone(),
            BackendTransport::CodexCli { command } => command.clone(),
        }
    }
}

struct AiProvider {
    backends: HashMap<String, BackendConfig>,
}

/// Default backend URLs/models.
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434/v1/chat/completions";
const DEFAULT_OLLAMA_MODEL: &str = "qwen3.5:0.8b";
const DEFAULT_VENICE_URL: &str = "https://api.venice.ai/api/v1/chat/completions";
const DEFAULT_VENICE_MODEL: &str = "llama-3.3-70b";
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_CODEX_MODEL: &str = "config-default";

impl AiProvider {
    fn new() -> Self {
        Self::with_env(
            std::env::var("OLLAMA_URL").ok(),
            std::env::var("OLLAMA_MODEL").ok(),
            std::env::var("VENICE_API_KEY").ok(),
            std::env::var("VENICE_MODEL").ok(),
            std::env::var("CODEX_BIN").ok(),
            std::env::var("CODEX_MODEL").ok(),
        )
    }

    fn with_env(
        ollama_url: Option<String>,
        ollama_model: Option<String>,
        venice_api_key: Option<String>,
        venice_model: Option<String>,
        codex_bin: Option<String>,
        codex_model: Option<String>,
    ) -> Self {
        let mut backends = HashMap::new();

        backends.insert(
            "local".to_string(),
            BackendConfig {
                transport: BackendTransport::Http {
                    api_url: ollama_url.unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string()),
                    api_key: None,
                },
                default_model: ollama_model.unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string()),
            },
        );

        backends.insert(
            "venice".to_string(),
            BackendConfig {
                transport: BackendTransport::Http {
                    api_url: DEFAULT_VENICE_URL.to_string(),
                    api_key: venice_api_key,
                },
                default_model: venice_model.unwrap_or_else(|| DEFAULT_VENICE_MODEL.to_string()),
            },
        );

        backends.insert(
            "codex".to_string(),
            BackendConfig {
                transport: BackendTransport::CodexCli {
                    command: codex_bin.unwrap_or_else(|| DEFAULT_CODEX_BIN.to_string()),
                },
                default_model: codex_model.unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string()),
            },
        );

        Self { backends }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::ChatCompletions {
                backend,
                messages,
                model,
                temperature,
                max_tokens,
            } => self.chat_completions(&backend, messages, model, temperature, max_tokens),
            Request::ListBackends => self.list_backends(),
            Request::Ping => Response::ok(serde_json::json!({"pong": true})),
            Request::Shutdown => {
                Response::ok(serde_json::json!({"message": "ai-provider shutting down"}))
            }
        }
    }

    fn init(&mut self, config: serde_json::Value) -> Response {
        // Runtime sends ProviderConfig with our settings in the "extra" field.
        // Support both top-level keys (direct init) and nested under "extra" (runtime bridge).
        let extra = config.get("extra").unwrap_or(&config);

        // Apply init config overrides (init > env > defaults)
        // Priority: local_url (llama-provider) > ollama_url > env > defaults
        if let Some(local) = self.backends.get_mut("local") {
            if let BackendTransport::Http { api_url, .. } = &mut local.transport {
                if let Some(url) = extra.get("local_url").and_then(|v| v.as_str()) {
                    *api_url = url.to_string();
                } else if let Some(url) = extra.get("ollama_url").and_then(|v| v.as_str()) {
                    *api_url = url.to_string();
                }
            }
            if let Some(model) = extra.get("ollama_model").and_then(|v| v.as_str()) {
                local.default_model = model.to_string();
            }
        }
        if let Some(venice) = self.backends.get_mut("venice") {
            if let BackendTransport::Http { api_key, .. } = &mut venice.transport {
                if let Some(key) = extra.get("venice_api_key").and_then(|v| v.as_str()) {
                    *api_key = Some(key.to_string());
                }
            }
            if let Some(model) = extra.get("venice_model").and_then(|v| v.as_str()) {
                venice.default_model = model.to_string();
            }
        }
        if let Some(codex) = self.backends.get_mut("codex") {
            if let BackendTransport::CodexCli { command } = &mut codex.transport {
                if let Some(bin) = extra.get("codex_bin").and_then(|v| v.as_str()) {
                    *command = bin.to_string();
                }
            }
            if let Some(model) = extra.get("codex_model").and_then(|v| v.as_str()) {
                codex.default_model = model.to_string();
            }
        }

        let backend_list: Vec<serde_json::Value> = self
            .backends
            .iter()
            .map(|(name, cfg)| {
                serde_json::json!({
                    "name": name,
                    "default_model": cfg.default_model,
                    "has_api_key": cfg.has_api_key(),
                    "transport": cfg.transport_name(),
                })
            })
            .collect();

        Response::ok(serde_json::json!({
            "protocol_version": "1.0",
            "provider": "ai-provider",
            "backends": backend_list,
        }))
    }

    fn chat_completions(
        &self,
        backend_name: &str,
        messages: Vec<Message>,
        model: Option<String>,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Response {
        let backend = match self.backends.get(backend_name) {
            Some(b) => b,
            None => {
                return Response::error(
                    "unknown_backend",
                    &format!(
                        "Unknown backend '{}'. Use list_backends to see available backends.",
                        backend_name
                    ),
                );
            }
        };

        // Venice requires an API key
        if backend_name == "venice" && !backend.has_api_key() {
            return Response::error(
                "missing_api_key",
                "Set VENICE_API_KEY env var or pass venice_api_key in init config",
            );
        }

        match &backend.transport {
            BackendTransport::Http { api_url, api_key } => {
                let model_name = model.unwrap_or_else(|| backend.default_model.clone());
                self.chat_http_backend(
                    backend_name,
                    api_url,
                    api_key.as_deref(),
                    model_name,
                    messages,
                    temperature,
                    max_tokens,
                )
            }
            BackendTransport::CodexCli { command } => {
                let model_override = model.as_deref().or_else(|| {
                    if backend.default_model == DEFAULT_CODEX_MODEL {
                        None
                    } else {
                        Some(backend.default_model.as_str())
                    }
                });
                self.chat_codex_backend(command, model_override, messages)
            }
        }
    }

    fn list_backends(&self) -> Response {
        let backends: Vec<serde_json::Value> = self
            .backends
            .iter()
            .map(|(name, cfg)| {
                serde_json::json!({
                    "name": name,
                    "api_url": cfg.target_label(),
                    "default_model": cfg.default_model,
                    "has_api_key": cfg.has_api_key(),
                    "transport": cfg.transport_name(),
                })
            })
            .collect();

        Response::ok(serde_json::json!({"backends": backends}))
    }

    fn chat_http_backend(
        &self,
        backend_name: &str,
        api_url: &str,
        api_key: Option<&str>,
        model_name: String,
        messages: Vec<Message>,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Response {
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

        let mut req = ureq::post(api_url)
            .timeout(HTTP_TIMEOUT)
            .set("Content-Type", "application/json");

        if let Some(key) = api_key {
            req = req.set("Authorization", &format!("Bearer {}", key));
        }

        let result = req.send_json(&body);

        match result {
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
                        &format!("Request timed out after {}s", HTTP_TIMEOUT.as_secs()),
                    )
                } else if backend_name == "local"
                    && (msg.contains("Connection refused") || msg.contains("connect"))
                {
                    Response::error(
                        "local_unavailable",
                        &format!(
                            "Local AI backend unavailable ({}). Run: elastos setup --with llama-server",
                            msg
                        ),
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
                if backend_name == "local" && status == 503 {
                    Response::error(
                        "model_loading",
                        "Local AI model is still loading. First request may take a few minutes.",
                    )
                } else {
                    Response::error("api_error", &format!("{} {}", status, truncated))
                }
            }
        }
    }

    fn chat_codex_backend(
        &self,
        command: &str,
        model_override: Option<&str>,
        messages: Vec<Message>,
    ) -> Response {
        let prompt = build_codex_prompt(&messages);
        let temp_path = unique_temp_file_path("elastos-codex");

        let mut cmd = Command::new(command);
        cmd.arg("exec")
            .arg("--skip-git-repo-check")
            .arg("--ephemeral")
            .arg("--color")
            .arg("never")
            .arg("--sandbox")
            .arg("read-only")
            .arg("--output-last-message")
            .arg(&temp_path)
            .arg("-C")
            .arg(std::env::temp_dir());
        if let Some(model) = model_override {
            cmd.arg("-m").arg(model);
        }
        cmd.arg(prompt)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Response::error(
                    "codex_unavailable",
                    "Codex CLI not found. Install Codex CLI and run `codex login` first.",
                );
            }
            Err(e) => {
                return Response::error(
                    "codex_unavailable",
                    &format!("Failed to start Codex CLI: {}", e),
                );
            }
        };

        let deadline = std::time::Instant::now() + HTTP_TIMEOUT;
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        timed_out = true;
                        let _ = child.kill();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let _ = fs::remove_file(&temp_path);
                    return Response::error(
                        "codex_error",
                        &format!("Failed while waiting for Codex CLI: {}", e),
                    );
                }
            }
        }

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(e) => {
                let _ = fs::remove_file(&temp_path);
                return Response::error(
                    "codex_error",
                    &format!("Failed to collect Codex CLI output: {}", e),
                );
            }
        };

        if timed_out {
            let _ = fs::remove_file(&temp_path);
            return Response::error(
                "timeout",
                &format!("Codex CLI timed out after {}s", HTTP_TIMEOUT.as_secs()),
            );
        }

        if !output.status.success() {
            let _ = fs::remove_file(&temp_path);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = if stderr.contains("login") || stderr.contains("auth") {
                "Codex CLI is not authenticated. Run `codex login` and try again.".to_string()
            } else {
                format!(
                    "Codex CLI failed (exit {}). Check local Codex auth/config and try again.",
                    output.status.code().unwrap_or(-1)
                )
            };
            return Response::error("codex_error", &message);
        }

        let text = match fs::read_to_string(&temp_path) {
            Ok(text) => text.trim().to_string(),
            Err(e) => {
                let _ = fs::remove_file(&temp_path);
                return Response::error(
                    "codex_error",
                    &format!("Codex CLI did not produce a readable final message: {}", e),
                );
            }
        };
        let _ = fs::remove_file(&temp_path);

        if text.is_empty() {
            return Response::error("empty_response", "Codex CLI returned an empty response");
        }

        Response::ok(serde_json::json!({
            "model": model_override.unwrap_or(DEFAULT_CODEX_MODEL),
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": text,
                },
                "finish_reason": "stop",
            }]
        }))
    }
}

fn build_codex_prompt(messages: &[Message]) -> String {
    let mut prompt = String::from(
        "You are the LLM backend for an ElastOS chat agent.\n\
         Return only the final assistant reply text that should be sent back to chat.\n\
         Keep the reply concise unless the user explicitly asks for more.\n\
         Do not include markdown fences, role labels, or explanations of your internal process.\n\
         No tool use is required for this task.\n\n\
         Conversation:\n",
    );

    for message in messages {
        prompt.push_str(&message.role.to_uppercase());
        prompt.push_str(":\n");
        prompt.push_str(&message.content);
        prompt.push_str("\n\n");
    }

    prompt.push_str("ASSISTANT:\n");
    prompt
}

fn unique_temp_file_path(prefix: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("{}-{}-{}.txt", prefix, std::process::id(), stamp))
}

fn main() {
    eprintln!("ai-provider: starting v{}", PROVIDER_VERSION);
    let mut provider = AiProvider::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error reading stdin: {}", e);
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
    eprintln!("ai-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_provider() -> AiProvider {
        AiProvider::with_env(None, None, None, None, None, None)
    }

    fn init_provider(config: serde_json::Value) -> AiProvider {
        let mut provider = default_provider();
        provider.handle(Request::Init { config });
        provider
    }

    #[test]
    fn test_init_returns_backends() {
        let mut provider = AiProvider::new();
        let resp = provider.handle(Request::Init {
            config: serde_json::json!({}),
        });

        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["protocol_version"], "1.0");
                assert_eq!(d["provider"], "ai-provider");
                let backends = d["backends"].as_array().unwrap();
                assert_eq!(backends.len(), 3);
                let names: Vec<&str> = backends
                    .iter()
                    .map(|b| b["name"].as_str().unwrap())
                    .collect();
                assert!(names.contains(&"local"));
                assert!(names.contains(&"venice"));
                assert!(names.contains(&"codex"));
            }
            other => panic!("Expected Ok with data, got {:?}", other),
        }
    }

    #[test]
    fn test_init_with_venice_key() {
        let provider = init_provider(serde_json::json!({"venice_api_key": "sk-test-123"}));
        let venice = provider.backends.get("venice").unwrap();
        match &venice.transport {
            BackendTransport::Http { api_key, .. } => {
                assert_eq!(api_key.as_deref(), Some("sk-test-123"));
            }
            other => panic!("Expected HTTP transport, got {:?}", other),
        }
    }

    #[test]
    fn test_list_backends() {
        let mut provider = AiProvider::new();
        let resp = provider.handle(Request::ListBackends);

        match resp {
            Response::Ok { data: Some(d) } => {
                let backends = d["backends"].as_array().unwrap();
                assert_eq!(backends.len(), 3);
                for b in backends {
                    assert!(b["name"].is_string());
                    assert!(b["api_url"].is_string());
                    assert!(b["default_model"].is_string());
                    assert!(b["has_api_key"].is_boolean());
                    assert!(b["transport"].is_string());
                }
            }
            other => panic!("Expected Ok with data, got {:?}", other),
        }
    }

    #[test]
    fn test_chat_unknown_backend_errors() {
        let mut provider = AiProvider::new();
        let resp = provider.handle(Request::ChatCompletions {
            backend: "nonexistent".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: None,
            temperature: None,
            max_tokens: None,
        });

        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, "unknown_backend");
                assert!(message.contains("nonexistent"));
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    #[test]
    fn test_chat_missing_key_errors() {
        // Ensure venice has no key
        let mut provider = AiProvider::new();
        if let BackendTransport::Http { api_key, .. } =
            &mut provider.backends.get_mut("venice").unwrap().transport
        {
            *api_key = None;
        } else {
            panic!("Expected HTTP transport for venice backend");
        }

        let resp = provider.handle(Request::ChatCompletions {
            backend: "venice".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: None,
            temperature: None,
            max_tokens: None,
        });

        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, "missing_api_key");
                assert!(message.contains("VENICE_API_KEY"));
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    #[test]
    fn test_chat_local_backend() {
        // Host-independent: any response is valid (no Ollama, Ollama up, model missing).
        // We only verify the provider doesn't panic and returns a well-formed Response.
        let mut provider = AiProvider::new();
        let resp = provider.handle(Request::ChatCompletions {
            backend: "local".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: None,
            temperature: None,
            max_tokens: None,
        });

        match resp {
            Response::Error { code, message } => {
                assert!(!code.is_empty(), "Error code must not be empty");
                assert!(!message.is_empty(), "Error message must not be empty");
            }
            Response::Ok { .. } => {
                // Ollama is running locally — valid response
            }
        }
    }

    #[test]
    fn test_ping() {
        let mut provider = AiProvider::new();
        let resp = provider.handle(Request::Ping);

        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["pong"], true);
            }
            other => panic!("Expected Ok with pong, got {:?}", other),
        }
    }

    #[test]
    fn test_default_backend_urls() {
        let provider = default_provider();
        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.target_label(), DEFAULT_OLLAMA_URL);
        assert_eq!(local.default_model, DEFAULT_OLLAMA_MODEL);

        let venice = provider.backends.get("venice").unwrap();
        assert_eq!(venice.default_model, DEFAULT_VENICE_MODEL);
        let codex = provider.backends.get("codex").unwrap();
        assert_eq!(codex.default_model, DEFAULT_CODEX_MODEL);
    }

    #[test]
    fn test_env_var_overrides() {
        let provider = AiProvider::with_env(
            Some("http://custom:1234/v1/chat/completions".to_string()),
            Some("llama3:8b".to_string()),
            None,
            Some("custom-venice-model".to_string()),
            Some("/usr/local/bin/codex".to_string()),
            Some("gpt-5.4-mini".to_string()),
        );

        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.target_label(), "http://custom:1234/v1/chat/completions");
        assert_eq!(local.default_model, "llama3:8b");

        let venice = provider.backends.get("venice").unwrap();
        assert_eq!(venice.default_model, "custom-venice-model");

        let codex = provider.backends.get("codex").unwrap();
        assert_eq!(codex.target_label(), "/usr/local/bin/codex");
        assert_eq!(codex.default_model, "gpt-5.4-mini");
    }

    #[test]
    fn test_init_config_overrides_env() {
        let mut provider = AiProvider::with_env(
            Some("http://env-url:1234/v1/chat/completions".to_string()),
            Some("env-model".to_string()),
            None,
            None,
            None,
            None,
        );

        provider.handle(Request::Init {
            config: serde_json::json!({
                "ollama_url": "http://init-url:5678/v1/chat/completions",
                "ollama_model": "init-model",
                "venice_model": "init-venice-model",
            }),
        });

        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.target_label(), "http://init-url:5678/v1/chat/completions");
        assert_eq!(local.default_model, "init-model");

        let venice = provider.backends.get("venice").unwrap();
        assert_eq!(venice.default_model, "init-venice-model");
    }

    #[test]
    fn test_missing_init_fields_keep_existing() {
        let mut provider =
            AiProvider::with_env(None, Some("env-model".to_string()), None, None, None, None);

        // Init with empty config — env value should survive
        provider.handle(Request::Init {
            config: serde_json::json!({}),
        });

        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.default_model, "env-model");
    }

    #[test]
    fn test_local_url_overrides_ollama() {
        let mut provider = AiProvider::with_env(
            Some("http://ollama:11434/v1/chat/completions".to_string()),
            None,
            None,
            None,
            None,
            None,
        );

        // local_url from llama-provider should take priority over ollama_url
        provider.handle(Request::Init {
            config: serde_json::json!({
                "local_url": "http://127.0.0.1:54321/v1/chat/completions",
                "ollama_url": "http://should-not-use:11434/v1/chat/completions",
            }),
        });

        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.target_label(), "http://127.0.0.1:54321/v1/chat/completions");
    }

    #[test]
    fn test_ollama_url_fallback_when_no_local_url() {
        let mut provider = default_provider();

        provider.handle(Request::Init {
            config: serde_json::json!({
                "ollama_url": "http://custom-ollama:11434/v1/chat/completions",
            }),
        });

        let local = provider.backends.get("local").unwrap();
        assert_eq!(local.target_label(), "http://custom-ollama:11434/v1/chat/completions");
    }

    #[test]
    fn test_init_codex_overrides() {
        let mut provider = AiProvider::with_env(
            None,
            None,
            None,
            None,
            Some("/usr/bin/codex".to_string()),
            Some("gpt-5.4".to_string()),
        );

        provider.handle(Request::Init {
            config: serde_json::json!({
                "codex_bin": "/custom/bin/codex",
                "codex_model": "gpt-5.4-mini",
            }),
        });

        let codex = provider.backends.get("codex").unwrap();
        assert_eq!(codex.target_label(), "/custom/bin/codex");
        assert_eq!(codex.default_model, "gpt-5.4-mini");
    }

    #[test]
    fn test_codex_missing_binary_errors() {
        let mut provider = AiProvider::with_env(
            None,
            None,
            None,
            None,
            Some("/definitely/missing/codex".to_string()),
            None,
        );

        let resp = provider.handle(Request::ChatCompletions {
            backend: "codex".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: None,
            temperature: None,
            max_tokens: None,
        });

        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, "codex_unavailable");
                assert!(message.contains("Codex CLI"));
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    #[test]
    fn test_build_codex_prompt_includes_roles() {
        let prompt = build_codex_prompt(&[
            Message {
                role: "system".to_string(),
                content: "System prompt".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
        ]);

        assert!(prompt.contains("SYSTEM:\nSystem prompt"));
        assert!(prompt.contains("USER:\nHello"));
        assert!(prompt.ends_with("ASSISTANT:\n"));
    }

    #[test]
    fn test_message_serialization() {
        let msg = Message {
            role: "user".to_string(),
            content: "Hello, world!".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let roundtrip: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.role, "user");
        assert_eq!(roundtrip.content, "Hello, world!");
    }
}
