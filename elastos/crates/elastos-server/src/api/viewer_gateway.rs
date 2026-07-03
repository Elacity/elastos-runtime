use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_runtime::auth::RuntimeAuditEventV1;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::gateway::{
    content_type, require_home_launch_token_for_any, require_home_launch_token_for_any_context,
    viewer_object_shell_description, viewer_object_shell_title, GatewayState,
    HomeLaunchTokenContext,
};

#[derive(Debug, Serialize)]
struct ViewerLibraryResponse {
    items: Vec<ViewerLibraryItem>,
}

#[derive(Debug, Serialize)]
struct ViewerLibraryItem {
    capsule: String,
    title: String,
    description: String,
    entrypoint: String,
}

#[derive(Debug, Deserialize)]
pub struct ViewerLibraryObjectQuery {
    uri: String,
    #[serde(default)]
    stat_only: bool,
    #[serde(default)]
    entries: bool,
    #[serde(default)]
    preview_entry: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ViewerLibraryObjectWrite {
    data: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    if_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ViewerLibraryArchiveExtractEntries {
    destination_uri: String,
    entries: Vec<String>,
    #[serde(default)]
    conflict_policy: Option<String>,
    #[serde(default)]
    if_revision: Option<String>,
    #[serde(default)]
    cancel: bool,
}

struct ViewerLibraryObjectRequest {
    uri: String,
    stat_only: bool,
    entries: bool,
    preview_entry: Option<String>,
    write: Option<ViewerLibraryObjectWrite>,
}

pub async fn viewer_library_summary(
    State(state): State<GatewayState>,
    Path(viewer): Path<String>,
    headers: HeaderMap,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    if !super::browser_capsules::is_viewer_capsule(&state.data_dir, &viewer) {
        return (StatusCode::NOT_FOUND, "viewer capsule not found").into_response();
    }
    let token_app = match require_viewer_library_launch_token(&state.data_dir, &headers, &viewer) {
        Ok(token_app) => token_app,
        Err(err) => return viewer_error_response(err),
    };

    Json(ViewerLibraryResponse {
        items: super::browser_capsules::list_viewer_bound_capsules(&state.data_dir, &viewer)
            .into_iter()
            .filter(|capsule| token_app == viewer || capsule.name == token_app)
            .map(|capsule| ViewerLibraryItem {
                title: viewer_object_shell_title(&capsule.name, capsule.description.as_deref()),
                description: viewer_object_shell_description(
                    &capsule.viewer,
                    capsule.description.as_deref(),
                ),
                entrypoint: capsule.entrypoint,
                capsule: capsule.name,
            })
            .collect(),
    })
    .into_response()
}

pub async fn viewer_content(
    State(state): State<GatewayState>,
    Path((viewer, capsule)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    let capsule = match clean_capsule_ref(&capsule, "capsule") {
        Ok(capsule) => capsule,
        Err(err) => return viewer_error_response(err),
    };
    if !super::browser_capsules::is_viewer_capsule(&state.data_dir, &viewer) {
        return (StatusCode::NOT_FOUND, "viewer capsule not found").into_response();
    }
    if let Err(err) =
        require_viewer_bound_launch_token_context(&state.data_dir, &headers, &viewer, &capsule)
    {
        return viewer_error_response(err);
    }

    let Some(capsule) =
        super::browser_capsules::resolve_viewer_bound_capsule(&state.data_dir, &capsule, &viewer)
    else {
        return (StatusCode::NOT_FOUND, "viewer content capsule not found").into_response();
    };
    let Some(capsule_dir) =
        super::capsule_inventory::capsule_dir_candidates(&state.data_dir, &capsule.name)
            .into_iter()
            .find(|candidate| candidate.join(&capsule.entrypoint).is_file())
    else {
        return (StatusCode::NOT_FOUND, "viewer content file not found").into_response();
    };
    let asset_path = capsule_dir.join(&capsule.entrypoint);
    let Ok(bytes) = tokio::fs::read(&asset_path).await else {
        return (StatusCode::NOT_FOUND, "viewer content file not found").into_response();
    };
    (
        StatusCode::OK,
        [
            ("content-type", content_type(&capsule.entrypoint)),
            ("cache-control", "no-store"),
        ],
        bytes,
    )
        .into_response()
}

pub async fn viewer_library_object_get(
    State(state): State<GatewayState>,
    Path(viewer): Path<String>,
    Query(query): Query<ViewerLibraryObjectQuery>,
    headers: HeaderMap,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    let context = match require_library_object_viewer_context(&state.data_dir, &headers, &viewer) {
        Ok(context) => context,
        Err(err) => return viewer_error_response(err),
    };
    match viewer_library_object(
        &state,
        &context,
        &viewer,
        ViewerLibraryObjectRequest {
            uri: query.uri,
            stat_only: query.stat_only,
            entries: query.entries,
            preview_entry: query.preview_entry,
            write: None,
        },
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => viewer_error_response(err),
    }
}

pub async fn viewer_library_object_put(
    State(state): State<GatewayState>,
    Path(viewer): Path<String>,
    Query(query): Query<ViewerLibraryObjectQuery>,
    headers: HeaderMap,
    Json(input): Json<ViewerLibraryObjectWrite>,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    if viewer != "documents" {
        return (
            StatusCode::FORBIDDEN,
            "viewer does not support Library object writes",
        )
            .into_response();
    }
    let context = match require_documents_viewer_context(&state.data_dir, &headers, &viewer) {
        Ok(context) => context,
        Err(err) => return viewer_error_response(err),
    };
    match viewer_library_object(
        &state,
        &context,
        &viewer,
        ViewerLibraryObjectRequest {
            uri: query.uri,
            stat_only: false,
            entries: false,
            preview_entry: None,
            write: Some(input),
        },
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => viewer_error_response(err),
    }
}

pub async fn viewer_library_object_post(
    State(state): State<GatewayState>,
    Path(viewer): Path<String>,
    Query(query): Query<ViewerLibraryObjectQuery>,
    headers: HeaderMap,
    Json(input): Json<ViewerLibraryArchiveExtractEntries>,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    let context = match require_library_object_viewer_context(&state.data_dir, &headers, &viewer) {
        Ok(context) => context,
        Err(err) => return viewer_error_response(err),
    };
    match viewer_library_archive_extract_entries(&state, &context, &viewer, &query.uri, input).await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => viewer_error_response(err),
    }
}

pub async fn viewer_library_roots_get(
    State(state): State<GatewayState>,
    Path(viewer): Path<String>,
    headers: HeaderMap,
) -> Response {
    let viewer = match clean_capsule_ref(&viewer, "viewer") {
        Ok(viewer) => viewer,
        Err(err) => return viewer_error_response(err),
    };
    if viewer != "archive-manager" {
        return (
            StatusCode::FORBIDDEN,
            "viewer does not support Library destination roots",
        )
            .into_response();
    }
    let context = match require_library_object_viewer_context(&state.data_dir, &headers, &viewer) {
        Ok(context) => context,
        Err(err) => return viewer_error_response(err),
    };
    match viewer_object_provider_request(&state, &context, "roots", json!({})).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => viewer_error_response(err),
    }
}

pub async fn viewer_storage_get(
    State(state): State<GatewayState>,
    Path((viewer, capsule, scope, name)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let target =
        match viewer_storage_target(&state.data_dir, &headers, &viewer, &capsule, &scope, &name) {
            Ok(target) => target,
            Err(err) => return viewer_error_response(err),
        };
    if !target.path.is_file() {
        return (StatusCode::NOT_FOUND, "viewer storage file not found").into_response();
    }
    match crate::auth::read_principal_root_object(
        &state.data_dir,
        &target.principal_id,
        &target.localhost_root,
        &target.object_uri,
        &target.path,
    ) {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type", "application/octet-stream"),
                ("cache-control", "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => viewer_error_response(err),
    }
}

pub async fn viewer_storage_put(
    State(state): State<GatewayState>,
    Path((viewer, capsule, scope, name)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target =
        match viewer_storage_target(&state.data_dir, &headers, &viewer, &capsule, &scope, &name) {
            Ok(target) => target,
            Err(err) => return viewer_error_response(err),
        };
    match crate::auth::write_principal_root_object(
        &state.data_dir,
        &target.principal_id,
        &target.localhost_root,
        &target.object_uri,
        &target.path,
        body.as_ref(),
    ) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => viewer_error_response(err),
    }
}

async fn viewer_library_object(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    viewer: &str,
    request: ViewerLibraryObjectRequest,
) -> anyhow::Result<Value> {
    let stat =
        viewer_object_provider_request(state, context, "stat", json!({ "uri": &request.uri }))
            .await?;
    ensure_viewer_can_view_library_object(&stat, viewer)?;
    if request.stat_only {
        return Ok(stat);
    }
    if request.entries {
        if viewer != "archive-manager" {
            anyhow::bail!("viewer does not support Library archive entry listing");
        }
        return viewer_object_provider_request(
            state,
            context,
            "archive_entries",
            json!({ "uri": &request.uri }),
        )
        .await;
    }
    if let Some(entry) = request.preview_entry {
        if viewer != "archive-manager" {
            anyhow::bail!("viewer does not support Library archive entry preview");
        }
        return viewer_object_provider_request(
            state,
            context,
            "archive_preview_entry",
            json!({ "uri": &request.uri, "entry": entry }),
        )
        .await;
    }
    if viewer != "documents" {
        anyhow::bail!("viewer supports Library object metadata only");
    }
    let payload = match request.write {
        Some(write) => json!({
            "uri": &request.uri,
            "data": write.data,
            "mime": write.mime,
            "if_revision": write.if_revision,
        }),
        None => json!({ "uri": &request.uri }),
    };
    let op = if payload.get("data").is_some() {
        "write"
    } else {
        "read"
    };
    viewer_object_provider_request(state, context, op, payload).await
}

pub(crate) async fn viewer_object_provider_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    op: &str,
    mut request: Value,
) -> anyhow::Result<Value> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("object provider unavailable"))?;
    request["op"] = Value::String(op.to_string());
    request["principal_id"] = Value::String(context.principal_id.clone());
    let request_id = format!("viewer-library:{op}:{}", crate::auth::now_ts());
    append_viewer_library_audit(
        &state.data_dir,
        context,
        &request_id,
        "library.viewer.requested",
        "requested",
        &format!("Viewer requested Library object operation {op}"),
    )?;
    let response = crate::library::handle_object_provider_runtime_request(
        &state.data_dir,
        Arc::clone(registry),
        &request,
    )
    .await;
    let completed = response.get("status").and_then(Value::as_str) == Some("ok");
    if completed && op == "write" {
        crate::library::library_event_notifier().notify_waiters();
    }
    append_viewer_library_audit(
        &state.data_dir,
        context,
        &request_id,
        if completed {
            "library.viewer.completed"
        } else {
            "library.viewer.failed"
        },
        if completed { "completed" } else { "failed" },
        &format!(
            "Viewer {} Library object operation {op}",
            if completed { "completed" } else { "failed" }
        ),
    )?;
    Ok(response)
}

async fn viewer_library_archive_extract_entries(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    viewer: &str,
    uri: &str,
    input: ViewerLibraryArchiveExtractEntries,
) -> anyhow::Result<Value> {
    if viewer != "archive-manager" {
        anyhow::bail!("viewer does not support Library archive extraction");
    }
    let stat =
        viewer_object_provider_request(state, context, "stat", json!({ "uri": uri })).await?;
    ensure_viewer_can_view_library_object(&stat, viewer)?;
    let response = viewer_object_provider_request(
        state,
        context,
        "archive_extract_entries",
        json!({
            "uri": uri,
            "destination_uri": input.destination_uri,
            "entries": input.entries,
            "conflict_policy": input.conflict_policy,
            "if_revision": input.if_revision,
            "cancel": input.cancel,
        }),
    )
    .await?;
    if response.get("status").and_then(Value::as_str) == Some("ok") {
        crate::library::library_event_notifier().notify_waiters();
    }
    Ok(response)
}

fn ensure_viewer_can_view_library_object(response: &Value, viewer_id: &str) -> anyhow::Result<()> {
    let object = response
        .get("data")
        .and_then(|data| data.get("object"))
        .ok_or_else(|| anyhow::anyhow!("library object not found"))?;
    let can_view = object
        .get("viewers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|viewer| viewer.get("id").and_then(Value::as_str) == Some(viewer_id));
    if !can_view {
        anyhow::bail!("Library object is not viewable by {viewer_id}");
    }
    Ok(())
}

fn require_library_object_viewer_context(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let viewer = clean_capsule_ref(viewer, "viewer")?;
    if !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        anyhow::bail!("viewer capsule not found");
    }
    require_home_launch_token_for_any_context(data_dir, headers, &[viewer.as_str()])
}

fn require_documents_viewer_context(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let viewer = clean_capsule_ref(viewer, "viewer")?;
    if viewer != "documents" || !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        anyhow::bail!("viewer capsule not found");
    }
    require_home_launch_token_for_any_context(data_dir, headers, &[viewer.as_str()])
}

fn append_viewer_library_audit(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    request_id: &str,
    event_type: &str,
    result: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let now = crate::auth::now_ts();
    crate::auth::append_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!("audit:{event_type}:{request_id}:{now}"),
            event_type: event_type.to_string(),
            principal_id: Some(context.principal_id.clone()),
            proof_binding_id: context.proof_binding_id.clone(),
            session_id: Some(context.session_id.clone()),
            challenge_id: Some(request_id.to_string()),
            capsule_id: Some("documents".to_string()),
            result: result.to_string(),
            reason: reason.to_string(),
            occurred_at: now,
            signer_did: None,
            signature: None,
        },
    )
}

struct ViewerStorageTarget {
    path: PathBuf,
    principal_id: String,
    localhost_root: String,
    object_uri: String,
}

fn require_viewer_library_launch_token(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
) -> anyhow::Result<String> {
    let mut allowed_apps = vec![viewer.to_string()];
    allowed_apps.extend(
        super::browser_capsules::list_viewer_bound_capsules(data_dir, viewer)
            .into_iter()
            .map(|capsule| capsule.name),
    );
    let allowed_app_refs = allowed_apps.iter().map(String::as_str).collect::<Vec<_>>();
    require_home_launch_token_for_any(data_dir, headers, &allowed_app_refs)
}

fn require_viewer_bound_launch_token_context(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    capsule: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    super::gateway::require_home_launch_token_for_any_context(data_dir, headers, &[capsule])
        .or_else(|_| {
            super::gateway::require_home_launch_token_for_any_context(data_dir, headers, &[viewer])
        })
}

fn viewer_storage_target(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    capsule: &str,
    scope: &str,
    name: &str,
) -> anyhow::Result<ViewerStorageTarget> {
    let viewer = clean_capsule_ref(viewer, "viewer")?;
    let capsule = clean_capsule_ref(capsule, "capsule")?;
    if !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        anyhow::bail!("viewer capsule not found");
    }
    let context = require_viewer_bound_launch_token_context(data_dir, headers, &viewer, &capsule)?;
    let root_uri = viewer_storage_root_uri(data_dir, &context, &viewer, &capsule)?;
    let root = rooted_localhost_fs_path(data_dir, &root_uri)
        .ok_or_else(|| anyhow::anyhow!("invalid viewer storage root"))?;
    let file_name = clean_storage_file_name(name)?;
    let (dir, object_uri) = match scope {
        "save" => (root, format!("{root_uri}/{file_name}")),
        "state" => (
            root.join("states"),
            format!("{root_uri}/states/{file_name}"),
        ),
        _ => anyhow::bail!("invalid viewer storage scope"),
    };
    let principal_id = context.principal_id;
    let localhost_root = crate::auth::principal_localhost_root(&principal_id);
    Ok(ViewerStorageTarget {
        path: dir.join(file_name),
        principal_id,
        localhost_root,
        object_uri,
    })
}

fn viewer_storage_root_uri(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    viewer: &str,
    capsule: &str,
) -> anyhow::Result<String> {
    let capsule = super::browser_capsules::resolve_viewer_bound_capsule(data_dir, capsule, viewer)
        .ok_or_else(|| anyhow::anyhow!("viewer content capsule not found"))?;
    let storage = capsule
        .storage
        .first()
        .ok_or_else(|| anyhow::anyhow!("viewer content capsule has no storage grant"))?;
    let root_uri = principal_scoped_storage_uri(storage, context);
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    if root_uri != localhost_root
        && !root_uri
            .strip_prefix(&localhost_root)
            .is_some_and(|rest| rest.starts_with('/'))
    {
        anyhow::bail!("viewer storage must be scoped to the launch principal root");
    }
    Ok(root_uri)
}

fn principal_scoped_storage_uri(storage: &str, context: &HomeLaunchTokenContext) -> String {
    let root_uri = storage.trim_end_matches('*').trim_end_matches('/');
    let principal_root = crate::auth::principal_localhost_root(&context.principal_id);
    if root_uri == "localhost://Users/self" {
        return principal_root;
    }
    if let Some(rest) = root_uri.strip_prefix("localhost://Users/self/") {
        return format!("{principal_root}/{rest}");
    }
    root_uri.to_string()
}

fn clean_capsule_ref(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    if value.contains('/') || value.contains('\\') || value == "." || value == ".." {
        anyhow::bail!("invalid {label}");
    }
    Ok(value.to_string())
}

fn clean_storage_file_name(name: &str) -> anyhow::Result<String> {
    let file_name = name.trim();
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name == "."
        || file_name == ".."
    {
        anyhow::bail!("invalid viewer storage file name");
    }
    Ok(file_name.to_string())
}

fn viewer_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("home launch token") {
        StatusCode::UNAUTHORIZED
    } else if text.contains("does not support")
        || text.contains("not viewable")
        || text.contains("metadata only")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("invalid")
        || text.contains("must not be empty")
        || text.contains("no storage grant")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}
