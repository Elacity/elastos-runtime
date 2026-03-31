//! HTTP API Integration Tests
//!
//! Tests the full capability request/grant/deny flow:
//! - Session creation and authentication
//! - Capability requests from capsule sessions
//! - Grant/deny from shell sessions
//! - Permission polling
//!
//! These tests use the session registry and capability system directly,
//! simulating the HTTP API flow without needing a full runtime.

use std::sync::Arc;

use elastos_runtime::capability::evaluator::ShellPassthroughVerifier;
use elastos_runtime::capability::{
    Action, AutoGrantVerifier, CapabilityManager, CapabilityStore, GrantDuration,
    PendingRequestStore, PolicyEvaluator, PolicyOutcome, ResourceId, RulesVerifier,
    TokenConstraints,
};
use elastos_runtime::content::{ContentResolver, NullFetcher, ResolverConfig};
use elastos_runtime::namespace::NamespaceStore;
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::metrics::MetricsManager;
use elastos_runtime::session::{SessionRegistry, SessionType};
use tempfile::tempdir;

/// Create test infrastructure with session registry and capability manager
fn create_test_infra() -> (
    Arc<SessionRegistry>,
    Arc<CapabilityManager>,
    Arc<PendingRequestStore>,
) {
    let audit_log = Arc::new(AuditLog::new());
    let metrics = Arc::new(MetricsManager::new());
    let store = Arc::new(CapabilityStore::new());

    let session_registry = Arc::new(SessionRegistry::new(audit_log.clone()));
    let capability_manager = Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
    let pending_store = Arc::new(PendingRequestStore::new(audit_log));

    (session_registry, capability_manager, pending_store)
}

// ==================== Session Registry Tests ====================

#[tokio::test]
async fn test_create_shell_session() {
    let (session_registry, _, _) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Shell, None)
        .await;

    assert!(session.is_shell());
    assert!(!session.token.is_empty());
    assert!(session.vm_id.is_none());
}

#[tokio::test]
async fn test_create_capsule_session_with_vm() {
    let (session_registry, _, _) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, Some("vm-123".to_string()))
        .await;

    assert!(!session.is_shell());
    assert_eq!(session.vm_id, Some("vm-123".to_string()));
}

#[tokio::test]
async fn test_validate_session_token() {
    let (session_registry, _, _) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Shell, None)
        .await;
    let token = session.token.clone();

    // Valid token should return session
    let validated = session_registry.validate_token(&token).await;
    assert!(validated.is_some());
    assert_eq!(validated.unwrap().id, session.id);

    // Invalid token should return None
    let invalid = session_registry.validate_token("invalid-token").await;
    assert!(invalid.is_none());
}

#[tokio::test]
async fn test_invalidate_session() {
    let (session_registry, _, _) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Shell, None)
        .await;
    let token = session.token.clone();

    // Token should be valid
    assert!(session_registry.validate_token(&token).await.is_some());

    // Invalidate
    session_registry.invalidate_session(&token).await;

    // Token should now be invalid
    assert!(session_registry.validate_token(&token).await.is_none());
}

// ==================== Capability Request Flow Tests ====================

#[tokio::test]
async fn test_create_capability_request() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    assert!(request.is_pending());
    assert_eq!(request.session_id, session.id);
    assert_eq!(
        request.resource.to_string(),
        "localhost://Users/self/Documents/photos/*"
    );
    assert_eq!(request.action, Action::Read);
}

#[tokio::test]
async fn test_list_pending_requests() {
    let (session_registry, _, pending_store) = create_test_infra();

    // Create two sessions with requests
    let session1 = session_registry
        .create_session(SessionType::Capsule, None)
        .await;
    let session2 = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    pending_store
        .create_request(
            session1.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    pending_store
        .create_request(
            session2.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/documents/*"),
            Action::Write,
        )
        .await;

    let pending = pending_store.list_pending().await;
    assert_eq!(pending.len(), 2);
}

#[tokio::test]
async fn test_grant_capability_request() {
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    // Create capability token
    let token = capability_manager.grant(
        &session.id.to_string(),
        ResourceId::new("localhost://Users/self/Documents/photos/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    // Grant the request
    let result = pending_store
        .grant_request(
            &request.id.to_string(),
            token.clone(),
            GrantDuration::Session,
        )
        .await;

    assert!(result.is_ok());

    // Check request status
    let updated = pending_store.get_request(&request.id.to_string()).await;
    assert!(updated.is_some());

    let updated = updated.unwrap();
    match &updated.status {
        elastos_runtime::capability::RequestStatus::Granted { token: t, duration } => {
            assert_eq!(t.to_base64().unwrap(), token.to_base64().unwrap());
            assert_eq!(*duration, GrantDuration::Session);
        }
        _ => panic!("Expected Granted status"),
    }
}

#[tokio::test]
async fn test_deny_capability_request() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/sensitive/*"),
            Action::Delete,
        )
        .await;

    // Deny the request
    let result = pending_store
        .deny_request(&request.id.to_string(), "Access denied by policy")
        .await;

    assert!(result.is_ok());

    // Check request status
    let updated = pending_store.get_request(&request.id.to_string()).await;
    assert!(updated.is_some());

    let updated = updated.unwrap();
    match &updated.status {
        elastos_runtime::capability::RequestStatus::Denied { reason } => {
            assert_eq!(reason, "Access denied by policy");
        }
        _ => panic!("Expected Denied status"),
    }
}

#[tokio::test]
async fn test_grant_nonexistent_request_fails() {
    let (_, capability_manager, pending_store) = create_test_infra();

    let token = capability_manager.grant(
        "fake-session",
        ResourceId::new("localhost://Users/self/Documents/test/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    let result = pending_store
        .grant_request("nonexistent-request-id", token, GrantDuration::Session)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_deny_nonexistent_request_fails() {
    let (_, _, pending_store) = create_test_infra();

    let result = pending_store
        .deny_request("nonexistent-request-id", "Denied")
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_cannot_grant_already_granted_request() {
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    let token = capability_manager.grant(
        &session.id.to_string(),
        ResourceId::new("localhost://Users/self/Documents/photos/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    // First grant should succeed
    let result1 = pending_store
        .grant_request(
            &request.id.to_string(),
            token.clone(),
            GrantDuration::Session,
        )
        .await;
    assert!(result1.is_ok());

    // Second grant should fail
    let result2 = pending_store
        .grant_request(&request.id.to_string(), token, GrantDuration::Session)
        .await;
    assert!(result2.is_err());
}

#[tokio::test]
async fn test_cannot_deny_already_denied_request() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    // First deny should succeed
    let result1 = pending_store
        .deny_request(&request.id.to_string(), "Denied")
        .await;
    assert!(result1.is_ok());

    // Second deny should fail
    let result2 = pending_store
        .deny_request(&request.id.to_string(), "Denied again")
        .await;
    assert!(result2.is_err());
}

// ==================== Grant Duration Tests ====================

#[tokio::test]
async fn test_grant_once_duration() {
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    let token = capability_manager.grant(
        &session.id.to_string(),
        ResourceId::new("localhost://Users/self/Documents/photos/*"),
        Action::Read,
        TokenConstraints::new(0, false, None, Some(1)),
        None,
    );

    pending_store
        .grant_request(&request.id.to_string(), token, GrantDuration::Once)
        .await
        .unwrap();

    let updated = pending_store
        .get_request(&request.id.to_string())
        .await
        .unwrap();
    match &updated.status {
        elastos_runtime::capability::RequestStatus::Granted { duration, .. } => {
            assert_eq!(*duration, GrantDuration::Once);
        }
        _ => panic!("Expected Granted status"),
    }
}

// ==================== Request Expiry Tests ====================

#[tokio::test]
async fn test_request_expiry_detection() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    // Request should not be expired initially (timeout is usually 5 minutes)
    assert!(!request.is_expired());
}

// ==================== Action Parsing Tests ====================

#[tokio::test]
async fn test_all_action_types() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let actions = vec![
        Action::Read,
        Action::Write,
        Action::Execute,
        Action::Delete,
        Action::Message,
        Action::Admin,
    ];

    for action in actions {
        let request = pending_store
            .create_request(
                session.id.clone(),
                ResourceId::new("localhost://Users/self/Documents/test/*"),
                action,
            )
            .await;

        assert_eq!(request.action, action);
    }
}

// ==================== Session Type Authorization Tests ====================

#[tokio::test]
async fn test_shell_session_is_shell() {
    let (session_registry, _, _) = create_test_infra();

    let shell = session_registry
        .create_session(SessionType::Shell, None)
        .await;
    let capsule = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    assert!(shell.is_shell());
    assert!(!capsule.is_shell());
}

// ==================== VM Session Tracking Tests ====================

#[tokio::test]
async fn test_vm_session_tracking() {
    let (session_registry, _, _) = create_test_infra();

    // Create session with VM ID
    let session = session_registry
        .create_session(SessionType::Capsule, Some("vm-abc".to_string()))
        .await;

    // Should be able to find by VM ID
    assert!(session_registry.has_vm_session("vm-abc").await);
    assert!(!session_registry.has_vm_session("vm-xyz").await);

    // Get token by VM ID
    let token = session_registry.get_vm_token("vm-abc").await;
    assert!(token.is_some());
    assert_eq!(token.unwrap(), session.token);
}

#[tokio::test]
async fn test_cleanup_dead_vm_sessions() {
    let (session_registry, _, _) = create_test_infra();

    // Create sessions for VMs
    session_registry
        .create_session(SessionType::Capsule, Some("vm-1".to_string()))
        .await;
    session_registry
        .create_session(SessionType::Capsule, Some("vm-2".to_string()))
        .await;
    session_registry
        .create_session(SessionType::Capsule, Some("vm-3".to_string()))
        .await;

    assert_eq!(session_registry.session_count().await, 3);

    // Cleanup with only vm-2 alive
    let cleaned = session_registry
        .cleanup_dead_vm_sessions(&["vm-2".to_string()])
        .await;

    assert_eq!(cleaned, 2);
    assert_eq!(session_registry.session_count().await, 1);
    assert!(session_registry.has_vm_session("vm-2").await);
    assert!(!session_registry.has_vm_session("vm-1").await);
    assert!(!session_registry.has_vm_session("vm-3").await);
}

// ==================== Multiple Concurrent Requests Tests ====================

#[tokio::test]
async fn test_multiple_requests_same_session() {
    let (session_registry, _, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    // Create multiple requests from same session
    let request1 = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    let request2 = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/documents/*"),
            Action::Write,
        )
        .await;

    // Both should be pending
    let pending = pending_store.list_pending().await;
    assert_eq!(pending.len(), 2);

    // Request IDs should be different
    assert_ne!(request1.id, request2.id);
}

// ==================== Full Flow Integration Test ====================

#[tokio::test]
async fn test_full_capability_flow() {
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    // 1. Shell session starts
    let shell = session_registry
        .create_session(SessionType::Shell, None)
        .await;

    // 2. Capsule session starts (would be a MicroVM)
    let capsule = session_registry
        .create_session(SessionType::Capsule, Some("vm-app-1".to_string()))
        .await;

    // 3. Capsule requests capability
    let request = pending_store
        .create_request(
            capsule.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    // 4. Shell polls for pending requests
    let pending = pending_store.list_pending().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].resource.to_string(),
        "localhost://Users/self/Documents/photos/*"
    );

    // 5. Shell grants the request
    let token = capability_manager.grant(
        &capsule.id.to_string(),
        ResourceId::new("localhost://Users/self/Documents/photos/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    pending_store
        .grant_request(
            &request.id.to_string(),
            token.clone(),
            GrantDuration::Session,
        )
        .await
        .unwrap();

    // 6. Capsule polls for request status and gets token
    let updated = pending_store
        .get_request(&request.id.to_string())
        .await
        .unwrap();
    assert!(matches!(
        &updated.status,
        elastos_runtime::capability::RequestStatus::Granted { .. }
    ));

    // 7. Pending list should now be empty
    let pending = pending_store.list_pending().await;
    assert_eq!(pending.len(), 0);

    // 8. Verify token is valid for the capsule
    let validated = capability_manager
        .validate(
            &token,
            &capsule.id.to_string(),
            Action::Read,
            &ResourceId::new("localhost://Users/self/Documents/photos/*"),
            None,
        )
        .await;
    assert!(validated.is_ok());

    // Cleanup: verify sessions can be invalidated
    session_registry.invalidate_session(&shell.token).await;
    session_registry.invalidate_session(&capsule.token).await;
    assert_eq!(session_registry.session_count().await, 0);
}

// ==================== Namespace API Tests ====================

/// Create test infrastructure with namespace store
fn create_namespace_test_infra() -> (Arc<NamespaceStore>, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let audit_log = Arc::new(AuditLog::new());
    let content_resolver = Arc::new(ContentResolver::new(
        ResolverConfig::default(),
        audit_log.clone(),
        Arc::new(NullFetcher),
    ));

    let store = Arc::new(NamespaceStore::new(
        dir.path().to_path_buf(),
        content_resolver,
        audit_log,
    ));

    (store, dir)
}

/// Test owner key for namespace tests (64 hex chars = 32 bytes)
const TEST_OWNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn test_namespace_list_empty() {
    let (store, _dir) = create_namespace_test_infra();

    // Create empty namespace
    store.load_or_create(TEST_OWNER).await.unwrap();

    // List root - should be empty
    let entries = store.list_path(TEST_OWNER, "").await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_namespace_write_and_list() {
    let (store, _dir) = create_namespace_test_infra();

    // Write files
    store
        .write_path(
            TEST_OWNER,
            "photos/a.jpg",
            b"image data a",
            Some("image/jpeg".into()),
        )
        .await
        .unwrap();
    store
        .write_path(TEST_OWNER, "photos/b.jpg", b"image data b", None)
        .await
        .unwrap();
    store
        .write_path(
            TEST_OWNER,
            "docs/readme.txt",
            b"hello",
            Some("text/plain".into()),
        )
        .await
        .unwrap();

    // List photos directory
    let entries = store.list_path(TEST_OWNER, "photos").await.unwrap();
    assert_eq!(entries.len(), 2);

    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.jpg"));
    assert!(names.contains(&"b.jpg"));

    // List root - should have photos and docs
    let root_entries = store.list_path(TEST_OWNER, "").await.unwrap();
    assert_eq!(root_entries.len(), 2);
}

#[tokio::test]
async fn test_namespace_read_content() {
    let (store, _dir) = create_namespace_test_infra();

    let content = b"hello world from elastos";
    store
        .write_path(TEST_OWNER, "test.txt", content, Some("text/plain".into()))
        .await
        .unwrap();

    // Read it back
    let result = store.read_path(TEST_OWNER, "test.txt").await.unwrap();
    assert_eq!(result.content, content);
    assert_eq!(result.content_type, Some("text/plain".into()));
    assert_eq!(result.size, content.len() as u64);
    assert!(result.cid.to_string().contains("sha256"));
}

#[tokio::test]
async fn test_namespace_delete_path() {
    let (store, _dir) = create_namespace_test_infra();

    store
        .write_path(TEST_OWNER, "deleteme.txt", b"temp", None)
        .await
        .unwrap();

    // Verify it exists
    let result = store.read_path(TEST_OWNER, "deleteme.txt").await;
    assert!(result.is_ok());

    // Delete it
    store.delete_path(TEST_OWNER, "deleteme.txt").await.unwrap();

    // Verify it's gone
    let result = store.read_path(TEST_OWNER, "deleteme.txt").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_namespace_status() {
    let (store, _dir) = create_namespace_test_infra();

    store
        .write_path(TEST_OWNER, "file1.txt", b"hello", None)
        .await
        .unwrap();
    store
        .write_path(TEST_OWNER, "dir/file2.txt", b"world!", None)
        .await
        .unwrap();

    let status = store.namespace_status(TEST_OWNER).await.unwrap();

    assert_eq!(status.owner, TEST_OWNER);
    assert_eq!(status.entry_count, 2); // Two files
    assert_eq!(status.total_size, 11); // "hello" + "world!"
    assert_eq!(status.cached_size, 11); // All cached locally
    assert!(!status.signed); // Not signed
    assert!(status.namespace_cid.to_string().contains("sha256"));
}

#[tokio::test]
async fn test_namespace_cache_stats() {
    let (store, _dir) = create_namespace_test_infra();

    store
        .write_path(TEST_OWNER, "a.txt", b"aaaa", None)
        .await
        .unwrap();
    store
        .write_path(TEST_OWNER, "b.txt", b"bbbbbb", None)
        .await
        .unwrap();

    let stats = store.cache_stats().await;

    assert_eq!(stats.entry_count, 2);
    assert_eq!(stats.total_bytes, 10); // 4 + 6
    assert!(stats.limit_bytes > 0);
}

#[tokio::test]
async fn test_namespace_is_cached() {
    let (store, _dir) = create_namespace_test_infra();

    // Write content
    let result = store
        .write_path(TEST_OWNER, "cached.txt", b"content", None)
        .await
        .unwrap();

    // Should be cached
    assert!(store.is_cached(&result.cid).await);

    // Clear cache
    store.clear_cache().await;

    // Should no longer be cached
    assert!(!store.is_cached(&result.cid).await);
}

#[tokio::test]
async fn test_namespace_entry_info() {
    let (store, _dir) = create_namespace_test_infra();

    store
        .write_path(
            TEST_OWNER,
            "photo.jpg",
            b"jpeg data",
            Some("image/jpeg".into()),
        )
        .await
        .unwrap();
    store
        .write_path(TEST_OWNER, "subdir/nested.txt", b"nested", None)
        .await
        .unwrap();

    let entries = store.list_path(TEST_OWNER, "").await.unwrap();

    // Find the photo entry
    let photo = entries.iter().find(|e| e.name == "photo.jpg").unwrap();
    assert_eq!(photo.entry_type, "file");
    assert_eq!(photo.content_type, Some("image/jpeg".into()));
    assert!(photo.cid.is_some());
    assert!(photo.cached);

    // Find the subdir entry
    let subdir = entries.iter().find(|e| e.name == "subdir").unwrap();
    assert_eq!(subdir.entry_type, "directory");
    assert!(subdir.cid.is_none());
}

#[tokio::test]
async fn test_namespace_multiple_users() {
    let (store, _dir) = create_namespace_test_infra();

    const USER1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const USER2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    // Write to user1's namespace
    store
        .write_path(USER1, "user1.txt", b"user1 data", None)
        .await
        .unwrap();

    // Write to user2's namespace
    store
        .write_path(USER2, "user2.txt", b"user2 data", None)
        .await
        .unwrap();

    // User1 should only see their files
    let user1_entries = store.list_path(USER1, "").await.unwrap();
    assert_eq!(user1_entries.len(), 1);
    assert_eq!(user1_entries[0].name, "user1.txt");

    // User2 should only see their files
    let user2_entries = store.list_path(USER2, "").await.unwrap();
    assert_eq!(user2_entries.len(), 1);
    assert_eq!(user2_entries[0].name, "user2.txt");
}

#[tokio::test]
async fn test_namespace_cid_returned_on_write() {
    let (store, _dir) = create_namespace_test_infra();

    let result = store
        .write_path(TEST_OWNER, "test.txt", b"hello", None)
        .await
        .unwrap();

    // CID should be returned
    assert!(!result.cid.to_string().is_empty());
    assert!(result.cid.to_string().contains("sha256"));

    // Namespace CID should also be returned
    assert!(!result.namespace_cid.to_string().is_empty());
    assert!(result.namespace_cid.to_string().contains("sha256"));
}

#[tokio::test]
async fn test_session_info_capabilities_count() {
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    let session = session_registry
        .create_session(SessionType::Capsule, None)
        .await;
    let session_id = session.id.to_string();

    // Initially zero granted capabilities
    let granted = pending_store.list_session_granted(&session_id).await;
    assert_eq!(granted.len(), 0, "should start with 0 capabilities");

    // Create and grant first capability
    let req1 = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    let token1 = capability_manager.grant(
        &session_id,
        ResourceId::new("localhost://Users/self/Documents/photos/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    pending_store
        .grant_request(&req1.id.to_string(), token1, GrantDuration::Session)
        .await
        .unwrap();

    let granted = pending_store.list_session_granted(&session_id).await;
    assert_eq!(
        granted.len(),
        1,
        "should have 1 capability after first grant"
    );

    // Create and grant second capability
    let req2 = pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/docs/*"),
            Action::Write,
        )
        .await;

    let token2 = capability_manager.grant(
        &session_id,
        ResourceId::new("localhost://Users/self/Documents/docs/*"),
        Action::Write,
        TokenConstraints::default(),
        None,
    );

    pending_store
        .grant_request(&req2.id.to_string(), token2, GrantDuration::Session)
        .await
        .unwrap();

    let granted = pending_store.list_session_granted(&session_id).await;
    assert_eq!(
        granted.len(),
        2,
        "should have 2 capabilities after second grant"
    );

    // Pending request should NOT be counted
    pending_store
        .create_request(
            session.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/music/*"),
            Action::Read,
        )
        .await;

    let granted = pending_store.list_session_granted(&session_id).await;
    assert_eq!(
        granted.len(),
        2,
        "pending request should not increase granted count"
    );

    // Other session's capabilities should not appear
    let other = session_registry
        .create_session(SessionType::Capsule, None)
        .await;
    let other_req = pending_store
        .create_request(
            other.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/other/*"),
            Action::Read,
        )
        .await;
    let other_token = capability_manager.grant(
        &other.id.to_string(),
        ResourceId::new("localhost://Users/self/Documents/other/*"),
        Action::Read,
        TokenConstraints::default(),
        None,
    );
    pending_store
        .grant_request(
            &other_req.id.to_string(),
            other_token,
            GrantDuration::Session,
        )
        .await
        .unwrap();

    let granted = pending_store.list_session_granted(&session_id).await;
    assert_eq!(
        granted.len(),
        2,
        "other session's grants should not be counted"
    );
}

#[tokio::test]
async fn test_namespace_path_normalization() {
    let (store, _dir) = create_namespace_test_infra();

    // Write with the current rooted namespace prefix
    store
        .write_path(
            TEST_OWNER,
            "localhost://Public/photos/test.jpg",
            b"img",
            None,
        )
        .await
        .unwrap();

    // Read without prefix
    let result = store.read_path(TEST_OWNER, "photos/test.jpg").await;
    assert!(result.is_ok());

    // Read with leading slash
    let result = store.read_path(TEST_OWNER, "/photos/test.jpg").await;
    assert!(result.is_ok());
}

// ==================== Shadow Verification / Divergence Tests ====================

#[tokio::test]
async fn test_shadow_divergence_visible_in_audit_log() {
    // End-to-end: deny via shell → shadow (auto-grant) diverges → audit has policy_divergence
    let (session_registry, capability_manager, pending_store) = create_test_infra();
    let audit_log = capability_manager.audit_log().clone();

    // Create evaluator with shadow: real=passthrough, shadow=auto-grant
    let evaluator = PolicyEvaluator::with_shadow(
        Box::new(ShellPassthroughVerifier),
        Box::new(AutoGrantVerifier),
        audit_log.clone(),
    );

    // Capsule requests a capability
    let capsule = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    let request = pending_store
        .create_request(
            capsule.id.clone(),
            ResourceId::new("localhost://Users/self/Documents/sensitive/*"),
            Action::Delete,
        )
        .await;

    // Shell denies → real=deny, shadow=grant → divergence
    let decision = evaluator.evaluate(&request, PolicyOutcome::Deny, "User denied access");
    assert_eq!(decision.outcome, PolicyOutcome::Deny);

    // Query audit log filtered by policy_divergence — should find exactly one
    let divergences = audit_log.recent_events_filtered(100, Some("policy_divergence"));
    assert_eq!(
        divergences.len(),
        1,
        "Expected exactly 1 policy_divergence event, got {}",
        divergences.len()
    );

    // Verify the divergence event contains correct outcomes
    match &divergences[0] {
        elastos_runtime::primitives::audit::AuditEvent::PolicyDivergence {
            real_outcome,
            shadow_outcome,
            request_id,
            ..
        } => {
            assert_eq!(real_outcome, "deny");
            assert_eq!(shadow_outcome, "grant");
            assert_eq!(request_id, &request.id.to_string());
        }
        other => panic!(
            "Expected PolicyDivergence event, got {:?}",
            other.event_type_name()
        ),
    }

    // Also verify the shadow decision event is present and flagged
    let decisions = audit_log.recent_events_filtered(100, Some("policy_decision_made"));
    let shadow_decisions: Vec<_> = decisions
        .iter()
        .filter(|e| {
            matches!(
                e,
                elastos_runtime::primitives::audit::AuditEvent::PolicyDecisionMade {
                    shadow: true,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        shadow_decisions.len(),
        1,
        "Expected exactly 1 shadow decision event"
    );
}

#[tokio::test]
async fn test_no_shadow_no_divergence_events() {
    // Without shadow mode, no divergence or shadow events should appear
    let (_session_registry, capability_manager, pending_store) = create_test_infra();
    let audit_log = capability_manager.audit_log().clone();

    // Evaluator without shadow (default behavior)
    let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());

    let request = pending_store
        .create_request(
            elastos_runtime::session::SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    let decision = evaluator.evaluate(&request, PolicyOutcome::Deny, "Denied");
    assert_eq!(decision.outcome, PolicyOutcome::Deny);

    // No divergence events
    let divergences = audit_log.recent_events_filtered(100, Some("policy_divergence"));
    assert_eq!(divergences.len(), 0, "No divergence without shadow mode");

    // No shadow decision events
    let decisions = audit_log.recent_events_filtered(100, Some("policy_decision_made"));
    let shadow_decisions: Vec<_> = decisions
        .iter()
        .filter(|e| {
            matches!(
                e,
                elastos_runtime::primitives::audit::AuditEvent::PolicyDecisionMade {
                    shadow: true,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        shadow_decisions.len(),
        0,
        "No shadow events without shadow mode"
    );
}

#[tokio::test]
async fn test_shadow_agreement_no_divergence() {
    // When both agree (grant + auto-grant), no divergence event
    let (_session_registry, capability_manager, pending_store) = create_test_infra();
    let audit_log = capability_manager.audit_log().clone();

    let evaluator = PolicyEvaluator::with_shadow(
        Box::new(ShellPassthroughVerifier),
        Box::new(AutoGrantVerifier),
        audit_log.clone(),
    );

    let request = pending_store
        .create_request(
            elastos_runtime::session::SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
        )
        .await;

    // Both grant → no divergence
    let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Auto-granted");
    assert_eq!(decision.outcome, PolicyOutcome::Grant);

    let divergences = audit_log.recent_events_filtered(100, Some("policy_divergence"));
    assert_eq!(divergences.len(), 0, "No divergence when both agree");

    // But shadow decision should still be emitted
    let decisions = audit_log.recent_events_filtered(100, Some("policy_decision_made"));
    assert_eq!(decisions.len(), 2, "Should have real + shadow decisions");
}

#[tokio::test]
async fn test_rules_shadow_diverges_on_blocked_resource() {
    // End-to-end: shell grants access to a blocked resource → RulesVerifier
    // shadow denies → audit log contains a policy_divergence event.
    let (_session_registry, capability_manager, pending_store) = create_test_infra();
    let audit_log = capability_manager.audit_log().clone();

    let evaluator = PolicyEvaluator::with_shadow(
        Box::new(ShellPassthroughVerifier),
        Box::new(RulesVerifier::with_defaults()),
        audit_log.clone(),
    );

    // Request targets a blocklisted resource segment ("admin")
    let request = pending_store
        .create_request(
            elastos_runtime::session::SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/admin/secrets"),
            Action::Read,
        )
        .await;

    // Shell grants — but rules shadow should deny (resource blocklist)
    let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Shell auto-grant");
    assert_eq!(
        decision.outcome,
        PolicyOutcome::Grant,
        "Real decision unchanged"
    );

    // Divergence event must exist: real=grant, shadow=deny
    let divergences = audit_log.recent_events_filtered(100, Some("policy_divergence"));
    assert_eq!(
        divergences.len(),
        1,
        "Expected exactly 1 policy_divergence event from rules shadow"
    );

    match &divergences[0] {
        elastos_runtime::primitives::audit::AuditEvent::PolicyDivergence {
            real_outcome,
            shadow_outcome,
            ..
        } => {
            assert_eq!(real_outcome, "grant");
            assert_eq!(shadow_outcome, "deny");
        }
        other => panic!(
            "Expected PolicyDivergence event, got: {:?}",
            other.event_type_name()
        ),
    }
}

// ==================== AI Provider Capability Gating Tests ====================

#[tokio::test]
async fn test_ai_provider_proxy_requires_capability() {
    // Verify that elastos://ai/ resources go through the same capability flow as other providers.
    // A capsule must hold a granted elastos://ai/* capability token to access the AI provider.
    let (session_registry, capability_manager, pending_store) = create_test_infra();

    let capsule = session_registry
        .create_session(SessionType::Capsule, None)
        .await;

    // Request elastos://ai/* capability
    let request = pending_store
        .create_request(
            capsule.id.clone(),
            ResourceId::new("elastos://ai/*"),
            Action::Execute,
        )
        .await;

    assert!(request.is_pending());

    // Without a grant, the capsule has no elastos://ai/ capability
    let no_token = capability_manager
        .validate(
            &capability_manager.grant(
                "wrong-session",
                ResourceId::new("elastos://ai/*"),
                Action::Execute,
                TokenConstraints::default(),
                None,
            ),
            &capsule.id.to_string(),
            Action::Execute,
            &ResourceId::new("elastos://ai/*"),
            None,
        )
        .await;
    assert!(
        no_token.is_err(),
        "Token for wrong session should fail validation"
    );

    // Grant elastos://ai/* to the capsule
    let token = capability_manager.grant(
        &capsule.id.to_string(),
        ResourceId::new("elastos://ai/*"),
        Action::Execute,
        TokenConstraints::default(),
        None,
    );

    pending_store
        .grant_request(
            &request.id.to_string(),
            token.clone(),
            GrantDuration::Session,
        )
        .await
        .unwrap();

    // Now the token should validate for the capsule
    let valid = capability_manager
        .validate(
            &token,
            &capsule.id.to_string(),
            Action::Execute,
            &ResourceId::new("elastos://ai/*"),
            None,
        )
        .await;
    assert!(
        valid.is_ok(),
        "Granted elastos://ai/* token should validate"
    );

    // The grant should show in session grants
    let granted = pending_store
        .list_session_granted(&capsule.id.to_string())
        .await;
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].resource.to_string(), "elastos://ai/*");
}

#[tokio::test]
async fn test_ai_capability_scope_matching() {
    // Token for elastos://ai/local/* validates for local backend but not venice.
    let (_, capability_manager, _) = create_test_infra();

    let session_id = "scope-test-session";

    let token = capability_manager.grant(
        session_id,
        ResourceId::new("elastos://ai/local/*"),
        Action::Execute,
        TokenConstraints::default(),
        None,
    );

    // Should validate for local backend
    let valid = capability_manager
        .validate(
            &token,
            session_id,
            Action::Execute,
            &ResourceId::new("elastos://ai/local/chat_completions"),
            None,
        )
        .await;
    assert!(
        valid.is_ok(),
        "Token should validate for elastos://ai/local/chat_completions"
    );

    // Should NOT validate for venice backend
    let invalid = capability_manager
        .validate(
            &token,
            session_id,
            Action::Execute,
            &ResourceId::new("elastos://ai/venice/chat_completions"),
            None,
        )
        .await;
    assert!(
        invalid.is_err(),
        "Token for local/* should not validate for venice/*"
    );
}

#[tokio::test]
async fn test_budget_scope_shadow_blocks_unknown_ai_backend() {
    // Shell grants elastos://ai/rogue/chat → RulesVerifier shadow denies.
    // Divergence: real=grant, shadow=deny.
    let (_, capability_manager, pending_store) = create_test_infra();
    let audit_log = capability_manager.audit_log().clone();

    let evaluator = PolicyEvaluator::with_shadow(
        Box::new(ShellPassthroughVerifier),
        Box::new(RulesVerifier::with_defaults()),
        audit_log.clone(),
    );

    let request = pending_store
        .create_request(
            elastos_runtime::session::SessionId::from_string("ai-budget-test"),
            ResourceId::new("elastos://ai/rogue/chat"),
            Action::Execute,
        )
        .await;

    // Shell grants — but rules shadow should deny (unknown backend)
    let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Shell auto-grant");
    assert_eq!(
        decision.outcome,
        PolicyOutcome::Grant,
        "Real decision unchanged"
    );

    // Divergence event: real=grant, shadow=deny
    let divergences = audit_log.recent_events_filtered(100, Some("policy_divergence"));
    assert_eq!(
        divergences.len(),
        1,
        "Expected 1 policy_divergence from AI budget/scope shadow"
    );

    match &divergences[0] {
        elastos_runtime::primitives::audit::AuditEvent::PolicyDivergence {
            real_outcome,
            shadow_outcome,
            ..
        } => {
            assert_eq!(real_outcome, "grant");
            assert_eq!(shadow_outcome, "deny");
        }
        other => panic!(
            "Expected PolicyDivergence, got: {:?}",
            other.event_type_name()
        ),
    }
}
