//! Session registry - tracks all active sessions
//!
//! The registry is the authority for session validation. All API requests
//! must have their session token validated through this registry.
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{Session, SessionType};
use crate::primitives::audit::AuditLog;
use crate::primitives::time::SecureTimestamp;

/// Session registry - manages all active sessions
pub struct SessionRegistry {
    /// Active sessions indexed by token
    sessions: RwLock<HashMap<String, Session>>,

    /// Mapping from VM ID to session token (for cleanup when VM dies)
    vm_sessions: RwLock<HashMap<String, String>>,

    /// Stable owner applied to new sessions unless an authenticated identity replaces it.
    default_owner: RwLock<Option<String>>,

    /// Audit log for session events
    audit_log: Arc<AuditLog>,
}

impl SessionRegistry {
    /// Create a new session registry
    pub fn new(audit_log: Arc<AuditLog>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            vm_sessions: RwLock::new(HashMap::new()),
            default_owner: RwLock::new(None),
            audit_log,
        }
    }

    /// Set the stable owner applied to newly created sessions.
    pub async fn set_default_owner(&self, owner: String) {
        let mut default_owner = self.default_owner.write().await;
        *default_owner = Some(owner);
    }

    /// Create a new session and register it
    ///
    /// Returns the session (including the token that must be passed to the VM)
    pub async fn create_session(
        &self,
        session_type: SessionType,
        vm_id: Option<String>,
    ) -> Session {
        let default_owner = self.default_owner.read().await.clone();
        let session = match default_owner {
            Some(owner) => Session::with_owner(session_type, vm_id.clone(), owner),
            None => Session::new(session_type, vm_id.clone()),
        };

        // Store session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.token.clone(), session.clone());
        }

        // Store VM -> token mapping if VM-bound
        if let Some(ref vm_id) = vm_id {
            let mut vm_sessions = self.vm_sessions.write().await;
            vm_sessions.insert(vm_id.clone(), session.token.clone());
        }

        // Audit
        self.audit_log
            .emit(crate::primitives::audit::AuditEvent::SessionCreated {
                timestamp: SecureTimestamp::now(),
                session_id: session.id.to_string(),
                session_type: session.session_type.to_string(),
                vm_id,
            });

        session
    }

    /// Validate a bearer token and return the session if valid
    pub async fn validate_token(&self, token: &str) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(token).cloned()
    }

    /// Get a session by token (mutable access for updates)
    pub async fn get_session_mut<F, R>(&self, token: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut sessions = self.sessions.write().await;
        sessions.get_mut(token).map(f)
    }

    /// Update last activity for a session
    pub async fn touch_session(&self, token: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(token) {
            session.touch();
        }
    }

    /// Check if a token belongs to a shell session
    pub async fn is_shell(&self, token: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions.get(token).map(|s| s.is_shell()).unwrap_or(false)
    }

    /// Invalidate a session by token
    pub async fn invalidate_session(&self, token: &str) -> Option<Session> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(token)
        };

        if let Some(ref session) = session {
            // Remove VM mapping if exists
            if let Some(ref vm_id) = session.vm_id {
                let mut vm_sessions = self.vm_sessions.write().await;
                vm_sessions.remove(vm_id);
            }

            // Audit
            self.audit_log
                .emit(crate::primitives::audit::AuditEvent::SessionDestroyed {
                    timestamp: SecureTimestamp::now(),
                    session_id: session.id.to_string(),
                    reason: "invalidated".to_string(),
                });
        }

        session
    }

    /// Invalidate session for a specific VM (called when VM dies)
    pub async fn invalidate_vm_session(&self, vm_id: &str) -> Option<Session> {
        let token = {
            let vm_sessions = self.vm_sessions.read().await;
            vm_sessions.get(vm_id).cloned()
        };

        if let Some(token) = token {
            let session = {
                let mut sessions = self.sessions.write().await;
                sessions.remove(&token)
            };

            // Remove VM mapping
            {
                let mut vm_sessions = self.vm_sessions.write().await;
                vm_sessions.remove(vm_id);
            }

            if let Some(ref session) = session {
                // Audit
                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::SessionDestroyed {
                        timestamp: SecureTimestamp::now(),
                        session_id: session.id.to_string(),
                        reason: format!("vm_terminated:{}", vm_id),
                    });
            }

            session
        } else {
            None
        }
    }

    /// Get all active sessions (for debugging/admin)
    pub async fn list_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    /// Get session count
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Check if a VM has an active session
    pub async fn has_vm_session(&self, vm_id: &str) -> bool {
        let vm_sessions = self.vm_sessions.read().await;
        vm_sessions.contains_key(vm_id)
    }

    /// Get session token for a VM
    pub async fn get_vm_token(&self, vm_id: &str) -> Option<String> {
        let vm_sessions = self.vm_sessions.read().await;
        vm_sessions.get(vm_id).cloned()
    }

    /// Cleanup stale sessions (called periodically)
    ///
    /// Removes sessions that haven't been active for the specified duration
    pub async fn cleanup_stale_sessions(&self, max_idle_secs: u64) -> usize {
        let now = SecureTimestamp::now();
        let mut removed = Vec::new();

        {
            let mut sessions = self.sessions.write().await;
            sessions.retain(|token, session| {
                let idle_secs = now.unix_secs.saturating_sub(session.last_active.unix_secs);
                if idle_secs > max_idle_secs {
                    removed.push((token.clone(), session.clone()));
                    false
                } else {
                    true
                }
            });
        }

        // Remove VM mappings and audit
        for (token, session) in &removed {
            if let Some(ref vm_id) = session.vm_id {
                let mut vm_sessions = self.vm_sessions.write().await;
                vm_sessions.remove(vm_id);
            }

            self.audit_log
                .emit(crate::primitives::audit::AuditEvent::SessionDestroyed {
                    timestamp: SecureTimestamp::now(),
                    session_id: session.id.to_string(),
                    reason: format!(
                        "stale_cleanup:idle_{}s",
                        now.unix_secs.saturating_sub(session.last_active.unix_secs)
                    ),
                });

            tracing::info!(
                session_id = %session.id,
                token = %token[..8.min(token.len())],
                "Cleaned up stale session"
            );
        }

        removed.len()
    }

    /// Check if sessions exist for a list of VM IDs and remove dead ones
    ///
    /// This is called with a list of live VM IDs; any sessions for VMs
    /// not in the list are invalidated.
    pub async fn cleanup_dead_vm_sessions(&self, live_vm_ids: &[String]) -> usize {
        let vm_tokens: Vec<(String, String)> = {
            let vm_sessions = self.vm_sessions.read().await;
            vm_sessions
                .iter()
                .filter(|(vm_id, _)| !live_vm_ids.contains(vm_id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        let mut count = 0;
        for (vm_id, _) in vm_tokens {
            if self.invalidate_vm_session(&vm_id).await.is_some() {
                count += 1;
            }
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_registry() -> SessionRegistry {
        SessionRegistry::new(Arc::new(AuditLog::new()))
    }

    #[tokio::test]
    async fn test_create_session() {
        let registry = create_test_registry();

        let session = registry
            .create_session(SessionType::Shell, Some("vm-1".to_string()))
            .await;

        assert!(session.is_shell());
        assert_eq!(session.vm_id, Some("vm-1".to_string()));
        assert!(!session.token.is_empty());

        // Should be able to validate the token
        let validated = registry.validate_token(&session.token).await;
        assert!(validated.is_some());
        assert_eq!(validated.unwrap().id, session.id);
    }

    #[tokio::test]
    async fn test_create_session_uses_default_owner() {
        let registry = create_test_registry();
        registry
            .set_default_owner(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            )
            .await;

        let session = registry.create_session(SessionType::Capsule, None).await;

        assert_eq!(
            session.owner.as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
    }

    #[tokio::test]
    async fn test_invalid_token() {
        let registry = create_test_registry();

        let validated = registry.validate_token("invalid-token").await;
        assert!(validated.is_none());
    }

    #[tokio::test]
    async fn test_invalidate_session() {
        let registry = create_test_registry();

        let session = registry.create_session(SessionType::Capsule, None).await;
        let token = session.token.clone();

        // Should exist
        assert!(registry.validate_token(&token).await.is_some());

        // Invalidate
        let removed = registry.invalidate_session(&token).await;
        assert!(removed.is_some());

        // Should no longer exist
        assert!(registry.validate_token(&token).await.is_none());
    }

    #[tokio::test]
    async fn test_vm_session_mapping() {
        let registry = create_test_registry();

        let session = registry
            .create_session(SessionType::Shell, Some("vm-test".to_string()))
            .await;

        // Should have VM mapping
        assert!(registry.has_vm_session("vm-test").await);
        assert_eq!(
            registry.get_vm_token("vm-test").await,
            Some(session.token.clone())
        );

        // Invalidate by VM ID
        let removed = registry.invalidate_vm_session("vm-test").await;
        assert!(removed.is_some());

        // Mappings should be gone
        assert!(!registry.has_vm_session("vm-test").await);
        assert!(registry.validate_token(&session.token).await.is_none());
    }

    #[tokio::test]
    async fn test_is_shell() {
        let registry = create_test_registry();

        let shell = registry.create_session(SessionType::Shell, None).await;
        let capsule = registry.create_session(SessionType::Capsule, None).await;

        assert!(registry.is_shell(&shell.token).await);
        assert!(!registry.is_shell(&capsule.token).await);
        assert!(!registry.is_shell("nonexistent").await);
    }

    #[tokio::test]
    async fn test_touch_session() {
        let registry = create_test_registry();

        let session = registry.create_session(SessionType::Shell, None).await;
        let initial_active = session.last_active;

        std::thread::sleep(std::time::Duration::from_millis(10));
        registry.touch_session(&session.token).await;

        let updated = registry.validate_token(&session.token).await.unwrap();
        assert!(updated.last_active.monotonic_seq > initial_active.monotonic_seq);
    }

    #[tokio::test]
    async fn test_session_count() {
        let registry = create_test_registry();

        assert_eq!(registry.session_count().await, 0);

        registry.create_session(SessionType::Shell, None).await;
        assert_eq!(registry.session_count().await, 1);

        let session2 = registry.create_session(SessionType::Capsule, None).await;
        assert_eq!(registry.session_count().await, 2);

        registry.invalidate_session(&session2.token).await;
        assert_eq!(registry.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let registry = create_test_registry();

        registry
            .create_session(SessionType::Shell, Some("vm-a".to_string()))
            .await;
        registry
            .create_session(SessionType::Capsule, Some("vm-b".to_string()))
            .await;

        let sessions = registry.list_sessions().await;
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_cleanup_dead_vm_sessions() {
        let registry = create_test_registry();

        registry
            .create_session(SessionType::Shell, Some("vm-alive".to_string()))
            .await;
        registry
            .create_session(SessionType::Shell, Some("vm-dead".to_string()))
            .await;

        // Only vm-alive is in the live list
        let removed = registry
            .cleanup_dead_vm_sessions(&["vm-alive".to_string()])
            .await;
        assert_eq!(removed, 1);

        // vm-alive should still exist
        assert!(registry.has_vm_session("vm-alive").await);

        // vm-dead should be gone
        assert!(!registry.has_vm_session("vm-dead").await);
    }

    // === Session security tests: hijacking, reuse, expiration races ===

    #[tokio::test]
    async fn test_invalidated_token_cannot_be_reused() {
        let registry = create_test_registry();
        let session = registry
            .create_session(SessionType::Shell, Some("vm-1".to_string()))
            .await;
        let token = session.token.clone();

        // Valid before invalidation
        assert!(registry.validate_token(&token).await.is_some());

        // Invalidate
        registry.invalidate_session(&token).await;

        // Must fail after invalidation
        assert!(
            registry.validate_token(&token).await.is_none(),
            "Invalidated token must not validate"
        );
    }

    #[tokio::test]
    async fn test_token_uniqueness_across_sessions() {
        let registry = create_test_registry();
        let mut tokens = std::collections::HashSet::new();

        for i in 0..100 {
            let session = registry
                .create_session(SessionType::Capsule, Some(format!("vm-{}", i)))
                .await;
            assert!(
                tokens.insert(session.token.clone()),
                "Duplicate token detected at session {}",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_session_id_uniqueness_across_sessions() {
        let registry = create_test_registry();
        let mut ids = std::collections::HashSet::new();

        for i in 0..100 {
            let session = registry
                .create_session(SessionType::Capsule, Some(format!("vm-{}", i)))
                .await;
            assert!(
                ids.insert(session.id.to_string()),
                "Duplicate session ID at session {}",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_stale_cleanup_removes_idle_sessions() {
        let registry = create_test_registry();
        let session = registry
            .create_session(SessionType::Capsule, Some("vm-stale".to_string()))
            .await;

        // Age the session by setting last_active far in the past
        registry
            .get_session_mut(&session.token, |s| {
                s.last_active = SecureTimestamp {
                    unix_secs: 1_000_000,
                    monotonic_seq: 0,
                };
            })
            .await;

        // With max_idle_secs=60, a session last active at epoch+1M is definitely stale
        let removed = registry.cleanup_stale_sessions(60).await;
        assert_eq!(removed, 1);

        // Session must be gone
        assert!(
            registry.validate_token(&session.token).await.is_none(),
            "Stale-cleaned session must not validate"
        );
    }

    #[tokio::test]
    async fn test_stale_cleanup_preserves_active_sessions() {
        let registry = create_test_registry();
        let session = registry
            .create_session(SessionType::Shell, Some("vm-active".to_string()))
            .await;

        // With a generous idle window, the session should survive
        let removed = registry.cleanup_stale_sessions(86400).await;
        assert_eq!(removed, 0);

        assert!(
            registry.validate_token(&session.token).await.is_some(),
            "Active session must survive cleanup"
        );
    }

    #[tokio::test]
    async fn test_vm_crash_invalidates_session() {
        let registry = create_test_registry();
        let session = registry
            .create_session(SessionType::Capsule, Some("vm-crash".to_string()))
            .await;

        // Simulate VM crash — vm-crash is NOT in the live list
        let removed = registry.cleanup_dead_vm_sessions(&[]).await;
        assert_eq!(removed, 1);

        // The crashed VM's session must be invalid
        assert!(
            registry.validate_token(&session.token).await.is_none(),
            "Crashed VM session must not validate"
        );
    }

    #[tokio::test]
    async fn test_wrong_token_never_validates() {
        let registry = create_test_registry();
        registry
            .create_session(SessionType::Shell, Some("vm-1".to_string()))
            .await;

        assert!(registry.validate_token("bogus-token").await.is_none());
        assert!(registry.validate_token("").await.is_none());
        assert!(registry
            .validate_token("00000000-0000-0000-0000-000000000000")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn test_concurrent_session_isolation() {
        let registry = Arc::new(create_test_registry());

        // Create two sessions
        let s1 = registry
            .create_session(SessionType::Shell, Some("vm-a".to_string()))
            .await;
        let s2 = registry
            .create_session(SessionType::Capsule, Some("vm-b".to_string()))
            .await;

        // Invalidate one
        registry.invalidate_session(&s1.token).await;

        // The other must still work
        assert!(registry.validate_token(&s1.token).await.is_none());
        assert!(registry.validate_token(&s2.token).await.is_some());
    }
}
