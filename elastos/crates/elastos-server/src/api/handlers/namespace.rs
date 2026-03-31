//! Namespace API handlers
//!
//! HTTP handlers for the namespace system - browse, read, write, delete operations.
//! All operations require authentication and capability tokens.
//! Shell sessions are exempt (orchestrator privilege).
//! Resource format: `elastos://namespace/<path>`

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use sha2::{Digest, Sha256};

use elastos_runtime::capability::{Action, CapabilityManager, CapabilityToken, ResourceId};
use elastos_runtime::namespace::{ContentId, NamespaceEntry, NamespaceStore};
use elastos_runtime::session::Session;

/// Shared state for namespace handlers
#[derive(Clone)]
pub struct NamespaceState {
    pub namespace_store: Arc<NamespaceStore>,
    pub capability_manager: Option<Arc<CapabilityManager>>,
}

// === Capability Enforcement ===

/// Enforce capability token for namespace operations.
///
/// Shell sessions are exempt (orchestrator privilege).
/// Capsule sessions must provide a valid `X-Capability-Token` header.
/// Resource format: `elastos://namespace/<path>`
async fn enforce_capability(
    state: &NamespaceState,
    session: &Session,
    headers: &HeaderMap,
    path: &str,
    action: Action,
) -> Result<(), (StatusCode, String)> {
    // Shell sessions have orchestrator privilege — no token needed
    if session.is_shell() {
        return Ok(());
    }

    let cap_mgr = match state.capability_manager {
        Some(ref mgr) => mgr,
        None => {
            return Err((
                StatusCode::FORBIDDEN,
                "Capability manager not configured — access denied (no ambient authority)"
                    .to_string(),
            ));
        }
    };

    let token_b64 = headers
        .get("X-Capability-Token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "missing X-Capability-Token header".to_string(),
            )
        })?;

    let token = CapabilityToken::from_base64(token_b64).map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            format!("invalid capability token: {}", e),
        )
    })?;

    let resource = ResourceId::new(format!("elastos://namespace/{}", path));

    cap_mgr
        .validate(&token, session.id.as_str(), action, &resource, None)
        .await
        .map_err(|e| (StatusCode::FORBIDDEN, format!("capability denied: {}", e)))
}

// === List Path ===

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Path to list (defaults to root)
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct EntryOutput {
    /// Entry name
    pub name: String,
    /// Full path (elastos://namespace/...)
    pub path: String,
    /// Type: "file" or "directory"
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Content ID (for files)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Size in bytes
    pub size: u64,
    /// Content type (for files)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Modification timestamp (unix secs)
    pub modified_at: u64,
    /// Whether content is cached locally
    pub cached: bool,
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    /// Path that was listed
    pub path: String,
    /// Entries at the path
    pub entries: Vec<EntryOutput>,
}

/// GET /api/namespace/list
///
/// List entries at a path in the user's namespace.
pub async fn list_path(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListOutput>, (StatusCode, String)> {
    let owner = session_owner(&session)?;
    let path = normalize_query_path(&query.path)?;

    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;

    let entries = state
        .namespace_store
        .list_path(&owner, &path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let entries: Vec<EntryOutput> = entries
        .into_iter()
        .map(|e| EntryOutput {
            name: e.name,
            path: e.path,
            entry_type: e.entry_type,
            cid: e.cid.map(|c| c.to_string()),
            size: e.size,
            content_type: e.content_type,
            modified_at: e.modified_at,
            cached: e.cached,
        })
        .collect();

    Ok(Json(ListOutput {
        path: namespace_uri(&path),
        entries,
    }))
}

// === Resolve Path ===

#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// Path to resolve
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveOutput {
    /// The resolved path
    pub path: String,
    /// Type: "file" or "directory"
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Content ID (for files)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Size in bytes
    pub size: u64,
    /// Content type (for files)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Modification timestamp
    pub modified_at: u64,
    /// Whether content is cached locally
    pub cached: bool,
    /// Number of children (for directories)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<usize>,
}

/// GET /api/namespace/resolve
///
/// Resolve a path to its entry info (without fetching content).
pub async fn resolve_path(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Query(query): Query<ResolveQuery>,
) -> Result<Json<ResolveOutput>, (StatusCode, String)> {
    let owner = session_owner(&session)?;
    let path = normalize_query_path(&query.path)?;

    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;

    let namespace = state
        .namespace_store
        .load(&owner)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let entry = namespace
        .resolve(&path)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Path not found: {}", path)))?;

    let (cid, content_type, child_count) = match entry {
        NamespaceEntry::File {
            cid, content_type, ..
        } => (Some(cid.clone()), content_type.clone(), None),
        NamespaceEntry::Directory { children, .. } => (None, None, Some(children.len())),
    };

    let cached = if let Some(ref c) = cid {
        state.namespace_store.is_cached(c).await
    } else {
        true
    };

    Ok(Json(ResolveOutput {
        path: namespace_uri(&path),
        entry_type: if entry.is_file() {
            "file".into()
        } else {
            "directory".into()
        },
        cid: cid.map(|c| c.to_string()),
        size: entry.size(),
        content_type,
        modified_at: entry.modified_at().unix_secs,
        cached,
        child_count,
    }))
}

// === Read Content ===

#[derive(Debug, Deserialize)]
pub struct ReadQuery {
    /// Path to read
    pub path: String,
}

/// GET /api/namespace/read
///
/// Read file content from the user's namespace.
/// Returns the raw content with appropriate Content-Type header.
pub async fn read_content(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Query(query): Query<ReadQuery>,
) -> Result<Response, (StatusCode, String)> {
    let owner = session_owner(&session)?;
    let path = normalize_query_path(&query.path)?;

    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;

    let result = state
        .namespace_store
        .read_path(&owner, &path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let content_type = result
        .content_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, result.size)
        .header("X-ElastOS-CID", result.cid.to_string())
        .header(
            "X-ElastOS-Cached",
            if result.was_cached { "true" } else { "false" },
        )
        .body(axum::body::Body::from(result.content))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(response)
}

// === Write Content ===

#[derive(Debug, Deserialize)]
pub struct WriteQuery {
    /// Path to write to
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct WriteOutput {
    /// Path that was written
    pub path: String,
    /// Content ID of the new content
    pub cid: String,
    /// Size in bytes
    pub size: u64,
    /// New namespace CID after the write
    pub namespace_cid: String,
}

/// POST /api/namespace/write
///
/// Write content to a path in the user's namespace.
/// Content is sent as the request body.
/// Content-Type header is preserved.
pub async fn write_content(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    Query(query): Query<WriteQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WriteOutput>, (StatusCode, String)> {
    let owner = session_owner(&session)?;
    let path = normalize_query_path(&query.path)?;

    enforce_capability(&state, &session, &headers, &path, Action::Write).await?;

    // Get content type from header
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    let result = state
        .namespace_store
        .write_path(&owner, &path, &body, content_type)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(WriteOutput {
        path: result.path,
        cid: result.cid.to_string(),
        size: result.size,
        namespace_cid: result.namespace_cid.to_string(),
    }))
}

// === Delete Path ===

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    /// Path to delete
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteOutput {
    /// Path that was deleted
    pub path: String,
    /// New namespace CID after deletion
    pub namespace_cid: String,
}

/// DELETE /api/namespace/delete
///
/// Delete a path from the user's namespace.
pub async fn delete_path(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Query(query): Query<DeleteQuery>,
) -> Result<Json<DeleteOutput>, (StatusCode, String)> {
    let owner = session_owner(&session)?;
    let path = normalize_query_path(&query.path)?;

    enforce_capability(&state, &session, &headers, &path, Action::Delete).await?;

    let result = state
        .namespace_store
        .delete_path(&owner, &path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(DeleteOutput {
        path: result.path,
        namespace_cid: result.namespace_cid.to_string(),
    }))
}

// === Namespace Status ===

#[derive(Debug, Serialize)]
pub struct NamespaceStatusOutput {
    /// Owner's public key (hex)
    pub owner: String,
    /// Namespace content ID
    pub namespace_cid: String,
    /// Total number of entries
    pub entry_count: u64,
    /// Total size of all content (bytes)
    pub total_size: u64,
    /// Size of locally cached content (bytes)
    pub cached_size: u64,
    /// Last modification timestamp
    pub last_modified: u64,
    /// Whether namespace is signed
    pub signed: bool,
}

/// GET /api/namespace/status
///
/// Get status information about the user's namespace.
pub async fn namespace_status(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
) -> Result<Json<NamespaceStatusOutput>, (StatusCode, String)> {
    let owner = session_owner(&session)?;

    enforce_capability(&state, &session, &headers, "", Action::Read).await?;

    let status = state
        .namespace_store
        .namespace_status(&owner)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(NamespaceStatusOutput {
        owner: status.owner,
        namespace_cid: status.namespace_cid.to_string(),
        entry_count: status.entry_count,
        total_size: status.total_size,
        cached_size: status.cached_size,
        last_modified: status.last_modified,
        signed: status.signed,
    }))
}

// === Cache Status ===

#[derive(Debug, Serialize)]
pub struct CacheStatusOutput {
    /// Number of cached entries
    pub entry_count: usize,
    /// Total bytes cached
    pub total_bytes: u64,
    /// Cache size limit
    pub limit_bytes: u64,
    /// Usage percentage
    pub usage_percent: f64,
}

/// GET /api/namespace/cache
///
/// Get cache statistics.
pub async fn cache_status(State(state): State<NamespaceState>) -> Json<CacheStatusOutput> {
    let stats = state.namespace_store.cache_stats().await;

    let usage_percent = if stats.limit_bytes > 0 {
        (stats.total_bytes as f64 / stats.limit_bytes as f64) * 100.0
    } else {
        0.0
    };

    Json(CacheStatusOutput {
        entry_count: stats.entry_count,
        total_bytes: stats.total_bytes,
        limit_bytes: stats.limit_bytes,
        usage_percent,
    })
}

// === Prefetch Content ===

#[derive(Debug, Deserialize)]
pub struct PrefetchInput {
    /// CIDs to prefetch
    pub cids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PrefetchOutput {
    /// Number of CIDs successfully fetched
    pub fetched: usize,
    /// Number of CIDs that failed
    pub failed: usize,
    /// Errors for failed fetches
    pub errors: Vec<PrefetchError>,
}

#[derive(Debug, Serialize)]
pub struct PrefetchError {
    pub cid: String,
    pub error: String,
}

/// POST /api/namespace/prefetch
///
/// Prefetch content by CIDs to warm the cache.
pub async fn prefetch_content(
    State(state): State<NamespaceState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Json(input): Json<PrefetchInput>,
) -> Result<Json<PrefetchOutput>, (StatusCode, String)> {
    enforce_capability(&state, &session, &headers, "", Action::Read).await?;

    let cids: Vec<ContentId> = input.cids.into_iter().map(ContentId::from).collect();

    let result = state.namespace_store.prefetch(&cids).await;

    Ok(Json(PrefetchOutput {
        fetched: result.fetched,
        failed: result.failed,
        errors: result
            .errors
            .into_iter()
            .map(|(cid, error)| PrefetchError {
                cid: cid.to_string(),
                error,
            })
            .collect(),
    }))
}

// === Helpers ===

/// Get the owner key from the session.
/// For now, we use the session ID as the owner until we have proper key management.
fn session_owner(session: &Session) -> Result<String, (StatusCode, String)> {
    // Use the session's owner field if set, otherwise derive from session ID
    // In production, this would come from the user's authenticated identity
    if let Some(ref owner) = session.owner {
        Ok(owner.clone())
    } else {
        // For testing/development, create a deterministic owner from session ID
        // This is a 64-char hex string (32 bytes) that uniquely identifies the session
        let hash = Sha256::digest(session.id.as_str().as_bytes());
        Ok(hex::encode(hash))
    }
}

/// Normalize a path from query parameters.
/// Rejects path traversal (`..`, `.`) and collapses duplicate slashes.
fn normalize_query_path(path: &str) -> Result<String, (StatusCode, String)> {
    let path = path.trim();
    let path = path.strip_prefix("elastos://namespace/").unwrap_or(path);
    // Split on '/', filter empty segments (collapses duplicate slashes)
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Reject path traversal
    if segments.iter().any(|seg| *seg == ".." || *seg == ".") {
        return Err((
            StatusCode::BAD_REQUEST,
            "path traversal not allowed".to_string(),
        ));
    }
    Ok(segments.join("/"))
}

fn namespace_uri(path: &str) -> String {
    if path.is_empty() {
        "elastos://namespace".to_string()
    } else {
        format!("elastos://namespace/{}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::capability::{CapabilityStore, TokenConstraints};
    use elastos_runtime::namespace::{
        ContentResolver, NamespaceAuditSink, NullAuditSink, NullFetcher, ResolverConfig,
    };
    use elastos_runtime::primitives::audit::AuditLog;
    use elastos_runtime::primitives::metrics::MetricsManager;
    use elastos_runtime::session::SessionType;

    struct TestNsAudit;
    impl NamespaceAuditSink for TestNsAudit {
        fn namespace_loaded(&self, _: &str) {}
        fn namespace_created(&self, _: &str) {}
        fn namespace_saved(&self, _: &str, _: &str) {}
    }

    fn test_namespace_state() -> (NamespaceState, tempfile::TempDir) {
        // Capability manager
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let cap_mgr = Arc::new(CapabilityManager::new(store, audit_log, metrics));

        // Namespace store (minimal, not used by enforce_capability tests)
        let dir = tempfile::tempdir().unwrap();
        let resolver_audit: Arc<NullAuditSink> = Arc::new(NullAuditSink);
        let resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            resolver_audit,
            Arc::new(NullFetcher),
        ));
        let ns_store = Arc::new(NamespaceStore::new(
            dir.path().to_path_buf(),
            resolver,
            Arc::new(TestNsAudit) as Arc<dyn NamespaceAuditSink>,
        ));

        let state = NamespaceState {
            namespace_store: ns_store,
            capability_manager: Some(cap_mgr),
        };
        (state, dir)
    }

    #[tokio::test]
    async fn test_enforce_capability_shell_exempt() {
        let (state, _dir) = test_namespace_state();
        let session = Session::new(SessionType::Shell, None);
        let headers = HeaderMap::new(); // no token needed

        let result =
            enforce_capability(&state, &session, &headers, "photos/test.jpg", Action::Read).await;
        assert!(result.is_ok(), "shell session should be exempt");
    }

    #[tokio::test]
    async fn test_enforce_capability_missing_token() {
        let (state, _dir) = test_namespace_state();
        let session = Session::new(SessionType::Capsule, None);
        let headers = HeaderMap::new(); // no X-Capability-Token

        let result =
            enforce_capability(&state, &session, &headers, "photos/test.jpg", Action::Read).await;
        assert!(result.is_err());
        let (status, msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            msg.contains("missing"),
            "error should mention missing token: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_enforce_capability_invalid_token() {
        let (state, _dir) = test_namespace_state();
        let session = Session::new(SessionType::Capsule, None);
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", "not-valid-base64!!!".parse().unwrap());

        let result =
            enforce_capability(&state, &session, &headers, "photos/test.jpg", Action::Read).await;
        assert!(result.is_err());
        let (status, msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            msg.contains("invalid capability token"),
            "error should mention invalid token: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_enforce_capability_valid_token() {
        let (state, _dir) = test_namespace_state();
        let session = Session::new(SessionType::Capsule, None);
        let cap_mgr = state.capability_manager.as_ref().unwrap();

        // Grant a token for this session+resource+action
        let token = cap_mgr.grant(
            session.id.as_str(),
            ResourceId::new("elastos://namespace/photos/test.jpg"),
            Action::Read,
            TokenConstraints::new(0, false, None, None),
            None,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            token.to_base64().unwrap().parse().unwrap(),
        );

        let result =
            enforce_capability(&state, &session, &headers, "photos/test.jpg", Action::Read).await;
        assert!(result.is_ok(), "valid token should pass: {:?}", result);
    }

    #[tokio::test]
    async fn test_enforce_capability_wrong_action() {
        let (state, _dir) = test_namespace_state();
        let session = Session::new(SessionType::Capsule, None);
        let cap_mgr = state.capability_manager.as_ref().unwrap();

        // Grant Read, but try Write
        let token = cap_mgr.grant(
            session.id.as_str(),
            ResourceId::new("elastos://namespace/photos/test.jpg"),
            Action::Read,
            TokenConstraints::new(0, false, None, None),
            None,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            token.to_base64().unwrap().parse().unwrap(),
        );

        let result =
            enforce_capability(&state, &session, &headers, "photos/test.jpg", Action::Write).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_normalize_query_path() {
        assert_eq!(normalize_query_path("").unwrap(), "");
        assert_eq!(normalize_query_path("/").unwrap(), "");
        assert_eq!(normalize_query_path("photos").unwrap(), "photos");
        assert_eq!(normalize_query_path("/photos/").unwrap(), "photos");
        assert_eq!(
            normalize_query_path("elastos://namespace/photos/test.jpg").unwrap(),
            "photos/test.jpg"
        );
    }

    #[test]
    fn test_normalize_rejects_traversal() {
        assert!(normalize_query_path("..").is_err());
        assert!(normalize_query_path(".").is_err());
        assert!(normalize_query_path("a/../b").is_err());
        assert!(normalize_query_path("/photos/../etc/passwd").is_err());
        assert!(normalize_query_path("./hidden").is_err());
    }

    #[test]
    fn test_normalize_collapses_slashes() {
        assert_eq!(
            normalize_query_path("photos//evil.jpg").unwrap(),
            "photos/evil.jpg"
        );
        assert_eq!(normalize_query_path("///a///b///c///").unwrap(), "a/b/c");
    }
}
