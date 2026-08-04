//! Home Agent chat streaming (dogfood).
//!
//! OpenAI-compat SSE proxy for the opaque home-gui shell. Authority stays on
//! the gateway (home-gui launch token). Upstream URLs come from operator env /
//! allowlisted Sparks pairs — never from a free-form client URL (SSRF-closed).

use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_lite::stream;
use serde::Deserialize;
use serde_json::{json, Value};

use super::*;

const AGENT_STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const AGENT_STREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_AGENT_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_PAIR_A: &str = "http://192.168.1.147:8888/v1/chat/completions";
const DEFAULT_PAIR_B: &str = "http://192.168.1.145:8888/v1/chat/completions";
const DEFAULT_MAX_TOKENS: u64 = 2048;
const MAX_MESSAGES: usize = 48;
const MAX_MESSAGE_CHARS: usize = 16_000;

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

fn pair_upstream(pair: Option<&str>) -> anyhow::Result<(String, String)> {
    let model = std::env::var("OLLAMA_MODEL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_MODEL.to_string());
    let key = pair.unwrap_or("a").trim().to_ascii_lowercase();
    let url = match key.as_str() {
        "b" | "pair-b" | "pair_b" => std::env::var("OLLAMA_URL_B")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_PAIR_B.to_string()),
        _ => std::env::var("OLLAMA_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_PAIR_A.to_string()),
    };
    validate_openai_chat_url(&url)?;
    Ok((url, model))
}

fn validate_openai_chat_url(url: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid upstream URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => anyhow::bail!("upstream URL must be http(s)"),
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("upstream URL missing host");
    }
    let path = parsed.path().trim_end_matches('/');
    if !path.ends_with("/v1/chat/completions") && !path.ends_with("/chat/completions") {
        anyhow::bail!("upstream URL must be an OpenAI-compat chat completions endpoint");
    }
    Ok(())
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

    let (api_url, default_model) = match pair_upstream(body.pair.as_deref()) {
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

    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(default_model.as_str())
        .to_string();
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
