//! Common types used across ElastOS

use serde::{Deserialize, Serialize};

/// Unique identifier for a capsule instance
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapsuleId(pub String);

impl CapsuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CapsuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a capsule
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapsuleStatus {
    Loading,
    Running,
    Stopped,
    Failed,
}

impl std::fmt::Display for CapsuleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapsuleStatus::Loading => write!(f, "loading"),
            CapsuleStatus::Running => write!(f, "running"),
            CapsuleStatus::Stopped => write!(f, "stopped"),
            CapsuleStatus::Failed => write!(f, "failed"),
        }
    }
}
