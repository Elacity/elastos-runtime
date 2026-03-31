//! Test helper endpoints (debug builds only)
//!
//! These endpoints are only available in debug builds and are used for testing
//! the capability request flow without needing a real VM.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use elastos_runtime::session::{SessionRegistry, SessionType};

/// State for test helper endpoints
#[derive(Clone)]
pub struct TestState {
    pub session_registry: Arc<SessionRegistry>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTestSessionInput {
    /// Session type: "shell" or "capsule"
    #[serde(default = "default_session_type")]
    pub session_type: String,

    /// Optional VM ID to associate with the session
    pub vm_id: Option<String>,
}

fn default_session_type() -> String {
    "shell".to_string()
}

#[derive(Debug, Serialize)]
pub struct CreateTestSessionOutput {
    pub session_id: String,
    pub token: String,
    pub session_type: String,
}

/// POST /api/test/create-session
///
/// Create a test session for integration testing.
/// This endpoint is only available in debug builds.
pub async fn create_test_session(
    State(state): State<TestState>,
    Json(input): Json<CreateTestSessionInput>,
) -> Result<Json<CreateTestSessionOutput>, (StatusCode, String)> {
    let session_type = match input.session_type.to_lowercase().as_str() {
        "shell" => SessionType::Shell,
        "capsule" => SessionType::Capsule,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid session type: {}. Expected 'shell' or 'capsule'",
                    input.session_type
                ),
            ));
        }
    };

    let session = state
        .session_registry
        .create_session(session_type, input.vm_id)
        .await;

    Ok(Json(CreateTestSessionOutput {
        session_id: session.id.to_string(),
        token: session.token.clone(),
        session_type: session.session_type.to_string(),
    }))
}
