//! Run lifecycle: state machine, event log, registry.
//!
//! A run is one invocation of an offer operation. Events are an append-only
//! log; callers read with a cursor (`runs.events`). Cancellation is
//! cooperative: the worker checks the flag between stream chunks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Preparing,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, RunState::Succeeded | RunState::Failed | RunState::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    State { state: RunState },
    Text { delta: String },
    Thinking { delta: String },
    Progress { completed: u64, total: u64, phase: String },
    Result { objects: Vec<ObjectDescriptor> },
    Error { code: String, message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectDescriptor {
    pub id: String,
    pub media_type: String,
    pub sha256: String,
    pub size: u64,
}

pub struct Run {
    pub run_id: String,
    pub offer_id: String,
    pub operation: String,
    pub state: RunState,
    pub events: Vec<RunEvent>,
    pub cancel: Arc<AtomicBool>,
    pub created_at: SystemTime,
}

impl Run {
    pub fn push(&mut self, event: RunEvent) {
        if let RunEvent::State { state } = event {
            self.state = state;
        }
        self.events.push(event);
    }
}

#[derive(Default)]
pub struct Registry {
    runs: HashMap<String, Arc<Mutex<Run>>>,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn new_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("run:{nanos:x}-{seq}")
}

impl Registry {
    pub fn active_runs_for(&self, offer_id: &str) -> usize {
        self.runs
            .values()
            .filter(|run| {
                let run = run.lock().unwrap();
                run.offer_id == offer_id && !run.state.is_terminal()
            })
            .count()
    }

    pub fn create(&mut self, offer_id: &str, operation: &str) -> Arc<Mutex<Run>> {
        let run = Arc::new(Mutex::new(Run {
            run_id: new_run_id(),
            offer_id: offer_id.to_string(),
            operation: operation.to_string(),
            state: RunState::Queued,
            events: vec![RunEvent::State { state: RunState::Queued }],
            cancel: Arc::new(AtomicBool::new(false)),
            created_at: SystemTime::now(),
        }));
        self.runs.insert(run.lock().unwrap().run_id.clone(), Arc::clone(&run));
        run
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<Mutex<Run>>> {
        self.runs.get(run_id).cloned()
    }
}

/// Validate chat `generate` inputs against the offer's parameters_schema
/// (additionalProperties: false is honored — unknown params are rejected).
pub fn validate_chat_inputs(inputs: &serde_json::Value) -> Result<ChatParams, Response2> {
    let messages = inputs
        .get("messages")
        .and_then(|value| value.as_array())
        .ok_or_else(|| ("invalid_inputs", "inputs.messages must be an array"))?;
    for message in messages {
        let role_ok = message.get("role").and_then(|v| v.as_str()).is_some();
        let content_ok = message.get("content").and_then(|v| v.as_str()).is_some();
        if !role_ok || !content_ok {
            return Err(("invalid_inputs", "each message needs string role + content"));
        }
    }
    let max_tokens = match inputs.get("max_tokens") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let n = value
                .as_u64()
                .ok_or_else(|| ("invalid_inputs", "max_tokens must be an integer"))?;
            if n == 0 {
                return Err(("invalid_inputs", "max_tokens must be at least 1"));
            }
            // No upper cap: the caller owns the compute and may request any budget.
            Some(n)
        }
    };
    let temperature = match inputs.get("temperature") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let t = value
                .as_f64()
                .ok_or_else(|| ("invalid_inputs", "temperature must be a number"))?;
            if !(0.0..=2.0).contains(&t) {
                return Err(("invalid_inputs", "temperature out of range 0..=2"));
            }
            Some(t)
        }
    };
    let known = ["messages", "max_tokens", "temperature"];
    if let Some(object) = inputs.as_object() {
        for key in object.keys() {
            if !known.contains(&key.as_str()) {
                return Err(("invalid_inputs", "unknown parameter (additionalProperties false)"));
            }
        }
    }
    Ok(ChatParams {
        messages: messages.clone(),
        max_tokens,
        temperature,
    })
}

pub struct ChatParams {
    pub messages: Vec<serde_json::Value>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// Validate video `generate` inputs against the offer's parameters_schema.
///
/// Honesty shim: the offer advertises `resolution`/`aspect_ratio`/`scale` as
/// the target contract, but the current H3 backend renders one profile
/// (768×448 @ 24fps, scale 2). Non-default values fail closed with a typed
/// error instead of being silently ignored — SP-EXPLOIT: no silent no-ops.
pub fn validate_video_inputs(inputs: &serde_json::Value) -> Result<VideoParams, Response2> {
    let prompt = inputs
        .get("prompt")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ("invalid_inputs", "inputs.prompt must be a non-empty string"))?;
    if prompt.len() > 8192 {
        return Err(("invalid_inputs", "prompt too long (8192 chars max)"));
    }
    let duration = match inputs.get("duration_seconds") {
        None | Some(serde_json::Value::Null) => 5,
        Some(value) => {
            let d = value
                .as_u64()
                .ok_or_else(|| ("invalid_inputs", "duration_seconds must be an integer"))?;
            if d == 0 {
                return Err(("invalid_inputs", "duration_seconds must be at least 1"));
            }
            // No upper cap: the caller owns the compute and may request any duration.
            d
        }
    };
    for (key, default) in [("resolution", "720p"), ("aspect_ratio", "16:9")] {
        if let Some(value) = inputs.get(key).and_then(|value| value.as_str()) {
            if value != default {
                return Err((
                    "unsupported_parameter",
                    "current backend renders 720p 16:9 only; non-default rejected, not ignored",
                ));
            }
        }
    }
    if let Some(value) = inputs.get("scale").and_then(|value| value.as_u64()) {
        if value != 2 {
            return Err((
                "unsupported_parameter",
                "current backend renders scale 2 only; non-default rejected, not ignored",
            ));
        }
    }
    let known = ["prompt", "duration_seconds", "resolution", "aspect_ratio", "scale", "reference"];
    if let Some(object) = inputs.as_object() {
        for key in object.keys() {
            if !known.contains(&key.as_str()) {
                return Err(("invalid_inputs", "unknown parameter (additionalProperties false)"));
            }
        }
    }
    Ok(VideoParams {
        prompt: prompt.to_string(),
        duration_seconds: duration,
    })
}

pub struct VideoParams {
    pub prompt: String,
    pub duration_seconds: u64,
}

/// (code, message) — converted to a Response at the call site.
pub type Response2 = (&'static str, &'static str);
