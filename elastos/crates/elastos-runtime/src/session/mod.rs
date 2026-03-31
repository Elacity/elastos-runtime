//! Session management for ElastOS
//!
//! Sessions represent authenticated connections to the runtime, typically from
//! VMs or external clients. Each session has a bearer token that must be
//! included in API requests.
//!
//! Session types:
//! - Shell: The primary UI capsule, can grant/deny capabilities
//! - Capsule: Other capsules that can only request capabilities
mod registry;

pub use registry::SessionRegistry;

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::primitives::time::SecureTimestamp;

/// Unique identifier for a session
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Create a new random session ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Create from a string
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Type of session - determines permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// Shell session - can grant/deny capabilities, view pending requests
    /// The desktop, CLI, or TUI shell
    Shell,

    /// Regular capsule session - can only request capabilities
    Capsule,
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionType::Shell => write!(f, "shell"),
            SessionType::Capsule => write!(f, "capsule"),
        }
    }
}

/// A session represents an authenticated connection to the runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session identifier
    pub id: SessionId,

    /// Bearer token for this session (UUID)
    /// This is what clients send in Authorization header
    pub token: String,

    /// Associated VM ID (if this session belongs to a VM)
    pub vm_id: Option<String>,

    /// Type of session (determines permissions)
    pub session_type: SessionType,

    /// Owner's public key (hex-encoded) - identifies the user's namespace
    /// This is set when the user authenticates with their key
    pub owner: Option<String>,

    /// When the session was created
    pub created_at: SecureTimestamp,

    /// Last activity timestamp (for cleanup)
    pub last_active: SecureTimestamp,
}

impl Session {
    /// Create a new session
    pub fn new(session_type: SessionType, vm_id: Option<String>) -> Self {
        let now = SecureTimestamp::now();
        Self {
            id: SessionId::new(),
            token: uuid::Uuid::new_v4().to_string(),
            vm_id,
            session_type,
            owner: None,
            created_at: now,
            last_active: now,
        }
    }

    /// Create a new session with an owner
    pub fn with_owner(session_type: SessionType, vm_id: Option<String>, owner: String) -> Self {
        let now = SecureTimestamp::now();
        Self {
            id: SessionId::new(),
            token: uuid::Uuid::new_v4().to_string(),
            vm_id,
            session_type,
            owner: Some(owner),
            created_at: now,
            last_active: now,
        }
    }

    /// Set the owner for this session
    pub fn set_owner(&mut self, owner: String) {
        self.owner = Some(owner);
    }

    /// Create a shell session for a VM
    pub fn new_shell(vm_id: String) -> Self {
        Self::new(SessionType::Shell, Some(vm_id))
    }

    /// Create a capsule session for a VM
    pub fn new_capsule(vm_id: String) -> Self {
        Self::new(SessionType::Capsule, Some(vm_id))
    }

    /// Check if this is a shell session
    pub fn is_shell(&self) -> bool {
        self.session_type == SessionType::Shell
    }

    /// Update last activity timestamp
    pub fn touch(&mut self) {
        self.last_active = SecureTimestamp::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
        assert!(!id1.as_str().is_empty());
    }

    #[test]
    fn test_session_creation() {
        let session = Session::new_shell("vm-123".to_string());
        assert!(session.is_shell());
        assert_eq!(session.vm_id, Some("vm-123".to_string()));
    }

    #[test]
    fn test_session_touch() {
        let mut session = Session::new_shell("vm-789".to_string());
        let initial = session.last_active;

        std::thread::sleep(std::time::Duration::from_millis(10));
        session.touch();

        assert!(session.last_active.monotonic_seq > initial.monotonic_seq);
    }
}
