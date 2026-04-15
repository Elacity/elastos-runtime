use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use elastos_common::{CapsuleManifest, CapsuleType};

use super::gateway::{content_type, validate_file_path, GatewayState};

const DEV_CAPSULES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");
const BROWSER_CAPSULE_CACHE_CONTROL: &str = "no-store";

struct BrowserCapsule {
    root: PathBuf,
    manifest: CapsuleManifest,
}

pub async fn redirect_browser_capsule_root(AxumPath(app): AxumPath<String>) -> Redirect {
    Redirect::permanent(&format!("/apps/{app}/"))
}

pub async fn serve_browser_capsule_index(
    State(state): State<GatewayState>,
    AxumPath(app): AxumPath<String>,
) -> Response {
    serve_browser_capsule_path(&state.data_dir, &app, None).await
}

pub async fn serve_browser_capsule_asset(
    State(state): State<GatewayState>,
    AxumPath((app, path)): AxumPath<(String, String)>,
) -> Response {
    serve_browser_capsule_path(&state.data_dir, &app, Some(&path)).await
}

async fn serve_browser_capsule_path(
    data_dir: &Path,
    app: &str,
    requested_path: Option<&str>,
) -> Response {
    let capsule = match resolve_browser_capsule(data_dir, app) {
        Ok(capsule) => capsule,
        Err(status) => return (status, "Browser capsule not found").into_response(),
    };

    let relative_path = requested_path.unwrap_or(&capsule.manifest.entrypoint);
    if validate_file_path(relative_path).is_err() {
        return (StatusCode::BAD_REQUEST, "Invalid file path").into_response();
    }

    let asset_path = capsule.root.join(relative_path);
    let Ok(bytes) = tokio::fs::read(&asset_path).await else {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    };

    (
        StatusCode::OK,
        [
            ("content-type", content_type(relative_path)),
            ("cache-control", BROWSER_CAPSULE_CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response()
}

fn resolve_browser_capsule(data_dir: &Path, app: &str) -> Result<BrowserCapsule, StatusCode> {
    let candidates = [
        PathBuf::from(DEV_CAPSULES_ROOT).join(app),
        data_dir.join("capsules").join(app),
    ];

    for candidate in candidates {
        if let Some(capsule) = load_browser_capsule(&candidate, app) {
            return Ok(capsule);
        }
    }

    Err(StatusCode::NOT_FOUND)
}

fn load_browser_capsule(dir: &Path, expected_name: &str) -> Option<BrowserCapsule> {
    if !dir.is_dir() {
        return None;
    }

    let manifest_path = dir.join("capsule.json");
    let bytes = std::fs::read(&manifest_path).ok()?;
    let manifest: CapsuleManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.validate().is_err() {
        return None;
    }
    if manifest.name != expected_name {
        return None;
    }
    if manifest.capsule_type != CapsuleType::Data {
        return None;
    }
    if !manifest.entrypoint.ends_with(".html") {
        return None;
    }
    if !dir.join(&manifest.entrypoint).is_file() {
        return None;
    }

    Some(BrowserCapsule {
        root: dir.to_path_buf(),
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dev_browser_capsule() {
        let data_dir = tempfile::tempdir().unwrap();
        let capsule = resolve_browser_capsule(data_dir.path(), "gba-emulator").unwrap();
        assert_eq!(capsule.manifest.name, "gba-emulator");
        assert_eq!(capsule.manifest.entrypoint, "index.html");
    }
}
