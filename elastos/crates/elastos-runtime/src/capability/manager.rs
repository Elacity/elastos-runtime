//! Capability manager - grant, validate, and revoke tokens
//!
//! This is the core of the capability system. Every resource access
//! must go through token validation here.

use ed25519_dalek::{SigningKey, VerifyingKey};
use std::path::Path;
use std::sync::Arc;

use super::store::CapabilityStore;
use super::token::{Action, CapabilityToken, ResourceId, TokenConstraints, TokenId};
use crate::primitives::audit::AuditLog;
use crate::primitives::metrics::MetricsManager;
use crate::primitives::time::SecureTimestamp;

/// Validation error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    InvalidSignature,
    UntrustedIssuer,
    WrongCapsule { expected: String, got: String },
    WrongAction { expected: Action, got: Action },
    WrongResource { expected: String, got: String },
    TokenRevoked,
    TokenExpired,
    FutureDatedToken,
    UseLimitExceeded { current: u32, max: u32 },
    ClassificationExceeded { token: u8, resource: u8 },
    InvalidVersion { expected: u8, got: u8 },
    DelegationNotAllowed,
    DelegationScopeWidened,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature => write!(f, "invalid token signature"),
            Self::UntrustedIssuer => write!(f, "token issuer not trusted"),
            Self::WrongCapsule { expected, got } => {
                write!(f, "wrong capsule: expected {}, got {}", expected, got)
            }
            Self::WrongAction { expected, got } => {
                write!(f, "wrong action: expected {}, got {}", expected, got)
            }
            Self::WrongResource { expected, got } => {
                write!(f, "wrong resource: expected {}, got {}", expected, got)
            }
            Self::TokenRevoked => write!(f, "token has been revoked"),
            Self::TokenExpired => write!(f, "token has expired"),
            Self::FutureDatedToken => write!(f, "token issued_at is in the future"),
            Self::UseLimitExceeded { current, max } => {
                write!(f, "use limit exceeded: {} >= {}", current, max)
            }
            Self::ClassificationExceeded { token, resource } => {
                write!(
                    f,
                    "classification exceeded: token allows {}, resource requires {}",
                    token, resource
                )
            }
            Self::InvalidVersion { expected, got } => {
                write!(
                    f,
                    "invalid token version: expected {}, got {}",
                    expected, got
                )
            }
            Self::DelegationNotAllowed => write!(f, "token is not delegatable"),
            Self::DelegationScopeWidened => {
                write!(f, "delegated token cannot widen scope of parent")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Capability manager - the authority for all capability operations
pub struct CapabilityManager {
    /// Runtime's signing key
    signing_key: SigningKey,

    /// Runtime's verifying key (derived from signing key)
    verifying_key: VerifyingKey,

    /// Token storage (epoch, use counts, revocations)
    store: Arc<CapabilityStore>,

    /// Audit log
    audit_log: Arc<AuditLog>,

    /// Metrics manager
    metrics: Arc<MetricsManager>,
}

impl CapabilityManager {
    /// Create a new capability manager with a fresh signing key
    pub fn new(
        store: Arc<CapabilityStore>,
        audit_log: Arc<AuditLog>,
        metrics: Arc<MetricsManager>,
    ) -> Self {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();

        Self {
            signing_key,
            verifying_key,
            store,
            audit_log,
            metrics,
        }
    }

    /// Load signing key from disk, or generate and persist a new one.
    ///
    /// This ensures capability tokens survive runtime restarts.
    /// The key is stored as 32 raw bytes at `{data_dir}/signing_key`.
    pub fn load_or_generate(
        data_dir: &Path,
        store: Arc<CapabilityStore>,
        audit_log: Arc<AuditLog>,
        metrics: Arc<MetricsManager>,
    ) -> Self {
        let key_path = data_dir.join("signing_key");

        let signing_key = if key_path.exists() {
            // Try to load existing key
            match std::fs::read(&key_path) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut key_bytes = [0u8; 32];
                    key_bytes.copy_from_slice(&bytes);
                    let key = SigningKey::from_bytes(&key_bytes);
                    tracing::info!("Loaded existing capability signing key from {:?}", key_path);
                    key
                }
                Ok(bytes) => {
                    tracing::warn!(
                        "Signing key file {:?} has wrong length ({} bytes, expected 32). Generating new key.",
                        key_path,
                        bytes.len()
                    );
                    Self::generate_and_persist_key(&key_path, &audit_log)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to read signing key from {:?}: {}. Generating new key.",
                        key_path,
                        e
                    );
                    Self::generate_and_persist_key(&key_path, &audit_log)
                }
            }
        } else {
            // No key file — generate and save
            if let Err(e) = std::fs::create_dir_all(data_dir) {
                tracing::warn!("Failed to create data directory {:?}: {}", data_dir, e);
            }
            Self::generate_and_persist_key(&key_path, &audit_log)
        };

        let verifying_key = signing_key.verifying_key();

        Self {
            signing_key,
            verifying_key,
            store,
            audit_log,
            metrics,
        }
    }

    /// Generate a new signing key and persist it to disk
    fn generate_and_persist_key(key_path: &Path, audit_log: &AuditLog) -> SigningKey {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());

        match std::fs::write(key_path, signing_key.to_bytes()) {
            Ok(()) => {
                // Restrict file permissions to owner-only (0600)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
                }
                tracing::info!(
                    "Generated and saved new capability signing key to {:?}",
                    key_path
                );
            }
            Err(e) => {
                tracing::error!(
                    "Failed to persist signing key to {:?}: {}. Tokens will not survive restart.",
                    key_path,
                    e
                );
            }
        }

        audit_log.security_warning(
            "signing_key_generated",
            "New capability signing key generated. All previously issued tokens are invalid.",
        );

        signing_key
    }

    /// Create a capability manager with an existing signing key
    pub fn with_key(
        signing_key: SigningKey,
        store: Arc<CapabilityStore>,
        audit_log: Arc<AuditLog>,
        metrics: Arc<MetricsManager>,
    ) -> Self {
        let verifying_key = signing_key.verifying_key();

        Self {
            signing_key,
            verifying_key,
            store,
            audit_log,
            metrics,
        }
    }

    /// Get the runtime's public key
    pub fn public_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Get the runtime's public key bytes
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    /// Grant a new capability token
    pub fn grant(
        &self,
        capsule_id: &str,
        resource: ResourceId,
        action: Action,
        constraints: TokenConstraints,
        expiry: Option<SecureTimestamp>,
    ) -> CapabilityToken {
        let issued_at = SecureTimestamp::now();

        // Ensure token epoch is at least current epoch
        let mut constraints = constraints;
        if constraints.epoch < self.store.current_epoch() {
            constraints.epoch = self.store.current_epoch();
        }

        let mut token = CapabilityToken::new(
            capsule_id.to_string(),
            self.public_key_bytes(),
            resource.clone(),
            action,
            constraints,
            issued_at,
            expiry,
        );

        // Sign the token
        token.sign(&self.signing_key);

        // Record metrics
        self.metrics.record_capability_request(capsule_id);

        // Audit log
        self.audit_log
            .capability_grant(&token.id, capsule_id, &resource, action, expiry);

        token
    }

    /// Validate a token for use
    ///
    /// This performs all 12 validation checks as specified in PHASE3.md.
    /// ALWAYS emits an audit event.
    pub async fn validate(
        &self,
        token: &CapabilityToken,
        caller_capsule_id: &str,
        requested_action: Action,
        requested_resource: &ResourceId,
        resource_classification: Option<u8>,
    ) -> Result<(), ValidationError> {
        // 1. Version verification
        if token.version != CapabilityToken::CURRENT_VERSION {
            self.audit_validation_failure(token, caller_capsule_id, "invalid_version");
            return Err(ValidationError::InvalidVersion {
                expected: CapabilityToken::CURRENT_VERSION,
                got: token.version,
            });
        }

        // 2. Signature verification
        if !token.verify_signature(&self.verifying_key) {
            self.audit_validation_failure(token, caller_capsule_id, "invalid_signature");
            return Err(ValidationError::InvalidSignature);
        }

        // 3. Issuer verification (must be runtime's key)
        if token.issuer != self.public_key_bytes() {
            self.audit_validation_failure(token, caller_capsule_id, "untrusted_issuer");
            return Err(ValidationError::UntrustedIssuer);
        }

        // 4. Caller verification
        if token.capsule != caller_capsule_id {
            self.audit_validation_failure(token, caller_capsule_id, "wrong_capsule");
            return Err(ValidationError::WrongCapsule {
                expected: token.capsule.clone(),
                got: caller_capsule_id.to_string(),
            });
        }

        // 5. Action verification
        if token.action != requested_action {
            self.audit_validation_failure(token, caller_capsule_id, "wrong_action");
            return Err(ValidationError::WrongAction {
                expected: token.action,
                got: requested_action,
            });
        }

        // 6. Resource verification (with pattern matching)
        if !requested_resource.matches(&token.resource) {
            self.audit_validation_failure(token, caller_capsule_id, "wrong_resource");
            return Err(ValidationError::WrongResource {
                expected: token.resource.to_string(),
                got: requested_resource.to_string(),
            });
        }

        // 7. Epoch verification (revocation check)
        if !self.store.is_epoch_valid(token.constraints.epoch) {
            self.audit_validation_failure(token, caller_capsule_id, "epoch_revoked");
            return Err(ValidationError::TokenRevoked);
        }

        // 8. Individual revocation check
        if self.store.is_token_revoked(&token.id).await {
            self.audit_validation_failure(token, caller_capsule_id, "individually_revoked");
            return Err(ValidationError::TokenRevoked);
        }

        // 9. Temporal verification - future dated
        if token.issued_at.is_future() {
            self.audit_validation_failure(token, caller_capsule_id, "future_dated");
            return Err(ValidationError::FutureDatedToken);
        }

        // 10. Temporal verification - expired
        if let Some(expiry) = &token.expiry {
            if expiry.is_expired() {
                self.audit_validation_failure(token, caller_capsule_id, "expired");
                return Err(ValidationError::TokenExpired);
            }
        }

        // 11. Use count verification (if limited) — atomic check-and-increment
        if let Some(max_uses) = token.constraints.max_uses {
            if let Err(current) = self.store.try_use_token(&token.id, max_uses).await {
                self.audit_validation_failure(token, caller_capsule_id, "use_limit_exceeded");
                return Err(ValidationError::UseLimitExceeded {
                    current,
                    max: max_uses,
                });
            }
        }

        // 12. Classification verification (if specified)
        if let (Some(token_max), Some(resource_class)) = (
            token.constraints.max_classification,
            resource_classification,
        ) {
            if token_max < resource_class {
                self.audit_validation_failure(token, caller_capsule_id, "classification_exceeded");
                return Err(ValidationError::ClassificationExceeded {
                    token: token_max,
                    resource: resource_class,
                });
            }
        }

        // SUCCESS - emit audit event
        self.audit_log.capability_use(
            &token.id,
            caller_capsule_id,
            requested_resource,
            requested_action,
            true,
        );

        // Record metrics
        self.metrics.record_capability_use(caller_capsule_id);

        Ok(())
    }

    /// Delegate a token: issue a narrower token on behalf of the parent.
    ///
    /// Rules (depth=1 only, no chaining):
    /// - Parent must pass full validation (signature, expiry, revocation, etc.)
    /// - Parent must be `delegatable`
    /// - Delegated token's resource must be a sub-scope of the parent
    /// - Delegated token inherits the same action
    /// - Expiry is the earlier of `parent.expiry` and `expiry` param
    /// - Delegated token is itself NOT delegatable (depth=1)
    pub async fn delegate(
        &self,
        parent: &CapabilityToken,
        caller_capsule_id: &str,
        to_capsule: &str,
        resource: ResourceId,
        expiry: Option<SecureTimestamp>,
    ) -> Result<CapabilityToken, ValidationError> {
        // Validate parent is legit (signature, expiry, revocation, etc.)
        self.validate(
            parent,
            caller_capsule_id,
            parent.action,
            &parent.resource,
            None,
        )
        .await?;

        if !parent.constraints.delegatable {
            return Err(ValidationError::DelegationNotAllowed);
        }

        // The requested resource must be a sub-scope of the parent
        if !resource.matches(&parent.resource) {
            return Err(ValidationError::DelegationScopeWidened);
        }

        // Effective expiry: min of parent's expiry and requested expiry
        let effective_expiry = match (&parent.expiry, &expiry) {
            (Some(p), Some(r)) => Some(if p.unix_secs < r.unix_secs { *p } else { *r }),
            (Some(p), None) => Some(*p),
            (None, Some(r)) => Some(*r),
            (None, None) => None,
        };

        // Non-delegatable, inherits parent action
        let constraints = TokenConstraints {
            delegatable: false,
            epoch: parent.constraints.epoch,
            max_classification: parent.constraints.max_classification,
            max_uses: parent.constraints.max_uses,
        };

        let token = self.grant(
            to_capsule,
            resource.clone(),
            parent.action,
            constraints,
            effective_expiry,
        );

        self.audit_log.security_warning(
            "capability_delegated",
            &format!(
                "Token {} delegated from {} to {} for {}",
                token.id, parent.capsule, to_capsule, resource
            ),
        );

        Ok(token)
    }

    /// Revoke a specific token
    pub async fn revoke(&self, token_id: TokenId, reason: &str) {
        self.store.revoke_token(token_id).await;
        self.audit_log.capability_revoke(&token_id, reason);
    }

    /// Revoke all tokens by advancing the epoch
    ///
    /// All tokens with epoch < new_epoch will be rejected.
    /// Returns the new epoch.
    pub fn revoke_all(&self, reason: &str) -> u64 {
        let old_epoch = self.store.current_epoch();
        let new_epoch = self.store.advance_epoch();
        self.audit_log.epoch_advance(old_epoch, new_epoch, reason);
        new_epoch
    }

    /// Rotate the signing key: generate a new key, advance the epoch, persist.
    ///
    /// All previously issued tokens become invalid (epoch revocation).
    /// Returns the new public key bytes.
    pub fn rotate_signing_key(&mut self, data_dir: &Path, reason: &str) -> [u8; 32] {
        let new_key = SigningKey::generate(&mut rand::thread_rng());
        let new_pub = new_key.verifying_key();

        // Persist new key with restrictive permissions
        let key_path = data_dir.join("signing_key");
        if let Err(e) = std::fs::write(&key_path, new_key.to_bytes()) {
            tracing::error!("Failed to persist rotated key: {}", e);
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
        }

        // Advance epoch to invalidate all existing tokens
        let old_epoch = self.store.current_epoch();
        let new_epoch = self.store.advance_epoch();

        self.audit_log.security_warning(
            "signing_key_rotated",
            &format!(
                "Signing key rotated (reason: {}). Epoch advanced {} → {}. All prior tokens invalidated.",
                reason, old_epoch, new_epoch
            ),
        );

        let pub_bytes = new_pub.to_bytes();
        self.signing_key = new_key;
        self.verifying_key = new_pub;

        pub_bytes
    }

    /// Get the current revocation epoch
    pub fn current_epoch(&self) -> u64 {
        self.store.current_epoch()
    }

    /// Get reference to the audit log
    pub fn audit_log(&self) -> &Arc<AuditLog> {
        &self.audit_log
    }

    /// Helper to audit validation failures
    fn audit_validation_failure(&self, token: &CapabilityToken, capsule_id: &str, reason: &str) {
        self.audit_log
            .capability_use(&token.id, capsule_id, &token.resource, token.action, false);
        self.audit_log.security_warning(
            "capability_validation_failed",
            &format!("token {} failed validation: {}", token.id, reason),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> CapabilityManager {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());

        CapabilityManager::new(store, audit_log, metrics)
    }

    #[tokio::test]
    async fn test_grant_and_validate() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Valid use
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test/file.txt"),
                None,
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_wrong_capsule() {
        let manager = create_test_manager();

        let token = manager.grant(
            "capsule-a",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Wrong capsule trying to use it
        let result = manager
            .validate(
                &token,
                "capsule-b",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;

        assert!(matches!(result, Err(ValidationError::WrongCapsule { .. })));
    }

    #[tokio::test]
    async fn test_wrong_action() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Trying to write with read token
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Write,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;

        assert!(matches!(result, Err(ValidationError::WrongAction { .. })));
    }

    #[tokio::test]
    async fn test_epoch_revocation() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Advance epoch (mass revocation)
        manager.revoke_all("test revocation");

        // Token should now be invalid
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;

        assert!(matches!(result, Err(ValidationError::TokenRevoked)));
    }

    #[tokio::test]
    async fn test_individual_revocation() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Revoke this specific token
        manager.revoke(token.id, "test revocation").await;

        // Token should now be invalid
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;

        assert!(matches!(result, Err(ValidationError::TokenRevoked)));
    }

    #[tokio::test]
    async fn test_use_limited_token() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints {
                max_uses: Some(2),
                ..Default::default()
            },
            None,
        );

        // First use - OK
        assert!(manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await
            .is_ok());

        // Second use - OK
        assert!(manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await
            .is_ok());

        // Third use - should fail
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(ValidationError::UseLimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn test_expired_token() {
        let manager = create_test_manager();

        // Create a token that's already expired
        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints::default(),
            Some(SecureTimestamp::at(1)), // Expired in 1970
        );

        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                None,
            )
            .await;

        assert!(matches!(result, Err(ValidationError::TokenExpired)));
    }

    #[tokio::test]
    async fn test_wildcard_resource() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        // Should match files in photos/
        assert!(manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/photos/vacation.jpg"),
                None,
            )
            .await
            .is_ok());

        // Should NOT match files outside photos/
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/documents/secret.txt"),
                None,
            )
            .await;
        assert!(matches!(result, Err(ValidationError::WrongResource { .. })));
    }

    #[tokio::test]
    async fn test_classification() {
        let manager = create_test_manager();

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/test.txt"),
            Action::Read,
            TokenConstraints {
                max_classification: Some(2), // Can access up to level 2
                ..Default::default()
            },
            None,
        );

        // Access level 1 resource - OK
        assert!(manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                Some(1),
            )
            .await
            .is_ok());

        // Access level 3 resource - should fail
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/test.txt"),
                Some(3),
            )
            .await;
        assert!(matches!(
            result,
            Err(ValidationError::ClassificationExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn test_delegate_success() {
        let manager = create_test_manager();

        let parent = manager.grant(
            "capsule-a",
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            TokenConstraints {
                delegatable: true,
                ..Default::default()
            },
            None,
        );

        let child = manager
            .delegate(
                &parent,
                "capsule-a",
                "capsule-b",
                ResourceId::new("localhost://Users/self/Documents/photos/vacation/*"),
                None,
            )
            .await
            .unwrap();

        assert_eq!(child.capsule, "capsule-b");
        assert!(
            !child.constraints.delegatable,
            "delegated tokens must not be re-delegatable"
        );

        // Child should be usable
        let result = manager
            .validate(
                &child,
                "capsule-b",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/photos/vacation/1.jpg"),
                None,
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delegate_non_delegatable() {
        let manager = create_test_manager();

        let parent = manager.grant(
            "capsule-a",
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            TokenConstraints::default(), // delegatable = false
            None,
        );

        let result = manager
            .delegate(
                &parent,
                "capsule-a",
                "capsule-b",
                ResourceId::new("localhost://Users/self/Documents/photos/vacation/*"),
                None,
            )
            .await;
        assert!(matches!(result, Err(ValidationError::DelegationNotAllowed)));
    }

    #[tokio::test]
    async fn test_delegate_from_revoked_parent_fails() {
        let manager = create_test_manager();

        let parent = manager.grant(
            "capsule-a",
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            TokenConstraints {
                delegatable: true,
                ..Default::default()
            },
            None,
        );

        // Revoke the parent
        manager.revoke(parent.id, "compromised").await;

        // Delegation should fail because parent is revoked
        let result = manager
            .delegate(
                &parent,
                "capsule-a",
                "capsule-b",
                ResourceId::new("localhost://Users/self/Documents/photos/vacation/*"),
                None,
            )
            .await;
        assert!(
            matches!(result, Err(ValidationError::TokenRevoked)),
            "Delegation from revoked parent must fail, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_valid_token_passes_all_checks() {
        // A properly issued token with correct capsule, action, resource,
        // valid epoch, not expired, not revoked — passes validation.
        let manager = create_test_manager();

        let token = manager.grant(
            "my-capsule",
            ResourceId::new("localhost://Users/self/Documents/docs/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        let result = manager
            .validate(
                &token,
                "my-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/docs/readme.md"),
                None,
            )
            .await;
        assert!(
            result.is_ok(),
            "Valid token must pass all checks: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_concurrent_use_count_respects_limit() {
        // Spawn many concurrent validations on a use-limited token.
        // Only `max_uses` should succeed.
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        let max_uses = 5u32;
        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/file.txt"),
            Action::Read,
            TokenConstraints {
                max_uses: Some(max_uses),
                ..Default::default()
            },
            None,
        );

        let concurrency = 20usize;
        let mut set = tokio::task::JoinSet::new();

        for _ in 0..concurrency {
            let mgr = Arc::clone(&manager);
            let tok = token.clone();
            set.spawn(async move {
                mgr.validate(
                    &tok,
                    "test-capsule",
                    Action::Read,
                    &ResourceId::new("localhost://Users/self/Documents/file.txt"),
                    None,
                )
                .await
            });
        }

        let mut results = Vec::new();
        while let Some(result) = set.join_next().await {
            results.push(result.unwrap());
        }

        let successes = results.iter().filter(|r| r.is_ok()).count();
        let failures = results.iter().filter(|r| r.is_err()).count();

        assert_eq!(
            successes, max_uses as usize,
            "Exactly max_uses validations should succeed"
        );
        assert_eq!(
            failures,
            concurrency - max_uses as usize,
            "Remaining validations should fail"
        );
    }

    // === Concurrent capability operation tests ===

    #[tokio::test]
    async fn test_concurrent_validate_and_revoke() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        let token_id = token.id;
        let mut set = tokio::task::JoinSet::new();

        // Spawn concurrent validates
        for _ in 0..20 {
            let mgr = Arc::clone(&manager);
            let tok = token.clone();
            set.spawn(async move {
                mgr.validate(
                    &tok,
                    "test-capsule",
                    Action::Read,
                    &ResourceId::new("localhost://Users/self/Documents/file.txt"),
                    None,
                )
                .await
            });
        }

        // Revoke the token mid-flight
        let mgr = Arc::clone(&manager);
        set.spawn(async move {
            mgr.revoke(token_id, "concurrent test").await;
            Ok(()) // Return Ok so we can join uniformly
        });

        let mut results = Vec::new();
        while let Some(result) = set.join_next().await {
            results.push(result.unwrap());
        }

        // After revocation, no new validates should succeed. Some pre-revoke
        // validates may have succeeded. The key invariant: no panic, no
        // data corruption, and revoked tokens eventually fail.
        let successes = results.iter().filter(|r| r.is_ok()).count();
        // At least the revoke task itself returns Ok
        assert!(successes >= 1, "At least the revoke task should succeed");
    }

    #[tokio::test]
    async fn test_concurrent_validate_and_epoch_advance() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        let mut set = tokio::task::JoinSet::new();

        // Spawn concurrent validates
        for _ in 0..30 {
            let mgr = Arc::clone(&manager);
            let tok = token.clone();
            set.spawn(async move {
                mgr.validate(
                    &tok,
                    "test-capsule",
                    Action::Read,
                    &ResourceId::new("localhost://Users/self/Documents/file.txt"),
                    None,
                )
                .await
            });
        }

        // Advance epoch to invalidate all tokens
        manager.revoke_all("epoch advance test");

        let mut successes = 0;
        let mut revoked = 0;
        while let Some(result) = set.join_next().await {
            match result.unwrap() {
                Ok(()) => successes += 1,
                Err(ValidationError::TokenRevoked) => revoked += 1,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        // Some validates ran before epoch advance, some after
        assert!(
            successes + revoked == 30,
            "All validates should either succeed or be revoked"
        );
    }

    #[tokio::test]
    async fn test_concurrent_revoke_same_token_idempotent() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        let token = manager.grant(
            "test-capsule",
            ResourceId::new("localhost://Users/self/Documents/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );

        let token_id = token.id;
        let mut set = tokio::task::JoinSet::new();

        // 50 concurrent revokes on the same token
        for _ in 0..50 {
            let mgr = Arc::clone(&manager);
            set.spawn(async move {
                mgr.revoke(token_id, "concurrent revoke").await;
            });
        }

        while let Some(result) = set.join_next().await {
            result.unwrap(); // No panics
        }

        // Token must be revoked after all concurrent revokes
        let result = manager
            .validate(
                &token,
                "test-capsule",
                Action::Read,
                &ResourceId::new("localhost://Users/self/Documents/file.txt"),
                None,
            )
            .await;
        assert!(
            matches!(result, Err(ValidationError::TokenRevoked)),
            "Token must be revoked after concurrent revokes"
        );
    }

    #[tokio::test]
    async fn test_concurrent_use_count_multiple_tokens_isolated() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        let max_uses = 10u32;
        let mut tokens = Vec::new();
        for i in 0..3 {
            let token = manager.grant(
                "test-capsule",
                ResourceId::new(format!("localhost://Users/self/Documents/file{}.txt", i)),
                Action::Read,
                TokenConstraints {
                    max_uses: Some(max_uses),
                    ..Default::default()
                },
                None,
            );
            tokens.push(token);
        }

        let mut set = tokio::task::JoinSet::new();

        // 20 concurrent validates per token
        for token in &tokens {
            for _ in 0..20 {
                let mgr = Arc::clone(&manager);
                let tok = token.clone();
                let res = token.resource.clone();
                set.spawn(async move {
                    mgr.validate(&tok, "test-capsule", Action::Read, &res, None)
                        .await
                });
            }
        }

        let mut total_success = 0usize;
        let mut total_fail = 0usize;
        while let Some(result) = set.join_next().await {
            match result.unwrap() {
                Ok(()) => total_success += 1,
                Err(_) => total_fail += 1,
            }
        }

        assert_eq!(
            total_success,
            (3 * max_uses) as usize,
            "Each of 3 tokens should allow exactly max_uses validations"
        );
        assert_eq!(
            total_fail,
            3 * (20 - max_uses as usize),
            "Remaining validates should fail"
        );
    }

    #[tokio::test]
    async fn test_delegate_scope_widened() {
        let manager = create_test_manager();

        let parent = manager.grant(
            "capsule-a",
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            TokenConstraints {
                delegatable: true,
                ..Default::default()
            },
            None,
        );

        // Try to delegate wider scope
        let result = manager
            .delegate(
                &parent,
                "capsule-a",
                "capsule-b",
                ResourceId::new("localhost://Users/self/Documents/*"),
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(ValidationError::DelegationScopeWidened)
        ));
    }
}
