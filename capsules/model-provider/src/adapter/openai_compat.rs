//! openai_compat adapter: chat runs against an OpenAI-compatible upstream
//! (Flash pair A on the Sparks LAN). The upstream URL comes from operator
//! Init config — never from the caller.
//!
//! Flash/vLLM quirk (ported from the Home gateway + agent-live.js): the model
//! may emit `content: null` with the real text in `reasoning` /
//! `reasoning_content` / `thinking`. Content deltas become `Text` events,
//! reasoning deltas become `Thinking` events, and a final `message` payload
//! is used as a fallback when deltas carried nothing.

use crate::run::{ChatParams, Run, RunEvent, RunState};
use std::io::{BufRead, BufReader};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/* Sovereign runtime: no policy ceiling on a chat run. The run ends when the model
finishes, the caller cancels (Stop), or the upstream connection genuinely fails —
never on a wall-clock timeout. The caller owns the compute. */

fn extract_delta(chunk: &serde_json::Value) -> (String, String) {
    let delta = &chunk["choices"][0]["delta"];
    let content = delta
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let mut reasoning = String::new();
    for field in ["reasoning", "reasoning_content", "thinking"] {
        if let Some(value) = delta.get(field).and_then(|value| value.as_str()) {
            if !value.is_empty() {
                reasoning = value.to_string();
                break;
            }
        }
    }
    (content, reasoning)
}

/// Fallback: some servers put final text on `message` instead of `delta`.
fn message_text(chunk: &serde_json::Value) -> String {
    let message = &chunk["choices"][0]["message"];
    let content = message
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if !content.is_empty() {
        return content.to_string();
    }
    for field in ["reasoning_content", "reasoning", "reasoning_text"] {
        if let Some(value) = message.get(field).and_then(|value| value.as_str()) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

pub fn run_chat(run: Arc<Mutex<Run>>, upstream_url: String, model: String, params: ChatParams) {
    let cancel = Arc::clone(&run.lock().unwrap().cancel);
    let push = |event: RunEvent| run.lock().unwrap().push(event);
    let fail = |code: &str, message: &str| {
        push(RunEvent::Error {
            code: code.to_string(),
            message: message.to_string(),
        });
        push(RunEvent::State {
            state: RunState::Failed,
        });
    };

    push(RunState::Running.into());

    let mut payload = serde_json::json!({
        "model": model,
        "messages": params.messages,
        "stream": true,
    });
    if let Some(max_tokens) = params.max_tokens {
        payload["max_tokens"] = max_tokens.into();
    }
    if let Some(temperature) = params.temperature {
        payload["temperature"] = temperature.into();
    }

    let response =
        match ureq::post(&format!("{}/chat/completions", upstream_url)).send_json(payload) {
            Ok(response) => response,
            Err(err) => {
                // ureq errors on non-2xx too: the upstream may be reachable and
                // rejecting the request — say "error", not "unreachable".
                fail("upstream_error", &err.to_string());
                return;
            }
        };

    let mut saw_text = false;
    let mut lines = BufReader::new(response.into_reader()).lines();
    loop {
        if cancel.load(Ordering::Relaxed) {
            push(RunState::Cancelled.into());
            return;
        }
        let line = match lines.next() {
            Some(Ok(line)) => line,
            Some(Err(err)) => {
                fail("stream_error", &err.to_string());
                return;
            }
            None => break,
        };
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            break;
        }
        let chunk: serde_json::Value = match serde_json::from_str(data) {
            Ok(chunk) => chunk,
            Err(_) => continue,
        };
        let (content, reasoning) = extract_delta(&chunk);
        if !reasoning.is_empty() {
            push(RunEvent::Thinking { delta: reasoning });
        }
        if !content.is_empty() {
            saw_text = true;
            push(RunEvent::Text { delta: content });
        }
        if !saw_text {
            let fallback = message_text(&chunk);
            if !fallback.is_empty() {
                saw_text = true;
                push(RunEvent::Text { delta: fallback });
            }
        }
    }

    push(RunState::Succeeded.into());
}

impl From<RunState> for RunEvent {
    fn from(state: RunState) -> Self {
        RunEvent::State { state }
    }
}
