use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::{header::SET_COOKIE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType};

use super::gateway::{content_type, validate_file_path, GatewayState};

const DEV_CAPSULES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");
const BROWSER_CAPSULE_CACHE_CONTROL: &str = "no-store";
const BROWSER_CAPSULE_COOP: &str = "same-origin";
const BROWSER_CAPSULE_COEP: &str = "require-corp";
const BROWSER_CAPSULE_CORP: &str = "same-origin";
const BROWSER_CAPSULE_OAC: &str = "?1";

struct BrowserCapsule {
    root: PathBuf,
    manifest: CapsuleManifest,
    entrypoint: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchableBrowserCapsule {
    pub name: String,
    pub description: Option<String>,
    pub role: CapsuleRole,
}

#[derive(Clone, Debug)]
pub(crate) struct ViewerBoundCapsule {
    pub name: String,
    pub description: Option<String>,
    pub viewer: String,
    pub entrypoint: String,
    pub storage: Vec<String>,
}

pub(crate) fn capsule_dir_candidates(data_dir: &Path, app: &str) -> [PathBuf; 2] {
    [
        data_dir.join("capsules").join(app),
        PathBuf::from(DEV_CAPSULES_ROOT).join(app),
    ]
}

fn browser_capsule_roots(data_dir: &Path) -> [PathBuf; 2] {
    [data_dir.join("capsules"), PathBuf::from(DEV_CAPSULES_ROOT)]
}

pub async fn serve_browser_app_root(AxumPath(app): AxumPath<String>) -> Redirect {
    Redirect::permanent(&format!("/apps/{app}/"))
}

pub async fn serve_browser_app_index(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(app): AxumPath<String>,
) -> Response {
    let mut response = serve_browser_capsule_path(&state.data_dir, &app, None).await;
    if app == super::gateway::HOME_CAPSULE_ID && response.status().is_success() {
        match super::gateway::home_session_cookie_header(
            &state.data_dir,
            super::gateway::request_uses_tls(&headers),
        ) {
            Ok(cookie) => {
                response.headers_mut().append(SET_COOKIE, cookie);
            }
            Err(err) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
            }
        }
    }
    response
}

pub async fn serve_browser_app_asset(
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

    let relative_path = requested_path.unwrap_or(&capsule.entrypoint);
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
            ("cross-origin-opener-policy", BROWSER_CAPSULE_COOP),
            ("cross-origin-embedder-policy", BROWSER_CAPSULE_COEP),
            ("cross-origin-resource-policy", BROWSER_CAPSULE_CORP),
            ("origin-agent-cluster", BROWSER_CAPSULE_OAC),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) fn list_launchable_browser_capsules(data_dir: &Path) -> Vec<LaunchableBrowserCapsule> {
    let mut capsules = BTreeMap::new();
    let active_components = active_component_names(data_dir);
    for root in browser_capsule_roots(data_dir) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if installed_capsule_is_inactive(data_dir, &dir, name, active_components.as_ref()) {
                continue;
            }
            let Ok(capsule) = resolve_browser_capsule(data_dir, name) else {
                continue;
            };
            if !capsule.manifest.role.is_shell_launchable() {
                continue;
            }
            capsules
                .entry(capsule.manifest.name.clone())
                .or_insert(LaunchableBrowserCapsule {
                    name: capsule.manifest.name,
                    description: capsule.manifest.description,
                    role: capsule.manifest.role,
                });
        }
    }

    capsules.into_values().collect()
}

pub(crate) fn list_viewer_bound_capsules(data_dir: &Path, viewer: &str) -> Vec<ViewerBoundCapsule> {
    let mut capsules = BTreeMap::new();
    let active_components = active_component_names(data_dir);
    for root in browser_capsule_roots(data_dir) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if installed_capsule_is_inactive(data_dir, &dir, name, active_components.as_ref()) {
                continue;
            }
            let Some(manifest) = load_capsule_manifest(&dir, name) else {
                continue;
            };
            if manifest.role != CapsuleRole::Content
                || manifest.capsule_type != CapsuleType::Data
                || manifest.viewer.as_deref() != Some(viewer)
                || !dir.join(&manifest.entrypoint).is_file()
                || !is_launchable_viewer_capsule(data_dir, viewer)
            {
                continue;
            }
            capsules
                .entry(manifest.name.clone())
                .or_insert(ViewerBoundCapsule {
                    name: manifest.name,
                    description: manifest.description,
                    viewer: viewer.to_string(),
                    entrypoint: manifest.entrypoint,
                    storage: manifest.permissions.storage,
                });
        }
    }

    capsules.into_values().collect()
}

pub(crate) fn list_all_viewer_bound_capsules(data_dir: &Path) -> Vec<ViewerBoundCapsule> {
    let mut capsules = BTreeMap::new();
    for viewer in list_launchable_browser_capsules(data_dir)
        .into_iter()
        .filter(|capsule| capsule.role == CapsuleRole::Viewer)
        .map(|capsule| capsule.name)
    {
        for capsule in list_viewer_bound_capsules(data_dir, &viewer) {
            capsules
                .entry((capsule.viewer.clone(), capsule.name.clone()))
                .or_insert(capsule);
        }
    }

    capsules.into_values().collect()
}

pub(crate) fn resolve_viewer_bound_capsule(
    data_dir: &Path,
    name: &str,
    viewer: &str,
) -> Option<ViewerBoundCapsule> {
    for candidate in capsule_dir_candidates(data_dir, name) {
        let Some(manifest) = load_capsule_manifest(&candidate, name) else {
            continue;
        };
        if manifest.role == CapsuleRole::Content
            && manifest.capsule_type == CapsuleType::Data
            && manifest.viewer.as_deref() == Some(viewer)
            && candidate.join(&manifest.entrypoint).is_file()
            && is_launchable_viewer_capsule(data_dir, viewer)
        {
            return Some(ViewerBoundCapsule {
                name: manifest.name,
                description: manifest.description,
                viewer: viewer.to_string(),
                entrypoint: manifest.entrypoint,
                storage: manifest.permissions.storage,
            });
        }
    }

    None
}

pub(crate) fn is_viewer_capsule(data_dir: &Path, viewer: &str) -> bool {
    is_launchable_viewer_capsule(data_dir, viewer)
}

fn resolve_browser_capsule(data_dir: &Path, app: &str) -> Result<BrowserCapsule, StatusCode> {
    let active_components = active_component_names(data_dir);
    for candidate in capsule_dir_candidates(data_dir, app) {
        if installed_capsule_is_inactive(data_dir, &candidate, app, active_components.as_ref()) {
            continue;
        }
        if let Some(capsule) = load_browser_capsule(&candidate, app) {
            return Ok(capsule);
        }
    }

    Err(StatusCode::NOT_FOUND)
}

fn active_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let bytes = std::fs::read(data_dir.join("components.json")).ok()?;
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(&bytes).ok()?;
    let mut names: BTreeSet<String> = manifest.external.keys().cloned().collect();
    names.extend(manifest.capsules.keys().cloned());
    Some(names)
}

fn installed_capsule_is_inactive(
    data_dir: &Path,
    dir: &Path,
    name: &str,
    active_components: Option<&BTreeSet<String>>,
) -> bool {
    dir == data_dir.join("capsules").join(name)
        && active_components.is_some_and(|components| !components.contains(name))
}

fn is_launchable_viewer_capsule(data_dir: &Path, viewer: &str) -> bool {
    matches!(
        resolve_browser_capsule(data_dir, viewer),
        Ok(capsule) if capsule.manifest.role == CapsuleRole::Viewer
    )
}

fn load_browser_capsule(dir: &Path, expected_name: &str) -> Option<BrowserCapsule> {
    let manifest = load_capsule_manifest(dir, expected_name)?;

    if manifest.capsule_type == CapsuleType::Data
        && manifest.entrypoint.ends_with(".html")
        && dir.join(&manifest.entrypoint).is_file()
    {
        return Some(BrowserCapsule {
            root: dir.to_path_buf(),
            entrypoint: manifest.entrypoint.clone(),
            manifest,
        });
    }

    let browser_root = dir.join("browser");
    let browser_entrypoint = browser_root.join("index.html");
    if browser_entrypoint.is_file() {
        return Some(BrowserCapsule {
            root: browser_root,
            entrypoint: "index.html".to_string(),
            manifest,
        });
    }

    None
}

fn load_capsule_manifest(dir: &Path, expected_name: &str) -> Option<CapsuleManifest> {
    if !dir.is_dir() {
        return None;
    }

    let manifest_path = dir.join("capsule.json");
    let bytes = std::fs::read(&manifest_path).ok()?;
    let manifest: CapsuleManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.validate().is_err() || manifest.name != expected_name {
        return None;
    }
    Some(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_test_browser_capsule(data_dir: &Path, name: &str, description: &str, role: &str) {
        let capsule_dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&capsule_dir).unwrap();
        fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": description,
                "author": "elastos",
                "role": role,
                "type": "data",
                "entrypoint": "index.html"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(capsule_dir.join("index.html"), "<!doctype html>").unwrap();
    }

    fn write_test_wasm_browser_capsule(data_dir: &Path, name: &str, description: &str, role: &str) {
        let capsule_dir = data_dir.join("capsules").join(name);
        let browser_dir = capsule_dir.join("browser");
        fs::create_dir_all(&browser_dir).unwrap();
        fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": description,
                "author": "elastos",
                "role": role,
                "type": "wasm",
                "entrypoint": format!("{name}.wasm")
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(browser_dir.join("index.html"), "<!doctype html>").unwrap();
    }

    fn write_test_viewer_capsule(
        data_dir: &Path,
        name: &str,
        viewer: &str,
        entrypoint: &str,
        description: &str,
    ) {
        let capsule_dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&capsule_dir).unwrap();
        fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": description,
                "author": "elastos",
                "role": "content",
                "type": "data",
                "entrypoint": entrypoint,
                "viewer": viewer,
                "permissions": {
                    "storage": ["localhost://Users/self/.AppData/LocalHost/GBA/test/*"]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(capsule_dir.join(entrypoint), "rom-data").unwrap();
    }

    fn write_test_components_manifest(data_dir: &Path, names: &[&str]) {
        let external = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    serde_json::json!({
                        "install_path": format!("capsules/{name}"),
                        "platforms": {
                            "*": {
                                "release_path": format!("{name}.tar.gz"),
                                "extract_path": name,
                                "install_path": format!("capsules/{name}")
                            }
                        }
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.components/v1",
                "capsules": {},
                "external": external,
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn resolves_installed_data_browser_capsule() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "data-viewer", "Data viewer", "viewer");

        let capsule = resolve_browser_capsule(data_dir.path(), "data-viewer").unwrap();
        assert_eq!(capsule.manifest.name, "data-viewer");
        assert_eq!(capsule.entrypoint, "index.html");
    }

    #[test]
    fn resolves_browser_surface_for_non_data_capsule() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_wasm_browser_capsule(data_dir.path(), "test-home", "Home", "app");

        let capsule = resolve_browser_capsule(data_dir.path(), "test-home").unwrap();
        assert_eq!(capsule.manifest.name, "test-home");
        assert_eq!(capsule.entrypoint, "index.html");
        assert!(capsule.root.ends_with("capsules/test-home/browser"));
    }

    #[test]
    fn resolves_installed_browser_capsule_before_dev_tree_copy() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(
            data_dir.path(),
            "gba-emulator",
            "Installed browser copy",
            "viewer",
        );

        let capsule = resolve_browser_capsule(data_dir.path(), "gba-emulator").unwrap();
        assert_eq!(
            capsule.root,
            data_dir.path().join("capsules").join("gba-emulator")
        );
        assert_eq!(
            capsule.manifest.description.as_deref(),
            Some("Installed browser copy")
        );
    }

    #[tokio::test]
    async fn browser_capsule_assets_include_cross_origin_isolation_headers() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "test-browser", "Browser test", "app");

        let response = serve_browser_capsule_path(data_dir.path(), "test-browser", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers
                .get("cross-origin-opener-policy")
                .and_then(|value| value.to_str().ok()),
            Some(BROWSER_CAPSULE_COOP)
        );
        assert_eq!(
            headers
                .get("cross-origin-embedder-policy")
                .and_then(|value| value.to_str().ok()),
            Some(BROWSER_CAPSULE_COEP)
        );
        assert_eq!(
            headers
                .get("cross-origin-resource-policy")
                .and_then(|value| value.to_str().ok()),
            Some(BROWSER_CAPSULE_CORP)
        );
        assert_eq!(
            headers
                .get("origin-agent-cluster")
                .and_then(|value| value.to_str().ok()),
            Some(BROWSER_CAPSULE_OAC)
        );
    }

    #[test]
    fn list_launchable_browser_capsules_prefers_installed_metadata() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(
            data_dir.path(),
            "gba-emulator",
            "Installed browser copy",
            "viewer",
        );

        let capsules = list_launchable_browser_capsules(data_dir.path());
        let gba = capsules
            .into_iter()
            .find(|capsule| capsule.name == "gba-emulator")
            .expect("gba-emulator to be listed");
        assert_eq!(gba.description.as_deref(), Some("Installed browser copy"));
        assert_eq!(gba.role, CapsuleRole::Viewer);
    }

    #[test]
    fn list_launchable_browser_capsules_hides_installed_capsules_missing_from_registry() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "system", "System", "app");
        write_test_browser_capsule(data_dir.path(), "elastos-manager", "Elastos Manager", "app");
        write_test_components_manifest(data_dir.path(), &["system"]);

        let names: Vec<_> = list_launchable_browser_capsules(data_dir.path())
            .into_iter()
            .map(|capsule| capsule.name)
            .collect();
        assert!(names.contains(&"system".to_string()));
        assert!(!names.contains(&"elastos-manager".to_string()));
        assert!(resolve_browser_capsule(data_dir.path(), "elastos-manager").is_err());
    }

    #[test]
    fn list_viewer_bound_capsules_prefers_installed_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(
            data_dir.path(),
            "gba-emulator",
            "Installed browser copy",
            "viewer",
        );
        write_test_viewer_capsule(
            data_dir.path(),
            "gba-ucity",
            "gba-emulator",
            "override.gba",
            "Demo ROM - test cartridge",
        );

        let capsules = list_viewer_bound_capsules(data_dir.path(), "gba-emulator");
        let capsule = capsules
            .into_iter()
            .find(|capsule| capsule.name == "gba-ucity")
            .expect("gba-ucity to be listed");
        assert_eq!(capsule.viewer, "gba-emulator");
        assert_eq!(capsule.entrypoint, "override.gba");
        assert_eq!(
            capsule.description.as_deref(),
            Some("Demo ROM - test cartridge")
        );
        assert_eq!(capsule.storage.len(), 1);
    }

    #[test]
    fn launchable_browser_capsules_exclude_provider_and_content_roles() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "viewer-surface", "Viewer", "viewer");
        write_test_browser_capsule(data_dir.path(), "provider-surface", "Provider", "provider");
        write_test_browser_capsule(data_dir.path(), "content-surface", "Content", "content");

        let capsules = list_launchable_browser_capsules(data_dir.path());
        let names: Vec<_> = capsules.into_iter().map(|capsule| capsule.name).collect();
        assert!(names.contains(&"viewer-surface".to_string()));
        assert!(!names.contains(&"provider-surface".to_string()));
        assert!(!names.contains(&"content-surface".to_string()));
    }
}
