//! Orchestrator endpoints — shell-only operations for runtime coordination.
//!
//! Allows a second CLI process (e.g. `elastos agent`) to mint a fresh capsule
//! session on an already-running runtime (e.g. started by `elastos chat`).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use elastos_runtime::session::{SessionRegistry, SessionType};

#[derive(Clone)]
pub struct OrchestratorState {
    pub session_registry: Arc<SessionRegistry>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionInput {
    /// Hint for audit trail (e.g. "chat", "agent")
    #[serde(default)]
    pub owner_hint: String,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionOutput {
    pub token: String,
    pub session_id: String,
}

/// POST /api/orchestrator/session — mint a fresh capsule session.
/// Requires shell session auth (via shell_only_middleware).
pub async fn create_session(
    State(state): State<OrchestratorState>,
    Json(_input): Json<CreateSessionInput>,
) -> Result<Json<CreateSessionOutput>, (StatusCode, String)> {
    let session = state
        .session_registry
        .create_session(SessionType::Capsule, None)
        .await;
    Ok(Json(CreateSessionOutput {
        token: session.token.clone(),
        session_id: session.id.to_string(),
    }))
}
