//! Mutable storage API handlers
//!
//! HTTP handlers for file-backed localhost:// roots.
//!
//! Public contract: rooted local paths such as `Users/self/...` or `MyWebSite/...`.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use elastos_common::localhost::rooted_localhost_uri;
use elastos_runtime::capability::{Action, CapabilityManager, CapabilityToken, ResourceId};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::provider::{
    EntryType as ProviderEntryType, ProviderError, ProviderRegistry, ResourceAction,
    ResourceResponse,
};
use elastos_runtime::session::Session;

/// Shared state for storage handlers (backed by ProviderRegistry)
#[derive(Clone)]
pub struct ProviderStorageState {
    pub registry: Arc<ProviderRegistry>,
    pub audit_log: Option<Arc<AuditLog>>,
    pub capability_manager: Option<Arc<CapabilityManager>>,
    /// Per-capsule storage quota in MB. 0 means unlimited.
    pub storage_quota_mb: u32,
}

/// Storage API errors
#[derive(Debug)]
pub enum StorageApiError {
    NotFound(String),
    PermissionDenied(String),
    InvalidPath(String),
    QuotaExceeded(String),
    Internal(String),
}

impl IntoResponse for StorageApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            StorageApiError::NotFound(path) => {
                (StatusCode::NOT_FOUND, format!("Not found: {}", path))
            }
            StorageApiError::PermissionDenied(msg) => {
                (StatusCode::FORBIDDEN, format!("Permission denied: {}", msg))
            }
            StorageApiError::InvalidPath(msg) => {
                (StatusCode::BAD_REQUEST, format!("Invalid path: {}", msg))
            }
            StorageApiError::QuotaExceeded(msg) => (
                StatusCode::INSUFFICIENT_STORAGE,
                format!("Storage quota exceeded: {}", msg),
            ),
            StorageApiError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {}", msg),
            ),
        };

        (status, message).into_response()
    }
}

impl From<ProviderError> for StorageApiError {
    fn from(err: ProviderError) -> Self {
        match err {
            ProviderError::NotFound(p) => StorageApiError::NotFound(p),
            ProviderError::PermissionDenied(m) => StorageApiError::PermissionDenied(m),
            ProviderError::InvalidUri(m) => StorageApiError::InvalidPath(m),
            ProviderError::NoProvider(m) => {
                StorageApiError::Internal(format!("No provider: {}", m))
            }
            ProviderError::Provider(m) => StorageApiError::Internal(m),
            ProviderError::Io(e) => StorageApiError::Internal(e.to_string()),
        }
    }
}

// === Read File ===

/// GET /api/localhost/*path
///
/// Read a file from a file-backed localhost root.
/// Returns the file contents with appropriate content-type.
pub async fn read_file(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;
    let uri = canonical_local_uri(&path)?;

    let result = state
        .registry
        .route(&uri, session.id.as_str(), ResourceAction::Read, None)
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "read", success);

    match result? {
        ResourceResponse::Data(data) => {
            let content_type = guess_content_type(&path);
            Ok(([(header::CONTENT_TYPE, content_type)], data).into_response())
        }
        _ => Err(StorageApiError::Internal("Unexpected response".into())),
    }
}

// === Write File ===

/// PUT /api/localhost/*path
///
/// Write a file to a file-backed localhost root.
/// Creates parent directories as needed.
pub async fn write_file(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
    body: Bytes,
) -> Result<Json<WriteOutput>, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Write).await?;

    // Enforce storage quota (0 = unlimited)
    if state.storage_quota_mb > 0 {
        let quota_bytes = state.storage_quota_mb as u64 * 1024 * 1024;
        let current_usage = state.registry.storage_usage(&user_id).await.unwrap_or(0);
        if current_usage + body.len() as u64 > quota_bytes {
            return Err(StorageApiError::QuotaExceeded(format!(
                "write of {} bytes would exceed {}MB quota (current usage: {} bytes)",
                body.len(),
                state.storage_quota_mb,
                current_usage
            )));
        }
    }

    let uri = canonical_local_uri(&path)?;

    let result = state
        .registry
        .route(
            &uri,
            session.id.as_str(),
            ResourceAction::Write,
            Some(body.to_vec()),
        )
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "write", success);

    match result? {
        ResourceResponse::Written { bytes } => Ok(Json(WriteOutput {
            path: canonical_local_uri(&path)?,
            size: bytes as u64,
        })),
        _ => Err(StorageApiError::Internal("Unexpected response".into())),
    }
}

#[derive(Debug, Serialize)]
pub struct WriteOutput {
    /// Path that was written
    pub path: String,
    /// Size in bytes
    pub size: u64,
}

// === Delete ===

/// DELETE /api/localhost/*path
///
/// Delete a file or empty directory.
/// Use ?recursive=true to delete non-empty directories.
pub async fn delete_path(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<Json<DeleteOutput>, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Delete).await?;
    let uri = canonical_local_uri(&path)?;
    let recursive = query.recursive.unwrap_or(false);

    let result = state
        .registry
        .route_with_options(
            &uri,
            session.id.as_str(),
            ResourceAction::Delete,
            None,
            recursive,
        )
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "delete", success);
    result?;

    Ok(Json(DeleteOutput {
        path: canonical_local_uri(&path)?,
        deleted: true,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    /// Delete recursively (for non-empty directories)
    pub recursive: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DeleteOutput {
    /// Path that was deleted
    pub path: String,
    /// Whether deletion succeeded
    pub deleted: bool,
}

// === List Directory ===

/// GET /api/localhost/*path?list=true
///
/// List contents of a directory.
pub async fn list_dir(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Json<ListOutput>, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;
    let uri = canonical_local_uri(&path)?;

    let result = state
        .registry
        .route(&uri, session.id.as_str(), ResourceAction::List, None)
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "list", success);

    match result? {
        ResourceResponse::List(entries) => {
            let entries: Vec<EntryOutput> = entries
                .into_iter()
                .map(|e| {
                    let entry_type = if e.is_directory {
                        elastos_storage::EntryType::Directory
                    } else {
                        elastos_storage::EntryType::File
                    };
                    let entry_path = if path.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{}/{}", path, e.name)
                    };
                    Ok(EntryOutput {
                        name: e.name.clone(),
                        path: canonical_local_uri(&entry_path)?,
                        entry_type,
                        size: e.size.unwrap_or(0),
                        modified: e.modified.unwrap_or(0),
                    })
                })
                .collect::<Result<_, StorageApiError>>()?;

            Ok(Json(ListOutput {
                path: canonical_local_uri(&path)?,
                entries,
            }))
        }
        _ => Err(StorageApiError::Internal("Unexpected response".into())),
    }
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    /// Path that was listed
    pub path: String,
    /// Directory entries
    pub entries: Vec<EntryOutput>,
}

#[derive(Debug, Serialize)]
pub struct EntryOutput {
    /// Entry name
    pub name: String,
    /// Full path (localhost://...)
    pub path: String,
    /// Type: "file" or "directory"
    #[serde(rename = "type")]
    pub entry_type: elastos_storage::EntryType,
    /// Size in bytes
    pub size: u64,
    /// Last modified timestamp (unix seconds)
    pub modified: u64,
}

// === Stat / Exists ===

/// HEAD /api/localhost/*path
///
/// Check if a path exists and get metadata.
/// Returns 200 with headers if exists, 404 if not.
pub async fn stat_path(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Read).await?;
    let uri = canonical_local_uri(&path)?;

    let result = state
        .registry
        .route(&uri, session.id.as_str(), ResourceAction::Stat, None)
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "stat", success);

    match result? {
        ResourceResponse::Metadata {
            size,
            entry_type,
            modified,
        } => {
            let type_str = match entry_type {
                ProviderEntryType::File => "file",
                ProviderEntryType::Directory => "directory",
            };

            Ok((
                StatusCode::OK,
                [
                    ("X-Entry-Type", type_str),
                    ("X-Size", &size.to_string()),
                    ("X-Modified", &modified.to_string()),
                ],
            )
                .into_response())
        }
        _ => Err(StorageApiError::Internal("Unexpected response".into())),
    }
}

// === Mkdir ===

/// POST /api/localhost/*path?mkdir=true
///
/// Create a directory (and parents).
pub async fn mkdir(
    State(state): State<ProviderStorageState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Json<MkdirOutput>, StorageApiError> {
    let user_id = session_user_id(&session);
    let path = normalize_path(&path);
    enforce_capability(&state, &session, &headers, &path, Action::Write).await?;
    let uri = canonical_local_uri(&path)?;

    let result = state
        .registry
        .route(&uri, session.id.as_str(), ResourceAction::Mkdir, None)
        .await;
    let success = result.is_ok();
    emit_audit(&state, &session, &user_id, &uri, "mkdir", success);
    result?;

    Ok(Json(MkdirOutput {
        path: canonical_local_uri(&path)?,
        created: true,
    }))
}

#[derive(Debug, Serialize)]
pub struct MkdirOutput {
    /// Path that was created
    pub path: String,
    /// Whether directory was created
    pub created: bool,
}

// === Capability Enforcement ===

/// Validate that the session has permission for this storage operation.
///
/// Shell sessions are exempt (orchestrator privilege).
/// Capsule sessions must provide a valid capability token in X-Capability-Token header.
async fn enforce_capability(
    state: &ProviderStorageState,
    session: &Session,
    headers: &HeaderMap,
    path: &str,
    action: Action,
) -> Result<(), StorageApiError> {
    // Shell sessions have orchestrator privilege — no token needed
    if session.is_shell() {
        return Ok(());
    }

    let cap_mgr = match state.capability_manager {
        Some(ref mgr) => mgr,
        None => {
            return Err(StorageApiError::PermissionDenied(
                "capability manager not configured — storage access denied (no ambient authority)"
                    .into(),
            ));
        }
    };

    let token_b64 = headers
        .get("X-Capability-Token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            StorageApiError::PermissionDenied("missing X-Capability-Token header".into())
        })?;

    let token = CapabilityToken::from_base64(token_b64)
        .map_err(|e| StorageApiError::PermissionDenied(format!("invalid token: {}", e)))?;

    let resource = ResourceId::new(canonical_local_uri(path)?);

    cap_mgr
        .validate(&token, session.id.as_str(), action, &resource, None)
        .await
        .map_err(|e| StorageApiError::PermissionDenied(e.to_string()))
}

// === Helpers ===

/// Emit audit event for storage operations
fn emit_audit(
    state: &ProviderStorageState,
    session: &Session,
    user_id: &str,
    uri: &str,
    action: &str,
    success: bool,
) {
    if let Some(ref audit) = state.audit_log {
        audit.storage_access(session.id.as_str(), user_id, uri, action, success);
    }
}

/// Get user ID from session for storage isolation.
/// Uses owner if set, otherwise derives from session ID.
fn session_user_id(session: &Session) -> String {
    if let Some(ref owner) = session.owner {
        owner.clone()
    } else {
        // Derive deterministic user ID from session ID
        let hash = Sha256::digest(session.id.as_str().as_bytes());
        hex::encode(hash)
    }
}

/// Normalize a path from the URL.
fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let path = path.strip_prefix("localhost://").unwrap_or(path);
    let path = path.trim_matches('/');
    path.to_string()
}

fn canonical_local_uri(path: &str) -> Result<String, StorageApiError> {
    if path.is_empty() {
        return Ok("localhost://".to_string());
    }

    rooted_localhost_uri(path).ok_or_else(|| {
        StorageApiError::InvalidPath("path must be rooted under localhost://<root>/...".to_string())
    })
}

/// Guess content type from file extension.
fn guess_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "json" => "application/json",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

// === Query params for routing ===

#[derive(Debug, Deserialize)]
pub struct StorageQuery {
    /// If true, list directory contents
    pub list: Option<bool>,
    /// If true, create directory
    pub mkdir: Option<bool>,
}

/// Root handler (no path parameter) - for /api/localhost without a subpath.
pub async fn handle_get_root(
    state: State<ProviderStorageState>,
    session: Extension<Session>,
    headers: HeaderMap,
    Query(query): Query<StorageQuery>,
) -> Result<Response, StorageApiError> {
    let path = Path(String::new());
    if query.list.unwrap_or(false) {
        list_dir(state, session, headers, path)
            .await
            .map(|j| j.into_response())
    } else {
        Err(StorageApiError::InvalidPath(
            "Cannot read root as file".to_string(),
        ))
    }
}

/// Combined handler that routes based on query params and method.
/// This simplifies routing for the wildcard path.
pub async fn handle_get(
    state: State<ProviderStorageState>,
    session: Extension<Session>,
    headers: HeaderMap,
    path: Path<String>,
    Query(query): Query<StorageQuery>,
) -> Result<Response, StorageApiError> {
    if query.list.unwrap_or(false) {
        list_dir(state, session, headers, path)
            .await
            .map(|j| j.into_response())
    } else {
        read_file(state, session, headers, path).await
    }
}

pub async fn handle_post(
    state: State<ProviderStorageState>,
    session: Extension<Session>,
    headers: HeaderMap,
    path: Path<String>,
    Query(query): Query<StorageQuery>,
    body: Bytes,
) -> Result<Response, StorageApiError> {
    if query.mkdir.unwrap_or(false) {
        mkdir(state, session, headers, path)
            .await
            .map(|j| j.into_response())
    } else {
        // POST without mkdir acts like PUT (write file)
        write_file(state, session, headers, path, body)
            .await
            .map(|j| j.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::capability::{CapabilityStore, TokenConstraints};
    use elastos_runtime::primitives::metrics::MetricsManager;
    use elastos_runtime::session::SessionType;

    fn make_capability_manager() -> Arc<CapabilityManager> {
        let store = Arc::new(CapabilityStore::new());
        let audit = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        Arc::new(CapabilityManager::new(store, audit, metrics))
    }

    fn make_session(session_type: SessionType) -> Session {
        Session::new(session_type, None)
    }

    fn make_state(cap_mgr: Option<Arc<CapabilityManager>>) -> ProviderStorageState {
        ProviderStorageState {
            registry: Arc::new(ProviderRegistry::new()),
            audit_log: None,
            capability_manager: cap_mgr,
            storage_quota_mb: 0,
        }
    }

    #[tokio::test]
    async fn test_shell_session_exempt_from_capability_check() {
        let cap_mgr = make_capability_manager();
        let state = make_state(Some(cap_mgr));
        let session = make_session(SessionType::Shell);
        let headers = HeaderMap::new();

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Read,
        )
        .await;
        assert!(
            result.is_ok(),
            "Shell sessions should bypass capability check"
        );
    }

    #[tokio::test]
    async fn test_capsule_session_rejected_without_token() {
        let cap_mgr = make_capability_manager();
        let state = make_state(Some(cap_mgr));
        let session = make_session(SessionType::Capsule);
        let headers = HeaderMap::new();

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Read,
        )
        .await;
        assert!(matches!(result, Err(StorageApiError::PermissionDenied(_))));
    }

    #[tokio::test]
    async fn test_capsule_session_rejected_with_invalid_token() {
        let cap_mgr = make_capability_manager();
        let state = make_state(Some(cap_mgr));
        let session = make_session(SessionType::Capsule);
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", "not-valid-base64!!".parse().unwrap());

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Read,
        )
        .await;
        assert!(matches!(result, Err(StorageApiError::PermissionDenied(_))));
    }

    #[tokio::test]
    async fn test_capsule_session_accepted_with_valid_token() {
        let cap_mgr = make_capability_manager();
        let session = make_session(SessionType::Capsule);

        // Grant a token for this session's capsule ID
        let token = cap_mgr.grant(
            session.id.as_str(),
            ResourceId::new("localhost://Users/self/Pictures/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let token_b64 = token.to_base64().unwrap();

        let state = make_state(Some(cap_mgr));
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", token_b64.parse().unwrap());

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Read,
        )
        .await;
        assert!(result.is_ok(), "Valid token should be accepted");
    }

    #[tokio::test]
    async fn test_capsule_session_rejected_wrong_resource() {
        let cap_mgr = make_capability_manager();
        let session = make_session(SessionType::Capsule);

        // Grant for photos/*, but access documents/
        let token = cap_mgr.grant(
            session.id.as_str(),
            ResourceId::new("localhost://Users/self/Pictures/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let token_b64 = token.to_base64().unwrap();

        let state = make_state(Some(cap_mgr));
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", token_b64.parse().unwrap());

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Documents/secret.txt",
            Action::Read,
        )
        .await;
        assert!(matches!(result, Err(StorageApiError::PermissionDenied(_))));
    }

    #[tokio::test]
    async fn test_capsule_session_rejected_wrong_action() {
        let cap_mgr = make_capability_manager();
        let session = make_session(SessionType::Capsule);

        // Grant read, but try to write
        let token = cap_mgr.grant(
            session.id.as_str(),
            ResourceId::new("localhost://Users/self/Pictures/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let token_b64 = token.to_base64().unwrap();

        let state = make_state(Some(cap_mgr));
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", token_b64.parse().unwrap());

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Write,
        )
        .await;
        assert!(matches!(result, Err(StorageApiError::PermissionDenied(_))));
    }

    #[tokio::test]
    async fn test_no_capability_manager_denies_access() {
        let state = make_state(None);
        let session = make_session(SessionType::Capsule);
        let headers = HeaderMap::new();

        let result = enforce_capability(
            &state,
            &session,
            &headers,
            "Users/self/Pictures/a.jpg",
            Action::Read,
        )
        .await;
        assert!(
            matches!(result, Err(StorageApiError::PermissionDenied(_))),
            "No capability manager must deny access — no ambient authority"
        );
    }
}
