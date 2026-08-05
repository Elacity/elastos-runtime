//! Home Agent chat streaming (dogfood).
//!
//! OpenAI-compat SSE proxy for the opaque home-gui shell. Authority stays on
//! the gateway (home-gui launch token). Upstream URLs come from operator env /
//! allowlisted Sparks pairs — never from a free-form client URL (SSRF-closed).

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::http::header::AUTHORIZATION;
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

/* ── Wave 5.01 — On-Home Library.read via Inbox capability (gateway-mediated) ── */

const AGENT_LIBRARY_READ_SCHEMA: &str = "elastos.home.agent.library-read/v1";
const AGENT_LIBRARY_READ_REASON: &str = "home-agent:library.read";
const MAX_LIBRARY_LIST_NAMES: usize = 48;
const MAX_LIBRARY_RESULT_CHARS: usize = 12_000;
const MAX_LIBRARY_JOBS: usize = 64;
const MAX_LIBRARY_EXTRACT_BYTES: usize = 200_000;
const MAX_LIBRARY_EXTRACT_CHARS: usize = 24_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentLibraryReadJob {
    request_id: String,
    principal_id: String,
    resource: String,
    created_at: u64,
    /// pending | ready | denied | error
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentLibraryReadStore {
    schema: String,
    jobs: Vec<AgentLibraryReadJob>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HomeAgentLibraryReadBody {
    #[serde(default)]
    uri: Option<String>,
}

fn agent_library_read_store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config/home-agent-library-read.json")
}

fn load_agent_library_read_store(data_dir: &Path) -> AgentLibraryReadStore {
    let path = agent_library_read_store_path(data_dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return AgentLibraryReadStore {
            schema: AGENT_LIBRARY_READ_SCHEMA.to_string(),
            jobs: Vec::new(),
        };
    };
    let Ok(parsed) = serde_json::from_slice::<AgentLibraryReadStore>(&bytes) else {
        return AgentLibraryReadStore {
            schema: AGENT_LIBRARY_READ_SCHEMA.to_string(),
            jobs: Vec::new(),
        };
    };
    if parsed.schema != AGENT_LIBRARY_READ_SCHEMA {
        return AgentLibraryReadStore {
            schema: AGENT_LIBRARY_READ_SCHEMA.to_string(),
            jobs: Vec::new(),
        };
    }
    parsed
}

fn store_agent_library_read_store(data_dir: &Path, store: &AgentLibraryReadStore) -> anyhow::Result<()> {
    let path = agent_library_read_store_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(store)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn upsert_agent_library_read_job(data_dir: &Path, job: AgentLibraryReadJob) -> anyhow::Result<()> {
    let mut store = load_agent_library_read_store(data_dir);
    if let Some(existing) = store
        .jobs
        .iter_mut()
        .find(|row| row.request_id == job.request_id)
    {
        *existing = job;
    } else {
        store.jobs.push(job);
    }
    if store.jobs.len() > MAX_LIBRARY_JOBS {
        let skip = store.jobs.len() - MAX_LIBRARY_JOBS;
        store.jobs = store.jobs.split_off(skip);
    }
    store_agent_library_read_store(data_dir, &store)
}

pub(super) fn agent_library_read_job_exists(data_dir: &Path, request_id: &str) -> bool {
    load_agent_library_read_store(data_dir)
        .jobs
        .iter()
        .any(|job| job.request_id == request_id)
}

fn agent_desktop_resource(
    context: &HomeLaunchTokenContext,
    uri: Option<&str>,
) -> anyhow::Result<String> {
    let root = crate::auth::principal_localhost_root(&context.principal_id);
    let desktop = format!("{root}/Desktop");
    let Some(uri) = uri.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(desktop);
    };
    if uri.contains("..") || uri.contains('\\') {
        anyhow::bail!("invalid library uri");
    }
    if uri == desktop
        || (uri.starts_with(&desktop) && uri.as_bytes().get(desktop.len()) == Some(&b'/'))
    {
        return Ok(uri.to_string());
    }
    anyhow::bail!("library.read is scoped to Desktop on this Home");
}

async fn agent_attach_client_token(
    client: &reqwest::Client,
    api_url: &str,
    attach_secret: &str,
) -> anyhow::Result<String> {
    gateway_attach_runtime_token(client, api_url, attach_secret, "client").await
}

async fn list_desktop_names_for_agent(
    state: &GatewayState,
    principal_id: &str,
    resource: &str,
) -> anyhow::Result<String> {
    let Some(registry) = state.provider_registry.as_ref() else {
        anyhow::bail!("object provider registry unavailable");
    };
    let request = json!({
        "op": "list",
        "principal_id": principal_id,
        "uri": resource,
    });
    let response = registry
        .send_raw("object", &request)
        .await
        .map_err(|err| anyhow::anyhow!("object list failed: {err}"))?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("object list failed");
        anyhow::bail!("{message}");
    }
    let objects = response
        .get("data")
        .and_then(|data| data.get("objects"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = Vec::new();
    lines.push(format!("Library.read · {resource}"));
    lines.push(format!("{} object(s)", objects.len()));
    for object in objects.iter().take(MAX_LIBRARY_LIST_NAMES) {
        let name = object
            .get("name")
            .or_else(|| object.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)");
        let kind = object
            .get("kind")
            .or_else(|| object.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("object");
        let object_uri = object
            .get("uri")
            .or_else(|| object.get("target_uri"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if object_uri.is_empty() {
            lines.push(format!("- {name} · {kind}"));
        } else {
            lines.push(format!("- {name} · {kind} · {object_uri}"));
        }
    }
    if objects.len() > MAX_LIBRARY_LIST_NAMES {
        lines.push(format!(
            "… {} more not shown",
            objects.len() - MAX_LIBRARY_LIST_NAMES
        ));
    }
    let mut text = lines.join("\n");
    if text.len() > MAX_LIBRARY_RESULT_CHARS {
        text.truncate(MAX_LIBRARY_RESULT_CHARS);
        text.push('…');
    }
    Ok(text)
}

/// After Inbox grants a home-agent library.read request — one list, then done.
pub(super) async fn fulfill_agent_library_read_after_grant(
    state: &GatewayState,
    request_id: &str,
) -> anyhow::Result<()> {
    let mut store = load_agent_library_read_store(&state.data_dir);
    let Some(job) = store
        .jobs
        .iter_mut()
        .find(|job| job.request_id == request_id)
    else {
        return Ok(());
    };
    if job.status == "ready" || job.status == "denied" {
        return Ok(());
    }
    let principal_id = job.principal_id.clone();
    let resource = job.resource.clone();
    match list_desktop_names_for_agent(state, &principal_id, &resource).await {
        Ok(result) => {
            job.status = "ready".to_string();
            job.result = Some(result);
            job.error = None;
        }
        Err(err) => {
            job.status = "error".to_string();
            job.error = Some(err.to_string());
            job.result = None;
        }
    }
    store_agent_library_read_store(&state.data_dir, &store)
}

pub(super) fn mark_agent_library_read_denied(data_dir: &Path, request_id: &str) {
    let mut store = load_agent_library_read_store(data_dir);
    let Some(job) = store
        .jobs
        .iter_mut()
        .find(|job| job.request_id == request_id)
    else {
        return;
    };
    job.status = "denied".to_string();
    job.result = None;
    job.error = Some("Denied in Inbox".to_string());
    let _ = store_agent_library_read_store(data_dir, &store);
}

/// POST /api/apps/home/agent/tools/library.read
pub(super) async fn home_agent_library_read_request(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<HomeAgentLibraryReadBody>,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_GUI_SHELL_ID],
    ) {
        Ok(context) => context,
        Err(err) => {
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
    };

    let resource = match agent_desktop_resource(&context, body.uri.as_deref()) {
        Ok(resource) => resource,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_resource",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let Some(coords) = load_live_runtime_coords(&state.data_dir).await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "error",
                "code": "runtime_unavailable",
                "message": "local runtime is not running — Inbox cannot mint library.read",
            })),
        )
            .into_response();
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
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

    let client_token = match agent_attach_client_token(
        &client,
        &coords.api_url,
        &coords.attach_secret,
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "error",
                    "code": "attach_failed",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let response = match client
        .post(format!("{}/api/capability/request", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {client_token}"))
        .json(&json!({
            "resource": resource,
            "action": "read",
            "reason": AGENT_LIBRARY_READ_REASON,
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "error",
                    "code": "capability_request_failed",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let status = response.status();
    let body: Value = match response.json().await {
        Ok(body) => body,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "error",
                    "code": "capability_request_decode",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    if !status.is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "error",
                "code": "capability_request_http",
                "message": body.get("message").and_then(Value::as_str).unwrap_or("capability request failed"),
            })),
        )
            .into_response();
    }

    let request_status = body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("error");
    if request_status == "denied" || request_status == "auto_denied" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "denied",
                "code": "capability_denied",
                "message": body.get("reason").and_then(Value::as_str).unwrap_or("capability denied"),
            })),
        )
            .into_response();
    }

    let Some(request_id) = body.get("request_id").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "error",
                "code": "missing_request_id",
                "message": "capability request returned no request_id",
            })),
        )
            .into_response();
    };

    let job = AgentLibraryReadJob {
        request_id: request_id.to_string(),
        principal_id: context.principal_id.clone(),
        resource: resource.clone(),
        created_at: crate::auth::now_ts(),
        status: "pending".to_string(),
        result: None,
        error: None,
    };
    if let Err(err) = upsert_agent_library_read_job(&state.data_dir, job) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "code": "job_store",
                "message": err.to_string(),
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "pending",
            "request_id": request_id,
            "resource": resource,
            "tool": "library.read",
            "label": "Library · Read",
            "summary": "Allow once in Inbox — one Desktop list for Agent on this Home.",
            "scope": resource,
            "inbox": true,
        })),
    )
        .into_response()
}

/// GET /api/apps/home/agent/tools/library.read/:request_id
pub(super) async fn home_agent_library_read_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_GUI_SHELL_ID],
    ) {
        Ok(context) => context,
        Err(err) => {
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
    };

    let store = load_agent_library_read_store(&state.data_dir);
    let Some(job) = store.jobs.iter().find(|job| job.request_id == request_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "error",
                "code": "unknown_request",
                "message": "no library.read job for that request_id",
            })),
        )
            .into_response();
    };
    if job.principal_id != context.principal_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "principal_mismatch",
                "message": "library.read job belongs to another Home principal",
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": job.status,
            "request_id": job.request_id,
            "resource": job.resource,
            "result": job.result,
            "error": job.error,
            "tool": "library.read",
        })),
    )
        .into_response()
}

/// POST /api/apps/home/agent/tools/library.read/:request_id/cancel
pub(super) async fn home_agent_library_read_cancel(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_GUI_SHELL_ID],
    ) {
        Ok(context) => context,
        Err(err) => {
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
    };

    let store = load_agent_library_read_store(&state.data_dir);
    let Some(job) = store.jobs.iter().find(|job| job.request_id == request_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "error",
                "code": "unknown_request",
                "message": "no library.read job for that request_id",
            })),
        )
            .into_response();
    };
    if job.principal_id != context.principal_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "principal_mismatch",
                "message": "library.read job belongs to another Home principal",
            })),
        )
            .into_response();
    }

    if let Some(coords) = load_live_runtime_coords(&state.data_dir).await {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            if let Ok(shell_token) =
                home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await
            {
                let _ = client
                    .post(format!("{}/api/capability/deny", coords.api_url))
                    .header(AUTHORIZATION, format!("Bearer {shell_token}"))
                    .json(&json!({
                        "request_id": request_id,
                        "reason": "Denied from Agent",
                    }))
                    .send()
                    .await;
            }
        }
    }
    mark_agent_library_read_denied(&state.data_dir, &request_id);

    (
        StatusCode::OK,
        Json(json!({
            "status": "denied",
            "request_id": request_id,
            "tool": "library.read",
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct HomeAgentLibraryExtractBody {
    uri: String,
}

async fn read_desktop_text_for_agent(
    state: &GatewayState,
    principal_id: &str,
    uri: &str,
) -> anyhow::Result<(String, String)> {
    let Some(registry) = state.provider_registry.as_ref() else {
        anyhow::bail!("object provider registry unavailable");
    };
    let request = json!({
        "op": "read",
        "principal_id": principal_id,
        "uri": uri,
    });
    let response = registry
        .send_raw("object", &request)
        .await
        .map_err(|err| anyhow::anyhow!("object read failed: {err}"))?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("object read failed");
        anyhow::bail!("{message}");
    }
    let data = response
        .get("data")
        .cloned()
        .unwrap_or(Value::Null);
    let encoding = data
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("base64");
    if encoding != "base64" {
        anyhow::bail!("unsupported object encoding: {encoding}");
    }
    let b64 = data
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("object read missing data"))?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|err| anyhow::anyhow!("object read base64 decode failed: {err}"))?;
    if bytes.len() > MAX_LIBRARY_EXTRACT_BYTES {
        anyhow::bail!(
            "object larger than {} bytes — not extracted",
            MAX_LIBRARY_EXTRACT_BYTES
        );
    }
    if bytes
        .iter()
        .take(512)
        .any(|&b| b == 0)
    {
        anyhow::bail!("binary object — text extract not supported");
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_LIBRARY_EXTRACT_CHARS {
        text.truncate(MAX_LIBRARY_EXTRACT_CHARS);
        text.push('…');
    }
    let name = data
        .get("object")
        .and_then(|object| object.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            uri.rsplit('/').next().unwrap_or("object")
        })
        .to_string();
    Ok((name, text))
}

/// POST /api/apps/home/agent/tools/web.search
/// Wave 6.02 — fail-closed until Exit/net capability exists (no browser scrape).
pub(super) async fn home_agent_web_search(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(_body): Json<Value>,
) -> Response {
    if let Err(err) = require_home_gui_launch(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "status": "unavailable",
            "tool": "web.search",
            "label": "Web · Search",
            "summary": "Web search needs an Exit/net grant on this Home. Agent does not scrape the open web from the browser (UI ≠ authority).",
            "scope": "exit/net (not granted)",
            "citations": [],
            "result": null,
            "fail_closed": true,
        })),
    )
        .into_response()
}

/// POST /api/apps/home/agent/tools/library.read/:request_id/extract
/// Wave 6.01 — after Inbox grant (job ready), extract one Desktop text object.
pub(super) async fn home_agent_library_read_extract(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
    Json(body): Json<HomeAgentLibraryExtractBody>,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_GUI_SHELL_ID],
    ) {
        Ok(context) => context,
        Err(err) => {
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
    };

    let store = load_agent_library_read_store(&state.data_dir);
    let Some(job) = store.jobs.iter().find(|job| job.request_id == request_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "error",
                "code": "unknown_request",
                "message": "no library.read job for that request_id",
            })),
        )
            .into_response();
    };
    if job.principal_id != context.principal_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "principal_mismatch",
                "message": "library.read job belongs to another Home principal",
            })),
        )
            .into_response();
    }
    if job.status != "ready" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "grant_not_ready",
                "message": format!(
                    "library.read job is {} — Inbox Allow once required before extract",
                    job.status
                ),
            })),
        )
            .into_response();
    }

    let uri = match agent_desktop_resource(&context, Some(body.uri.as_str())) {
        Ok(uri) => uri,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_resource",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };
    if uri == job.resource {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "code": "not_a_file",
                "message": "extract requires a Desktop file uri, not the Desktop root",
            })),
        )
            .into_response();
    }

    match read_desktop_text_for_agent(&state, &context.principal_id, &uri).await {
        Ok((name, text)) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "request_id": request_id,
                "uri": uri,
                "name": name,
                "text": text,
                "tool": "library.read",
                "cited": true,
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "code": "extract_failed",
                "message": err.to_string(),
            })),
        )
            .into_response(),
    }
}
