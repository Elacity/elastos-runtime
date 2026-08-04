//! Home Agent chat streaming (dogfood).
//!
//! OpenAI-compat SSE proxy for the opaque home-gui shell. Authority stays on
//! the gateway (home-gui launch token). Upstream URLs come from operator env /
//! allowlisted Sparks pairs — never from a free-form client URL (SSRF-closed).

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_lite::stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::*;

const AGENT_STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const AGENT_STREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_AGENT_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_PAIR_A: &str = "http://192.168.1.147:8888/v1/chat/completions";
const DEFAULT_PAIR_B: &str = "http://192.168.1.145:8888/v1/chat/completions";
const DEFAULT_PAIR_A_LABEL: &str = "Sparks pair A";
const DEFAULT_PAIR_B_LABEL: &str = "Sparks pair B";
const DEFAULT_MAX_TOKENS: u64 = 2048;
const MAX_MESSAGES: usize = 48;
const MAX_MESSAGE_CHARS: usize = 16_000;
const MAX_MODEL_CHARS: usize = 128;
const MAX_LABEL_CHARS: usize = 64;
const MAX_URL_CHARS: usize = 512;
const AGENT_OPENAI_CONFIG_SCHEMA: &str = "elastos.home.agent.openai-compat/v1";

#[derive(Debug, Deserialize)]
pub(super) struct HomeAgentChatStreamBody {
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    temperature: Option<f64>,
    /// `"a"` (default) or `"b"` — mapped server-side to allowlisted upstreams.
    #[serde(default)]
    pair: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentPairConfig {
    url: String,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentOpenAiConfig {
    schema: String,
    model: String,
    pairs: AgentPairsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentPairsConfig {
    a: AgentPairConfig,
    b: AgentPairConfig,
}

#[derive(Debug, Deserialize)]
pub(super) struct HomeAgentBackendsPutBody {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    pair_a_url: Option<String>,
    #[serde(default)]
    pair_a_label: Option<String>,
    #[serde(default)]
    pair_b_url: Option<String>,
    #[serde(default)]
    pair_b_label: Option<String>,
}

fn agent_openai_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config/home-agent-openai.json")
}

fn env_or(default: &str, key: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn default_agent_openai_config() -> AgentOpenAiConfig {
    AgentOpenAiConfig {
        schema: AGENT_OPENAI_CONFIG_SCHEMA.to_string(),
        model: env_or(DEFAULT_AGENT_MODEL, "OLLAMA_MODEL"),
        pairs: AgentPairsConfig {
            a: AgentPairConfig {
                url: env_or(DEFAULT_PAIR_A, "OLLAMA_URL"),
                label: DEFAULT_PAIR_A_LABEL.to_string(),
            },
            b: AgentPairConfig {
                url: env_or(DEFAULT_PAIR_B, "OLLAMA_URL_B"),
                label: DEFAULT_PAIR_B_LABEL.to_string(),
            },
        },
    }
}

fn load_agent_openai_config(data_dir: &Path) -> AgentOpenAiConfig {
    let path = agent_openai_config_path(data_dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return default_agent_openai_config();
    };
    let Ok(parsed) = serde_json::from_slice::<AgentOpenAiConfig>(&bytes) else {
        return default_agent_openai_config();
    };
    if parsed.schema != AGENT_OPENAI_CONFIG_SCHEMA {
        return default_agent_openai_config();
    }
    let mut config = parsed;
    if config.model.trim().is_empty() {
        config.model = default_agent_openai_config().model;
    }
    if config.pairs.a.url.trim().is_empty() {
        config.pairs.a = default_agent_openai_config().pairs.a;
    }
    if config.pairs.b.url.trim().is_empty() {
        config.pairs.b = default_agent_openai_config().pairs.b;
    }
    if config.pairs.a.label.trim().is_empty() {
        config.pairs.a.label = DEFAULT_PAIR_A_LABEL.to_string();
    }
    if config.pairs.b.label.trim().is_empty() {
        config.pairs.b.label = DEFAULT_PAIR_B_LABEL.to_string();
    }
    config
}

fn store_agent_openai_config(data_dir: &Path, config: &AgentOpenAiConfig) -> anyhow::Result<()> {
    let path = agent_openai_config_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(config)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn validate_openai_chat_url(url: &str) -> anyhow::Result<()> {
    if url.len() > MAX_URL_CHARS {
        anyhow::bail!("upstream URL is too long");
    }
    let parsed = url::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid upstream URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => anyhow::bail!("upstream URL must be http(s)"),
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("upstream URL missing host");
    }
    if parsed.username() != "" || parsed.password().is_some() {
        anyhow::bail!("upstream URL must not include credentials");
    }
    let path = parsed.path().trim_end_matches('/');
    if !path.ends_with("/v1/chat/completions") && !path.ends_with("/chat/completions") {
        anyhow::bail!("upstream URL must be an OpenAI-compat chat completions endpoint");
    }
    Ok(())
}

fn normalize_model_id(value: &str) -> anyhow::Result<String> {
    let model = value.trim();
    if model.is_empty() {
        anyhow::bail!("model is required");
    }
    if model.len() > MAX_MODEL_CHARS {
        anyhow::bail!("model id is too long");
    }
    if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
    {
        anyhow::bail!("model id has unsupported characters");
    }
    Ok(model.to_string())
}

fn normalize_label(value: &str, fallback: &str) -> String {
    let label = value.trim();
    if label.is_empty() {
        return fallback.to_string();
    }
    label.chars().take(MAX_LABEL_CHARS).collect()
}

fn pair_upstream(data_dir: &Path, pair: Option<&str>) -> anyhow::Result<(String, String)> {
    let config = load_agent_openai_config(data_dir);
    let key = pair.unwrap_or("a").trim().to_ascii_lowercase();
    let url = match key.as_str() {
        "b" | "pair-b" | "pair_b" => config.pairs.b.url,
        _ => config.pairs.a.url,
    };
    validate_openai_chat_url(&url)?;
    Ok((url, config.model))
}

fn backends_json(data_dir: &Path) -> Value {
    let config = load_agent_openai_config(data_dir);
    let from_file = agent_openai_config_path(data_dir).is_file();
    json!({
        "schema": "elastos.home.agent.backends/v1",
        "source": if from_file { "file" } else { "env-default" },
        "model": config.model,
        "stream": "openai-compat-sse",
        "pairs": [
            {
                "id": "a",
                "label": config.pairs.a.label,
                "url": config.pairs.a.url,
            },
            {
                "id": "b",
                "label": config.pairs.b.label,
                "url": config.pairs.b.url,
            }
        ],
        "notes": [
            "Browser never fetches upstream URLs directly.",
            "PUT re-validates OpenAI-compat path before persist.",
            "Venice/Codex appear only when wired as OpenAI-compat pairs."
        ]
    })
}

fn require_home_gui_launch(state: &GatewayState, headers: &HeaderMap) -> Result<(), String> {
    require_home_launch_token_for_any_context(&state.data_dir, headers, &[HOME_GUI_SHELL_ID])
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// GET /api/apps/home/agent/backends
pub(super) async fn home_agent_backends_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(message) = require_home_gui_launch(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": message,
            })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(backends_json(&state.data_dir))).into_response()
}

/// PUT /api/apps/home/agent/backends
pub(super) async fn home_agent_backends_put(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<HomeAgentBackendsPutBody>,
) -> Response {
    if let Err(message) = require_home_gui_launch(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": message,
            })),
        )
            .into_response();
    }

    let mut config = load_agent_openai_config(&state.data_dir);
    if let Some(model) = body.model.as_deref() {
        match normalize_model_id(model) {
            Ok(model) => config.model = model,
            Err(err) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "status": "error",
                        "code": "invalid_model",
                        "message": err.to_string(),
                    })),
                )
                    .into_response();
            }
        }
    }
    if let Some(url) = body.pair_a_url.as_deref() {
        let url = url.trim();
        if let Err(err) = validate_openai_chat_url(url) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_pair_a_url",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
        config.pairs.a.url = url.to_string();
    }
    if let Some(label) = body.pair_a_label.as_deref() {
        config.pairs.a.label = normalize_label(label, DEFAULT_PAIR_A_LABEL);
    }
    if let Some(url) = body.pair_b_url.as_deref() {
        let url = url.trim();
        if let Err(err) = validate_openai_chat_url(url) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_pair_b_url",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
        config.pairs.b.url = url.to_string();
    }
    if let Some(label) = body.pair_b_label.as_deref() {
        config.pairs.b.label = normalize_label(label, DEFAULT_PAIR_B_LABEL);
    }
    config.schema = AGENT_OPENAI_CONFIG_SCHEMA.to_string();

    if let Err(err) = store_agent_openai_config(&state.data_dir, &config) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "code": "persist_failed",
                "message": err.to_string(),
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "backends": backends_json(&state.data_dir),
        })),
    )
        .into_response()
}

fn sanitize_messages(raw: Vec<Value>) -> anyhow::Result<Vec<Value>> {
    if raw.is_empty() {
        anyhow::bail!("messages are required");
    }
    if raw.len() > MAX_MESSAGES {
        anyhow::bail!("too many messages");
    }
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let role = item
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !matches!(role, "system" | "user" | "assistant") {
            anyhow::bail!("unsupported message role");
        }
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(MAX_MESSAGE_CHARS)
            .collect::<String>();
        out.push(json!({ "role": role, "content": content }));
    }
    Ok(out)
}

/// POST /api/apps/home/agent/chat/stream
pub(super) async fn home_agent_chat_stream(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<HomeAgentChatStreamBody>,
) -> Response {
    if let Err(err) =
        require_home_launch_token_for_any_context(&state.data_dir, &headers, &[HOME_GUI_SHELL_ID])
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err.to_string(),
            })),
        )
            .into_response();
    }

    let messages = match sanitize_messages(body.messages) {
        Ok(messages) => messages,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_request",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let (api_url, model) = match pair_upstream(&state.data_dir, body.pair.as_deref()) {
        Ok(pair) => pair,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "error",
                    "code": "upstream_config",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };
    /* Model id is gateway-owned (Settings / env / config file). Client `model`
       is ignored so the browser cannot steer the upstream outside the allowlist. */
    let _ignored_client_model = body.model;
    let max_tokens = body.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS).clamp(16, 8192);
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "max_tokens": max_tokens,
    });
    if let Some(temperature) = body.temperature {
        payload["temperature"] = json!(temperature.clamp(0.0, 2.0));
    }

    let client = match reqwest::Client::builder()
        .connect_timeout(AGENT_STREAM_CONNECT_TIMEOUT)
        .timeout(AGENT_STREAM_TOTAL_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "code": "client_build",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let upstream = match client
        .post(&api_url)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "text/event-stream")
        .json(&payload)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "error",
                    "code": "upstream_connect",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    if !status.is_success() {
        let detail = upstream.text().await.unwrap_or_default();
        let truncated = if detail.len() > 400 {
            format!("{}…", &detail[..400])
        } else {
            detail
        };
        return (
            status,
            Json(json!({
                "status": "error",
                "code": "upstream_http",
                "message": truncated,
            })),
        )
            .into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut byte_stream = upstream.bytes_stream();
        while let Some(item) = byte_stream.next().await {
            let mapped = item.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));
            if tx.send(mapped).await.is_err() {
                /* Client dropped the body — abort upstream by dropping the stream. */
                break;
            }
        }
    });

    let body_stream = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(item) => Some((item, rx)),
            None => None,
        }
    });

    let mut response = Response::new(Body::from_stream(body_stream));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    response
}
