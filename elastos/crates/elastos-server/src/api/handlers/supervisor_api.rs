//! Supervisor HTTP endpoints — shell-only operations for capsule VM lifecycle.
//!
//! These endpoints expose the supervisor's control plane over HTTP so the shell
//! capsule (running inside a VM) can orchestrate capsule lifecycle:
//! ensure externals, ensure capsules, launch, stop.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::setup::detect_platform;
use crate::supervisor::{Supervisor, SupervisorRequest, SupervisorResponse};

#[derive(Clone)]
pub struct SupervisorState {
    pub supervisor: Arc<Supervisor>,
}

// ── Request/Response types ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EnsureExternalInput {
    pub name: String,
    /// Platform key matching components.json (e.g. "linux-amd64", "linux-arm64").
    /// Defaults to the host platform via setup::detect_platform().
    #[serde(default = "default_platform")]
    pub platform: String,
}

fn default_platform() -> String {
    detect_platform()
}

#[derive(Debug, Deserialize)]
pub struct EnsureCapsuleInput {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct LaunchCapsuleInput {
    pub name: String,
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct StopCapsuleInput {
    pub handle: String,
}

#[derive(Debug, Deserialize)]
pub struct WaitCapsuleInput {
    pub handle: String,
}

#[derive(Debug, Deserialize)]
pub struct ResolvePlanInput {
    pub target: String,
}

#[derive(Debug, Deserialize)]
pub struct StartGatewayInput {
    pub addr: String,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SupervisorOutput {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vsock_cid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolvePlanOutput {
    pub status: String,
    pub capsules: Vec<String>,
    pub externals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<SupervisorResponse> for SupervisorOutput {
    fn from(r: SupervisorResponse) -> Self {
        Self {
            status: r.status,
            path: r.path,
            handle: r.handle,
            vsock_cid: r.vsock_cid,
            error: r.error,
        }
    }
}

/// POST /api/supervisor/resolve-plan — resolve transitive capsule/external dependencies.
pub async fn resolve_plan(
    State(state): State<SupervisorState>,
    Json(input): Json<ResolvePlanInput>,
) -> Result<Json<ResolvePlanOutput>, (StatusCode, String)> {
    match state.supervisor.resolve_launch_plan(&input.target).await {
        Ok((capsules, externals)) => Ok(Json(ResolvePlanOutput {
            status: "ok".into(),
            capsules,
            externals,
            error: None,
        })),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

// ── Handlers ────────────────────────────────────────────────────────

/// POST /api/supervisor/ensure-external — download/verify an external tool.
pub async fn ensure_external(
    State(state): State<SupervisorState>,
    Json(input): Json<EnsureExternalInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::DownloadExternal {
        name: input.name,
        platform: input.platform,
    };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}

/// POST /api/supervisor/ensure-capsule — download/verify a capsule artifact.
pub async fn ensure_capsule(
    State(state): State<SupervisorState>,
    Json(input): Json<EnsureCapsuleInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::EnsureCapsule { name: input.name };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}

/// POST /api/supervisor/launch-capsule — boot a capsule in a crosvm VM.
pub async fn launch_capsule(
    State(state): State<SupervisorState>,
    Json(input): Json<LaunchCapsuleInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::LaunchCapsule {
        name: input.name,
        config: input.config,
    };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}

/// POST /api/supervisor/stop-capsule — stop a running capsule VM.
pub async fn stop_capsule(
    State(state): State<SupervisorState>,
    Json(input): Json<StopCapsuleInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::StopCapsule {
        handle: input.handle,
    };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}

/// POST /api/supervisor/wait-capsule — wait for a running capsule VM to exit.
pub async fn wait_capsule(
    State(state): State<SupervisorState>,
    Json(input): Json<WaitCapsuleInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::WaitCapsule {
        handle: input.handle,
    };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}

/// POST /api/supervisor/start-gateway — start/reuse runtime gateway server.
pub async fn start_gateway(
    State(state): State<SupervisorState>,
    Json(input): Json<StartGatewayInput>,
) -> Result<Json<SupervisorOutput>, (StatusCode, String)> {
    let req = SupervisorRequest::StartGateway {
        addr: input.addr,
        cache_dir: input.cache_dir,
    };
    let resp = state.supervisor.handle_request(req).await;
    if resp.status != "ok" {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            resp.error.unwrap_or_else(|| "unknown error".into()),
        ));
    }
    Ok(Json(resp.into()))
}
