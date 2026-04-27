use std::path::{Path as FsPath, PathBuf};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use elastos_common::localhost::rooted_localhost_fs_path;
use serde::Serialize;

use super::gateway::{
    content_type, require_home_launch_token, require_home_launch_token_for_any,
    viewer_object_shell_description, viewer_object_shell_title, GatewayState,
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
    if let Err(err) = require_viewer_library_launch_token(&state.data_dir, &headers, &viewer) {
        return viewer_error_response(err);
    }

    Json(ViewerLibraryResponse {
        items: super::browser_capsules::list_viewer_bound_capsules(&state.data_dir, &viewer)
            .into_iter()
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
        require_viewer_bound_launch_token(&state.data_dir, &headers, &viewer, &capsule)
    {
        return viewer_error_response(err);
    }

    let Some(capsule) =
        super::browser_capsules::resolve_viewer_bound_capsule(&state.data_dir, &capsule, &viewer)
    else {
        return (StatusCode::NOT_FOUND, "viewer content capsule not found").into_response();
    };
    let Some(capsule_dir) =
        super::browser_capsules::capsule_dir_candidates(&state.data_dir, &capsule.name)
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

pub async fn viewer_storage_get(
    State(state): State<GatewayState>,
    Path((viewer, capsule, scope, name)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let path =
        match viewer_storage_file(&state.data_dir, &headers, &viewer, &capsule, &scope, &name) {
            Ok(path) => path,
            Err(err) => return viewer_error_response(err),
        };
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type", "application/octet-stream"),
                ("cache-control", "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "viewer storage file not found").into_response()
        }
        Err(err) => viewer_error_response(err.into()),
    }
}

pub async fn viewer_storage_put(
    State(state): State<GatewayState>,
    Path((viewer, capsule, scope, name)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path =
        match viewer_storage_file(&state.data_dir, &headers, &viewer, &capsule, &scope, &name) {
            Ok(path) => path,
            Err(err) => return viewer_error_response(err),
        };
    if let Some(parent) = path.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return viewer_error_response(err.into());
        }
    }
    match tokio::fs::write(path, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => viewer_error_response(err.into()),
    }
}

fn require_viewer_library_launch_token(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
) -> anyhow::Result<()> {
    let mut allowed_apps = vec![viewer.to_string()];
    allowed_apps.extend(
        super::browser_capsules::list_viewer_bound_capsules(data_dir, viewer)
            .into_iter()
            .map(|capsule| capsule.name),
    );
    let allowed_app_refs = allowed_apps.iter().map(String::as_str).collect::<Vec<_>>();
    require_home_launch_token_for_any(data_dir, headers, &allowed_app_refs).map(|_| ())
}

fn require_viewer_bound_launch_token(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    capsule: &str,
) -> anyhow::Result<()> {
    require_home_launch_token(data_dir, headers, capsule)
        .or_else(|_| require_home_launch_token(data_dir, headers, viewer))
}

fn viewer_storage_file(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    capsule: &str,
    scope: &str,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let viewer = clean_capsule_ref(viewer, "viewer")?;
    let capsule = clean_capsule_ref(capsule, "capsule")?;
    if !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        anyhow::bail!("viewer capsule not found");
    }
    require_viewer_bound_launch_token(data_dir, headers, &viewer, &capsule)?;
    let root = viewer_storage_root(data_dir, &viewer, &capsule)?;
    let file_name = clean_storage_file_name(name)?;
    let dir = match scope {
        "save" => root,
        "state" => root.join("states"),
        _ => anyhow::bail!("invalid viewer storage scope"),
    };
    Ok(dir.join(file_name))
}

fn viewer_storage_root(data_dir: &FsPath, viewer: &str, capsule: &str) -> anyhow::Result<PathBuf> {
    let capsule = super::browser_capsules::resolve_viewer_bound_capsule(data_dir, capsule, viewer)
        .ok_or_else(|| anyhow::anyhow!("viewer content capsule not found"))?;
    let storage = capsule
        .storage
        .first()
        .ok_or_else(|| anyhow::anyhow!("viewer content capsule has no storage grant"))?;
    let root_uri = storage.trim_end_matches('*').trim_end_matches('/');
    rooted_localhost_fs_path(data_dir, root_uri)
        .ok_or_else(|| anyhow::anyhow!("invalid viewer storage root"))
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
