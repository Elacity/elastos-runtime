//! Audit logging for ElastOS
//!
//! Runtime generates audit events at every security-relevant operation.
//! These events CANNOT be bypassed by any capsule, including the shell.
//!
//! Phase 3: Simple file-based logging
//! Later: Tamper-evident storage, cryptographic chaining, audit capsule
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use super::time::SecureTimestamp;
use crate::capability::token::{Action, ResourceId, TokenId};

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    /// Runtime started
    RuntimeStart {
        timestamp: SecureTimestamp,
        version: String,
    },

    /// Runtime stopped
    RuntimeStop { timestamp: SecureTimestamp },

    /// Capsule launched
    CapsuleLaunch {
        timestamp: SecureTimestamp,
        capsule_id: String,
        capsule_name: String,
        cid: Option<String>,
        trust_level: TrustLevel,
    },

    /// Capsule stopped
    CapsuleStop {
        timestamp: SecureTimestamp,
        capsule_id: String,
        reason: StopReason,
    },

    /// Capability granted
    CapabilityGrant {
        timestamp: SecureTimestamp,
        token_id: String,
        capsule_id: String,
        resource: String,
        action: String,
        expiry: Option<SecureTimestamp>,
    },

    /// Capability revoked
    CapabilityRevoke {
        timestamp: SecureTimestamp,
        token_id: String,
        reason: String,
    },

    /// Capability used
    CapabilityUse {
        timestamp: SecureTimestamp,
        token_id: String,
        capsule_id: String,
        resource: String,
        action: String,
        success: bool,
    },

    /// Content fetched via elastos://
    ContentFetch {
        timestamp: SecureTimestamp,
        cid: String,
        source: FetchSource,
        success: bool,
    },

    /// Authentication attempt
    AuthAttempt {
        timestamp: SecureTimestamp,
        identity: String,
        success: bool,
        method: String,
    },

    /// Epoch advanced (mass revocation)
    EpochAdvance {
        timestamp: SecureTimestamp,
        old_epoch: u64,
        new_epoch: u64,
        reason: String,
    },

    /// Configuration changed
    ConfigChange {
        timestamp: SecureTimestamp,
        setting: String,
        old_value: String,
        new_value: String,
    },

    /// Security warning
    SecurityWarning {
        timestamp: SecureTimestamp,
        warning_type: String,
        details: String,
    },

    /// Session created
    SessionCreated {
        timestamp: SecureTimestamp,
        session_id: String,
        session_type: String,
        vm_id: Option<String>,
    },

    /// Session destroyed
    SessionDestroyed {
        timestamp: SecureTimestamp,
        session_id: String,
        reason: String,
    },

    /// Capability requested (pending approval)
    CapabilityRequested {
        timestamp: SecureTimestamp,
        request_id: String,
        session_id: String,
        resource: String,
        action: String,
    },

    /// Capability request denied
    CapabilityDenied {
        timestamp: SecureTimestamp,
        request_id: String,
        session_id: String,
        reason: String,
    },

    /// Identity registered (passkey)
    IdentityRegistered {
        timestamp: SecureTimestamp,
        user_id: String,
        method: String,
    },

    /// Storage access via provider
    StorageAccess {
        timestamp: SecureTimestamp,
        session_id: String,
        user_id: String,
        uri: String,
        action: String,
        success: bool,
    },

    /// Inter-capsule message sent
    MessageSent {
        timestamp: SecureTimestamp,
        from: String,
        to: String,
        size_bytes: usize,
    },

    /// Policy proposal (advisory recommendation from proposer)
    PolicyProposal {
        timestamp: SecureTimestamp,
        request_id: String,
        recommended_outcome: String,
        confidence: f32,
        rationale: String,
    },

    /// Policy decision made (authoritative verifier decision)
    PolicyDecisionMade {
        timestamp: SecureTimestamp,
        decision_id: String,
        request_id: String,
        outcome: String,
        checks_passed: usize,
        checks_failed: usize,
        shadow: bool,
        rationale: String,
    },

    /// Policy divergence (real and shadow verifiers disagree)
    PolicyDivergence {
        timestamp: SecureTimestamp,
        request_id: String,
        real_decision_id: String,
        shadow_decision_id: String,
        real_outcome: String,
        shadow_outcome: String,
        real_rationale: String,
        shadow_rationale: String,
    },

    /// Custom event for extensibility
    Custom {
        event_type: String,
        details: serde_json::Value,
    },
}

/// Trust level for capsules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// Signed by root-trusted key
    Trusted,
    /// Signed by known community key
    Community,
    /// Unsigned or unknown signer
    Untrusted,
}

/// Reason for capsule stop
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Normal stop requested
    Requested,
    /// Capsule exited normally
    Completed,
    /// Capsule crashed/errored
    Error(String),
    /// Resource limit exceeded
    ResourceLimit(String),
    /// Security violation
    SecurityViolation(String),
}

/// Source of fetched content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchSource {
    LocalCache,
    IpfsGateway(String),
    Peer(String),
}

/// Maximum events to keep in memory buffer
const MAX_MEMORY_EVENTS: usize = 1000;

/// Audit log manager
pub struct AuditLog {
    writer: Option<Mutex<BufWriter<File>>>,
    log_path: Option<PathBuf>,
    /// Also write to stdout (for development)
    echo_stdout: bool,
    /// In-memory buffer of recent events (ring buffer)
    memory_buffer: RwLock<VecDeque<AuditEvent>>,
}

impl AuditLog {
    /// Create a new audit log without file output (memory only, for testing)
    pub fn new() -> Self {
        Self {
            writer: None,
            log_path: None,
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
        }
    }

    /// Create an audit log that writes to the given path
    pub fn with_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        let writer = BufWriter::new(file);

        Ok(Self {
            writer: Some(Mutex::new(writer)),
            log_path: Some(path),
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
        })
    }

    /// Enable/disable echoing to stdout
    pub fn set_echo_stdout(&mut self, echo: bool) {
        self.echo_stdout = echo;
    }

    /// Emit an audit event
    ///
    /// This is the ONLY way to create audit records. Capsules cannot call this directly.
    pub fn emit(&self, event: AuditEvent) {
        // Store in memory buffer
        {
            if let Ok(mut buffer) = self.memory_buffer.write() {
                if buffer.len() >= MAX_MEMORY_EVENTS {
                    buffer.pop_front();
                }
                buffer.push_back(event.clone());
            }
        }

        let json = match serde_json::to_string(&event) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Audit event serialization failed: {}", e);
                return;
            }
        };

        // Echo to stdout if enabled
        if self.echo_stdout {
            println!("[AUDIT] {}", json);
        }

        // Write to file if configured
        if let Some(writer) = &self.writer {
            if let Ok(mut w) = writer.lock() {
                if let Err(e) = writeln!(w, "{}", json) {
                    tracing::error!("Audit event write failed: {}", e);
                }
                // Flush to ensure durability
                let _ = w.flush();
            }
        }
    }

    /// Emit a runtime start event
    pub fn runtime_start(&self, version: &str) {
        self.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: version.to_string(),
        });
    }

    /// Emit a runtime stop event
    pub fn runtime_stop(&self) {
        self.emit(AuditEvent::RuntimeStop {
            timestamp: SecureTimestamp::now(),
        });
    }

    /// Emit a capsule launch event
    pub fn capsule_launch(
        &self,
        capsule_id: &str,
        capsule_name: &str,
        cid: Option<&str>,
        trust_level: TrustLevel,
    ) {
        self.emit(AuditEvent::CapsuleLaunch {
            timestamp: SecureTimestamp::now(),
            capsule_id: capsule_id.to_string(),
            capsule_name: capsule_name.to_string(),
            cid: cid.map(String::from),
            trust_level,
        });
    }

    /// Emit a capsule stop event
    pub fn capsule_stop(&self, capsule_id: &str, reason: StopReason) {
        self.emit(AuditEvent::CapsuleStop {
            timestamp: SecureTimestamp::now(),
            capsule_id: capsule_id.to_string(),
            reason,
        });
    }

    /// Emit a capability grant event
    pub fn capability_grant(
        &self,
        token_id: &TokenId,
        capsule_id: &str,
        resource: &ResourceId,
        action: Action,
        expiry: Option<SecureTimestamp>,
    ) {
        self.emit(AuditEvent::CapabilityGrant {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            expiry,
        });
    }

    /// Emit a capability revoke event
    pub fn capability_revoke(&self, token_id: &TokenId, reason: &str) {
        self.emit(AuditEvent::CapabilityRevoke {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            reason: reason.to_string(),
        });
    }

    /// Emit a capability use event
    pub fn capability_use(
        &self,
        token_id: &TokenId,
        capsule_id: &str,
        resource: &ResourceId,
        action: Action,
        success: bool,
    ) {
        self.emit(AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            success,
        });
    }

    /// Emit a content fetch event
    pub fn content_fetch(&self, cid: &str, source: FetchSource, success: bool) {
        self.emit(AuditEvent::ContentFetch {
            timestamp: SecureTimestamp::now(),
            cid: cid.to_string(),
            source,
            success,
        });
    }

    /// Emit an epoch advance event
    pub fn epoch_advance(&self, old_epoch: u64, new_epoch: u64, reason: &str) {
        self.emit(AuditEvent::EpochAdvance {
            timestamp: SecureTimestamp::now(),
            old_epoch,
            new_epoch,
            reason: reason.to_string(),
        });
    }

    /// Emit a storage access event
    pub fn storage_access(
        &self,
        session_id: &str,
        user_id: &str,
        uri: &str,
        action: &str,
        success: bool,
    ) {
        self.emit(AuditEvent::StorageAccess {
            timestamp: SecureTimestamp::now(),
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            uri: uri.to_string(),
            action: action.to_string(),
            success,
        });
    }

    /// Emit a security warning
    pub fn security_warning(&self, warning_type: &str, details: &str) {
        self.emit(AuditEvent::SecurityWarning {
            timestamp: SecureTimestamp::now(),
            warning_type: warning_type.to_string(),
            details: details.to_string(),
        });
    }

    /// Record an inter-capsule message
    pub fn message_sent(&self, from: &str, to: &str, size_bytes: usize) {
        self.emit(AuditEvent::MessageSent {
            timestamp: SecureTimestamp::now(),
            from: from.to_string(),
            to: to.to_string(),
            size_bytes,
        });
    }

    /// Emit a policy proposal event
    pub fn policy_proposal(
        &self,
        request_id: &str,
        recommended_outcome: &str,
        confidence: f32,
        rationale: &str,
    ) {
        self.emit(AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: request_id.to_string(),
            recommended_outcome: recommended_outcome.to_string(),
            confidence,
            rationale: rationale.to_string(),
        });
    }

    /// Emit a policy divergence event (real and shadow verifiers disagree)
    #[allow(clippy::too_many_arguments)]
    pub fn policy_divergence(
        &self,
        request_id: &str,
        real_decision_id: &str,
        shadow_decision_id: &str,
        real_outcome: &str,
        shadow_outcome: &str,
        real_rationale: &str,
        shadow_rationale: &str,
    ) {
        self.emit(AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: request_id.to_string(),
            real_decision_id: real_decision_id.to_string(),
            shadow_decision_id: shadow_decision_id.to_string(),
            real_outcome: real_outcome.to_string(),
            shadow_outcome: shadow_outcome.to_string(),
            real_rationale: real_rationale.to_string(),
            shadow_rationale: shadow_rationale.to_string(),
        });
    }

    /// Emit a policy decision made event
    #[allow(clippy::too_many_arguments)]
    pub fn policy_decision_made(
        &self,
        decision_id: &str,
        request_id: &str,
        outcome: &str,
        checks_passed: usize,
        checks_failed: usize,
        shadow: bool,
        rationale: &str,
    ) {
        self.emit(AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: decision_id.to_string(),
            request_id: request_id.to_string(),
            outcome: outcome.to_string(),
            checks_passed,
            checks_failed,
            shadow,
            rationale: rationale.to_string(),
        });
    }

    /// Get the log file path (if configured)
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// Get recent events from memory buffer
    ///
    /// Returns the most recent `limit` events, newest first.
    pub fn recent_events(&self, limit: usize) -> Vec<AuditEvent> {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer.iter().rev().take(limit).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get recent events filtered by type
    ///
    /// Returns the most recent events matching the filter, newest first.
    pub fn recent_events_filtered(
        &self,
        limit: usize,
        event_type: Option<&str>,
    ) -> Vec<AuditEvent> {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer
                .iter()
                .rev()
                .filter(|e| {
                    if let Some(filter) = event_type {
                        e.event_type_name() == filter
                    } else {
                        true
                    }
                })
                .take(limit)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get events from file (reads entire log file)
    ///
    /// Returns all events from file, or events from memory if no file configured.
    /// For large logs, prefer recent_events() which uses the memory buffer.
    pub fn read_from_file(&self, limit: usize) -> Vec<AuditEvent> {
        if let Some(path) = &self.log_path {
            if let Ok(file) = File::open(path) {
                let reader = BufReader::new(file);
                let events: Vec<AuditEvent> = reader
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str(&line).ok())
                    .collect();

                // Return last `limit` events
                let start = events.len().saturating_sub(limit);
                return events[start..].to_vec();
            }
        }

        // Fall back to memory buffer
        self.recent_events(limit)
    }

    /// Get total event count in memory buffer
    pub fn event_count(&self) -> usize {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer.len()
        } else {
            0
        }
    }
}

impl AuditEvent {
    /// Get the event type name as a string
    pub fn event_type_name(&self) -> &'static str {
        match self {
            AuditEvent::RuntimeStart { .. } => "runtime_start",
            AuditEvent::RuntimeStop { .. } => "runtime_stop",
            AuditEvent::CapsuleLaunch { .. } => "capsule_launch",
            AuditEvent::CapsuleStop { .. } => "capsule_stop",
            AuditEvent::CapabilityGrant { .. } => "capability_grant",
            AuditEvent::CapabilityRevoke { .. } => "capability_revoke",
            AuditEvent::CapabilityUse { .. } => "capability_use",
            AuditEvent::ContentFetch { .. } => "content_fetch",
            AuditEvent::AuthAttempt { .. } => "auth_attempt",
            AuditEvent::EpochAdvance { .. } => "epoch_advance",
            AuditEvent::ConfigChange { .. } => "config_change",
            AuditEvent::SecurityWarning { .. } => "security_warning",
            AuditEvent::SessionCreated { .. } => "session_created",
            AuditEvent::SessionDestroyed { .. } => "session_destroyed",
            AuditEvent::CapabilityRequested { .. } => "capability_requested",
            AuditEvent::CapabilityDenied { .. } => "capability_denied",
            AuditEvent::IdentityRegistered { .. } => "identity_registered",
            AuditEvent::StorageAccess { .. } => "storage_access",
            AuditEvent::MessageSent { .. } => "message_sent",
            AuditEvent::PolicyProposal { .. } => "policy_proposal",
            AuditEvent::PolicyDecisionMade { .. } => "policy_decision_made",
            AuditEvent::PolicyDivergence { .. } => "policy_divergence",
            AuditEvent::Custom { .. } => "custom",
        }
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// Bridge: implement namespace crate's audit traits for the runtime's AuditLog

impl elastos_namespace::AuditSink for AuditLog {
    fn content_fetch(
        &self,
        identifier: &str,
        source: elastos_namespace::FetchSource,
        verified: bool,
    ) {
        let runtime_source = match source {
            elastos_namespace::FetchSource::LocalCache => FetchSource::LocalCache,
            elastos_namespace::FetchSource::IpfsGateway(gw) => FetchSource::IpfsGateway(gw),
        };
        self.content_fetch(identifier, runtime_source, verified);
    }
}

impl elastos_namespace::NamespaceAuditSink for AuditLog {
    fn namespace_loaded(&self, owner: &str) {
        self.emit(AuditEvent::Custom {
            event_type: "namespace_loaded".to_string(),
            details: serde_json::json!({ "owner": owner }),
        });
    }

    fn namespace_created(&self, owner: &str) {
        self.emit(AuditEvent::Custom {
            event_type: "namespace_created".to_string(),
            details: serde_json::json!({ "owner": owner }),
        });
    }

    fn namespace_saved(&self, owner: &str, cid: &str) {
        self.emit(AuditEvent::Custom {
            event_type: "namespace_saved".to_string(),
            details: serde_json::json!({ "owner": owner, "cid": cid }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent::CapsuleLaunch {
            timestamp: SecureTimestamp::now(),
            capsule_id: "cap-123".to_string(),
            capsule_name: "test-capsule".to_string(),
            cid: Some("Qm123".to_string()),
            trust_level: TrustLevel::Trusted,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("capsule_launch"));
        assert!(json.contains("cap-123"));
    }

    #[test]
    fn test_audit_log_memory() {
        let log = AuditLog::new();
        log.runtime_start("0.1.0");
        log.capsule_launch("cap-1", "test", None, TrustLevel::Untrusted);
        log.capsule_stop("cap-1", StopReason::Completed);
        log.runtime_stop();
        // No panic = success (memory-only log doesn't persist)
    }

    #[test]
    fn test_audit_log_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("audit.log");

        let log = AuditLog::with_file(&log_path).unwrap();
        log.runtime_start("0.1.0");
        log.capsule_launch("cap-1", "test", None, TrustLevel::Trusted);

        // Read back the log
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("runtime_start"));
        assert!(content.contains("capsule_launch"));
    }

    #[test]
    fn test_policy_proposal_event_serialization() {
        let event = AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: "req-001".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.9,
            rationale: "User granted before".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_proposal\""));
        assert!(json.contains("req-001"));
        assert!(json.contains("0.9"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_policy_decision_made_event_serialization() {
        let event = AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: "dec-001".to_string(),
            request_id: "req-001".to_string(),
            outcome: "grant".to_string(),
            checks_passed: 3,
            checks_failed: 0,
            shadow: false,
            rationale: "All checks passed".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_decision_made\""));
        assert!(json.contains("dec-001"));
        assert!(json.contains("\"shadow\":false"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_decision_made");
    }

    #[test]
    fn test_policy_event_type_names() {
        let proposal = AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: "r".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.5,
            rationale: "test".to_string(),
        };
        assert_eq!(proposal.event_type_name(), "policy_proposal");

        let decision = AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: "d".to_string(),
            request_id: "r".to_string(),
            outcome: "deny".to_string(),
            checks_passed: 1,
            checks_failed: 2,
            shadow: true,
            rationale: "test".to_string(),
        };
        assert_eq!(decision.event_type_name(), "policy_decision_made");
    }

    #[test]
    fn test_policy_divergence_event_serialization() {
        let event = AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: "req-001".to_string(),
            real_decision_id: "dec-real".to_string(),
            shadow_decision_id: "dec-shadow".to_string(),
            real_outcome: "deny".to_string(),
            shadow_outcome: "grant".to_string(),
            real_rationale: "Denied by user".to_string(),
            shadow_rationale: "Auto-grant: all requests approved".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_divergence\""));
        assert!(json.contains("req-001"));
        assert!(json.contains("dec-real"));
        assert!(json.contains("dec-shadow"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_divergence");
    }

    #[test]
    fn test_policy_divergence_event_type_name() {
        let event = AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: "r".to_string(),
            real_decision_id: "d1".to_string(),
            shadow_decision_id: "d2".to_string(),
            real_outcome: "deny".to_string(),
            shadow_outcome: "grant".to_string(),
            real_rationale: "test".to_string(),
            shadow_rationale: "test".to_string(),
        };
        assert_eq!(event.event_type_name(), "policy_divergence");
    }
}
