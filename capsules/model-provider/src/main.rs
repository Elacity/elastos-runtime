//! ElastOS Model Provider Capsule
//!
//! One offers/runs contract for model inference from configured backends:
//! local (llama/ollama), Spark/Jetson on LAN, Spark via Carrier, or hosted
//! OpenAI-compatible APIs. Upstreams come from operator Init config only —
//! never from callers (SSRF-closed).
//! Wire protocol: line-delimited JSON over stdin/stdout.

mod adapter {
    pub mod h3_video;
    pub mod openai_compat;
}
mod offer;
mod run;

use offer::ProviderConfig;
use run::{Registry, RunState};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    OffersList,
    RunsCreate {
        offer_id: String,
        operation: String,
        inputs: serde_json::Value,
    },
    RunsGet {
        run_id: String,
    },
    RunsEvents {
        run_id: String,
        #[serde(default)]
        cursor: u64,
    },
    RunsCancel {
        run_id: String,
    },
    Ping,
    Shutdown,
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

struct ModelProvider {
    config: ProviderConfig,
    registry: Registry,
}

impl ModelProvider {
    fn new() -> Self {
        ModelProvider {
            config: ProviderConfig::default(),
            registry: Registry::default(),
        }
    }

    fn runs_create(
        &mut self,
        offer_id: &str,
        operation: &str,
        inputs: serde_json::Value,
    ) -> Response {
        if operation != "generate" {
            return Response::error("invalid_operation", "only the generate operation exists");
        }
        let configured = offer::configured_offers(&self.config);
        let Some(offer) = configured
            .iter()
            .map(|service| &service.descriptor)
            .find(|descriptor| descriptor.offer_id == offer_id)
        else {
            return Response::error("offer_not_found", "offer unknown or backend not configured");
        };

        let active = self.registry.active_runs_for(offer_id);
        if active >= offer.policy.maximum_concurrent_runs as usize {
            return Response::error(
                "policy_violation",
                "offer is at its concurrent run limit; retry later",
            );
        }

        match offer_id {
            "offer:flash-chat:pair-a" => {
                let Some(upstream) = self.config.flash_url.clone().filter(|u| !u.is_empty()) else {
                    return Response::error("offer_not_found", "chat backend is not configured");
                };
                let params = match run::validate_chat_inputs(&inputs) {
                    Ok(params) => params,
                    Err((code, message)) => return Response::error(code, message),
                };
                let run = self.registry.create(offer_id, operation);
                let run_id = run.lock().unwrap().run_id.clone();
                let model = offer.model.id.clone();
                let worker = std::sync::Arc::clone(&run);
                std::thread::spawn(move || {
                    adapter::openai_compat::run_chat(worker, upstream, model, params);
                });
                Response::ok(serde_json::json!({
                    "run_id": run_id,
                    "offer_id": offer_id,
                    "state": RunState::Queued,
                }))
            }
            "offer:h3-video:2x" => {
                let params = match run::validate_video_inputs(&inputs) {
                    Ok(params) => params,
                    Err((code, message)) => return Response::error(code, message),
                };
                let run = self.registry.create(offer_id, operation);
                let run_id = run.lock().unwrap().run_id.clone();
                let upstream = format!(
                    "{}/v1/videos/sync",
                    self.config.h3_url.clone().unwrap_or_default()
                );
                let output_dir = self.config.output_dir();
                let worker = std::sync::Arc::clone(&run);
                std::thread::spawn(move || {
                    adapter::h3_video::run_video(worker, upstream, output_dir, params);
                });
                Response::ok(serde_json::json!({
                    "run_id": run_id,
                    "offer_id": offer_id,
                    "state": RunState::Queued,
                }))
            }
            _ => Response::error("offer_not_found", "offer has no adapter on this provider"),
        }
    }

    fn runs_get(&self, run_id: &str) -> Response {
        let Some(run) = self.registry.get(run_id) else {
            return Response::error("run_not_found", "unknown run_id");
        };
        let run = run.lock().unwrap();
        let age_ms = run
            .created_at
            .elapsed()
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Response::ok(serde_json::json!({
            "run_id": run.run_id,
            "offer_id": run.offer_id,
            "operation": run.operation,
            "state": run.state,
            "events": run.events,
            "event_count": run.events.len(),
            "age_ms": age_ms,
        }))
    }

    fn runs_events(&self, run_id: &str, cursor: u64) -> Response {
        let Some(run) = self.registry.get(run_id) else {
            return Response::error("run_not_found", "unknown run_id");
        };
        let run = run.lock().unwrap();
        let from = (cursor as usize).min(run.events.len());
        let events: Vec<_> = run.events[from..].to_vec();
        Response::ok(serde_json::json!({
            "run_id": run.run_id,
            "state": run.state,
            "events": events,
            "cursor": run.events.len() as u64,
        }))
    }

    fn runs_cancel(&self, run_id: &str) -> Response {
        let Some(run) = self.registry.get(run_id) else {
            return Response::error("run_not_found", "unknown run_id");
        };
        let run = run.lock().unwrap();
        if run.state.is_terminal() {
            return Response::error("not_cancellable", "run is already in a terminal state");
        }
        run.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        Response::ok(serde_json::json!({
            "run_id": run.run_id,
            "state": "cancelling",
        }))
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => {
                self.config = ProviderConfig::from_init(&config);
                Response::ok(serde_json::json!({
                    "provider": "model-provider",
                    "version": PROVIDER_VERSION,
                }))
            }
            Request::OffersList => {
                let offers = offer::configured_offers(&self.config);
                Response::ok(serde_json::json!({ "offers": offers }))
            }
            Request::RunsCreate {
                offer_id,
                operation,
                inputs,
            } => self.runs_create(&offer_id, &operation, inputs),
            Request::RunsGet { run_id } => self.runs_get(&run_id),
            Request::RunsEvents { run_id, cursor } => self.runs_events(&run_id, cursor),
            Request::RunsCancel { run_id } => self.runs_cancel(&run_id),
            Request::Ping => Response::ok(serde_json::json!({
                "provider": "model-provider",
                "version": PROVIDER_VERSION,
            })),
            Request::Shutdown => Response::ok(serde_json::json!({ "bye": true })),
        }
    }
}

fn main() {
    eprintln!("model-provider: starting v{}", PROVIDER_VERSION);
    let mut provider = ModelProvider::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
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
}
