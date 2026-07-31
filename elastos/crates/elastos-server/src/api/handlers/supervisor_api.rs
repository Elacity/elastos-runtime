//! Supervisor HTTP endpoints — shell-only operations for capsule VM lifecycle.
//!
//! These endpoints expose the supervisor's control plane over HTTP so the shell
//! capsule (running inside a VM) can orchestrate capsule lifecycle:
//! ensure externals, ensure capsules, launch, stop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, http::HeaderValue, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::setup::detect_platform;
use crate::supervisor::{Supervisor, SupervisorRequest, SupervisorResponse};

#[derive(Clone)]
pub struct SupervisorState {
    pub supervisor: Arc<Supervisor>,
    pub data_dir: Option<PathBuf>,
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
#[serde(deny_unknown_fields)]
pub struct LaunchCapsuleInput {
    pub name: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub principal_id: Option<serde_json::Value>,
    #[serde(default)]
    pub launch_grant: Option<String>,
    #[serde(default)]
    pub home_token: Option<serde_json::Value>,
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
    let principal_id = supervisor_launch_principal_from_input(state.data_dir.as_deref(), &input)?;
    let req = SupervisorRequest::LaunchCapsule {
        name: input.name,
        config: input.config,
        principal_id,
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

const SUPERVISOR_PRINCIPAL_AUTHORITY_KEYS: &[&str] =
    &["principal_id", "launch_grant", "home_token"];

fn supervisor_launch_principal_from_input(
    data_dir: Option<&Path>,
    input: &LaunchCapsuleInput,
) -> Result<Option<String>, (StatusCode, String)> {
    let has_top_level_authority = [
        input.principal_id.as_ref().map(|_| "principal_id"),
        input.home_token.as_ref().map(|_| "home_token"),
    ]
    .into_iter()
    .flatten()
    .next();
    if let Some(key) = has_top_level_authority {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "supervisor launches do not accept raw principal authority ({key}); use signed launch_grant"
            ),
        ));
    }

    if let Some(config) = input.config.as_object() {
        for key in SUPERVISOR_PRINCIPAL_AUTHORITY_KEYS {
            if config.contains_key(*key) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "supervisor launch config must not carry {key}; use the signed Home launch-grant path"
                    ),
                ));
            }
        }
    }

    let Some(grant) = input
        .launch_grant
        .as_deref()
        .map(str::trim)
        .filter(|grant| !grant.is_empty())
    else {
        return Ok(None);
    };

    let data_dir = data_dir.ok_or((
        StatusCode::BAD_REQUEST,
        "principal launch grant unavailable".to_string(),
    ))?;
    let mut headers = HeaderMap::new();
    let header_value = HeaderValue::from_str(grant).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "invalid launch_grant header value".to_string(),
        )
    })?;
    headers.insert("x-elastos-home-token", header_value);
    let context = crate::api::gateway::require_internal_shell_launch_grant_for_any_context(
        data_dir,
        &headers,
        &[input.name.as_str()],
    )
    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    Ok(Some(context.principal_id))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn launch_input(value: serde_json::Value) -> LaunchCapsuleInput {
        serde_json::from_value(value).expect("launch input should parse")
    }

    #[test]
    fn supervisor_launch_rejects_top_level_principal_authority() {
        let input = launch_input(serde_json::json!({
            "name": "chat",
            "principal_id": "person:local:admin"
        }));

        let err = supervisor_launch_principal_from_input(None, &input)
            .expect_err("principal_id must fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err
            .1
            .contains("supervisor launches do not accept raw principal authority"));
    }

    #[test]
    fn supervisor_launch_rejects_config_principal_authority() {
        let input = launch_input(serde_json::json!({
            "name": "chat",
            "config": {
                "launch_grant": "signed-home-grant"
            }
        }));

        let err = supervisor_launch_principal_from_input(None, &input)
            .expect_err("launch_grant must fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err
            .1
            .contains("supervisor launch config must not carry launch_grant"));
    }

    #[test]
    fn supervisor_launch_allows_runtime_reserved_config() {
        let input = launch_input(serde_json::json!({
            "name": "chat",
            "config": {
                "_elastos_interactive": true,
                "_elastos_capsule_args": ["--demo"]
            }
        }));

        supervisor_launch_principal_from_input(None, &input)
            .expect("runtime reserved config should pass");
    }

    #[test]
    fn supervisor_launch_accepts_signed_launch_grant() {
        let dir = tempfile::tempdir().unwrap();
        let context = crate::api::gateway::local_home_launch_token_context(dir.path()).unwrap();
        let grant =
            crate::api::gateway::issue_home_launch_token_with_context(dir.path(), "chat", &context)
                .unwrap();
        let input = launch_input(serde_json::json!({
            "name": "chat",
            "launch_grant": grant
        }));

        let principal = supervisor_launch_principal_from_input(Some(dir.path()), &input).unwrap();

        assert_eq!(principal.as_deref(), Some(context.principal_id.as_str()));
    }

    #[test]
    fn supervisor_launch_rejects_wrong_app_grant() {
        let dir = tempfile::tempdir().unwrap();
        let context = crate::api::gateway::local_home_launch_token_context(dir.path()).unwrap();
        let grant = crate::api::gateway::issue_home_launch_token_with_context(
            dir.path(),
            "documents",
            &context,
        )
        .unwrap();
        let input = launch_input(serde_json::json!({
            "name": "chat",
            "launch_grant": grant
        }));

        let err = supervisor_launch_principal_from_input(Some(dir.path()), &input).unwrap_err();

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("not authorized"));
    }
}
