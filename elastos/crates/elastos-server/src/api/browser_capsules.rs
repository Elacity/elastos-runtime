use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType};

use super::capsule_inventory::{
    installed_active_capsule_dir, installed_capsules_root, list_active_capsule_manifests,
    load_capsule_manifest,
};
use super::gateway::{
    content_type, ensure_wallet_connector_configured, request_uses_tls, validate_file_path,
    GatewayState,
};

const BROWSER_CAPSULE_CACHE_CONTROL: &str = "no-store";
const BROWSER_CAPSULE_COOP: &str = "same-origin";
const BROWSER_CAPSULE_COEP: &str = "require-corp";
const BROWSER_CAPSULE_DOCUMENT_CORP: &str = "cross-origin";
const BROWSER_CAPSULE_ASSET_CORP: &str = "cross-origin";
const BROWSER_CAPSULE_OPAQUE_ORIGIN: &str = "null";

pub(super) fn is_allowed_capsule_origin(origin: &axum::http::HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if origin == BROWSER_CAPSULE_OPAQUE_ORIGIN {
        return true;
    }
    match url::Url::parse(origin) {
        Ok(url) => matches!(
            url.host_str(),
            Some("127.0.0.1") | Some("localhost") | Some("::1") | Some("[::1]")
        ),
        Err(_) => false,
    }
}

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

pub async fn serve_browser_app_root(AxumPath(app): AxumPath<String>) -> Response {
    Redirect::permanent(&format!("/apps/{app}/")).into_response()
}

pub async fn serve_browser_app_index(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
    AxumPath(app): AxumPath<String>,
) -> Response {
    serve_browser_capsule_path(&state.data_dir, &headers, &app, None).await
}

pub async fn serve_browser_app_asset(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
    AxumPath((app, path)): AxumPath<(String, String)>,
) -> Response {
    serve_browser_capsule_path(&state.data_dir, &headers, &app, Some(&path)).await
}

pub(super) fn canonical_browser_capsule_route(route: &str) -> Result<String, String> {
    let parsed = url::Url::parse(&format!("http://elastos.invalid{route}"))
        .map_err(|_| "Runtime returned an invalid capsule route".to_string())?;
    let mut segments = parsed
        .path_segments()
        .ok_or_else(|| "Runtime returned an invalid capsule route".to_string())?;
    if segments.next() != Some("apps") {
        return Err("Runtime returned a non-capsule browser route".to_string());
    }
    segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Runtime returned a capsule route without an app".to_string())?;
    Ok(parsed[url::Position::BeforePath..].to_string())
}

async fn serve_browser_capsule_path(
    data_dir: &Path,
    request_headers: &axum::http::HeaderMap,
    app: &str,
    requested_path: Option<&str>,
) -> Response {
    if ensure_wallet_connector_configured(data_dir, app).is_err() {
        return (StatusCode::NOT_FOUND, "Browser capsule not found").into_response();
    }

    let capsule = match resolve_browser_capsule(data_dir, app) {
        Ok(capsule) => capsule,
        Err(status) => return (status, "Browser capsule not found").into_response(),
    };

    let relative_path = requested_path.unwrap_or(&capsule.entrypoint);
    if validate_file_path(relative_path).is_err() {
        return (StatusCode::BAD_REQUEST, "Invalid file path").into_response();
    }

    let installed_root = match tokio::fs::canonicalize(installed_capsules_root(data_dir)).await {
        Ok(path) => path,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    let capsule_root = match tokio::fs::canonicalize(&capsule.root).await {
        Ok(path) if path.starts_with(&installed_root) => path,
        _ => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    let asset_path = match tokio::fs::canonicalize(capsule_root.join(relative_path)).await {
        Ok(path) if path.starts_with(&capsule_root) => path,
        _ => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    let Ok(bytes) = tokio::fs::read(&asset_path).await else {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    };
    let resource_policy =
        if relative_path == capsule.entrypoint && app != super::gateway::HOME_CAPSULE_ID {
            BROWSER_CAPSULE_DOCUMENT_CORP
        } else {
            BROWSER_CAPSULE_ASSET_CORP
        };

    let is_document = relative_path == capsule.entrypoint;
    let mut response = (
        StatusCode::OK,
        [
            ("content-type", content_type(relative_path)),
            ("cache-control", BROWSER_CAPSULE_CACHE_CONTROL),
            ("cross-origin-opener-policy", BROWSER_CAPSULE_COOP),
            ("cross-origin-embedder-policy", BROWSER_CAPSULE_COEP),
            ("cross-origin-resource-policy", resource_policy),
        ],
        bytes,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        BROWSER_CAPSULE_OPAQUE_ORIGIN.parse().unwrap(),
    );
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    if is_document {
        if let Some(policy) = shell_content_security_policy(request_headers, app) {
            headers.insert("content-security-policy", policy.parse().unwrap());
        }
    }
    response
}

fn shell_content_security_policy(headers: &axum::http::HeaderMap, app: &str) -> Option<String> {
    if app != super::gateway::HOME_CAPSULE_ID && !super::gateway::is_trusted_home_shell_id(app) {
        return None;
    }
    let home_source = home_document_origin(headers)?;
    let is_home_host = app == super::gateway::HOME_CAPSULE_ID;
    let frame_ancestors = if is_home_host {
        "'none'".to_string()
    } else {
        home_source.clone()
    };
    let (default_source, style_source) = if is_home_host {
        ("'self'".to_string(), "'self'".to_string())
    } else {
        (
            home_source.clone(),
            format!("{home_source} 'unsafe-inline'"),
        )
    };
    let connect_source = if app == super::gateway::HOME_CLI_SHELL_ID {
        format!("{default_source} {}", home_websocket_origin(&home_source)?)
    } else {
        default_source.clone()
    };
    Some(format!(
        "default-src {default_source}; script-src {default_source}; style-src {style_source}; img-src {default_source} blob: data:; connect-src {connect_source}; frame-src {default_source}; object-src 'none'; base-uri 'none'; form-action {default_source}; frame-ancestors {frame_ancestors}"
    ))
}

fn home_websocket_origin(home_source: &str) -> Option<String> {
    if let Some(authority) = home_source.strip_prefix("https://") {
        return Some(format!("wss://{authority}"));
    }
    home_source
        .strip_prefix("http://")
        .map(|authority| format!("ws://{authority}"))
}

fn home_document_origin(headers: &axum::http::HeaderMap) -> Option<String> {
    let authority = headers
        .get(axum::http::header::HOST)?
        .to_str()
        .ok()?
        .parse::<axum::http::uri::Authority>()
        .ok()?;
    Some(format!(
        "{}://{authority}",
        if request_uses_tls(headers) {
            "https"
        } else {
            "http"
        }
    ))
}

pub(crate) fn list_launchable_browser_capsules(data_dir: &Path) -> Vec<LaunchableBrowserCapsule> {
    let mut capsules = BTreeMap::new();
    for manifest in list_active_capsule_manifests(data_dir) {
        if !manifest.role.is_shell_launchable() {
            continue;
        }
        if ensure_wallet_connector_configured(data_dir, &manifest.name).is_err() {
            continue;
        }
        let Ok(capsule) = resolve_browser_capsule(data_dir, &manifest.name) else {
            continue;
        };
        capsules.insert(
            capsule.manifest.name.clone(),
            LaunchableBrowserCapsule {
                name: capsule.manifest.name,
                description: capsule.manifest.description,
                role: capsule.manifest.role,
            },
        );
    }

    capsules.into_values().collect()
}

pub(crate) fn list_viewer_bound_capsules(data_dir: &Path, viewer: &str) -> Vec<ViewerBoundCapsule> {
    let mut capsules = BTreeMap::new();
    let installed_root = installed_capsules_root(data_dir);
    for manifest in list_active_capsule_manifests(data_dir) {
        let dir = installed_root.join(&manifest.name);
        if manifest.role != CapsuleRole::Content
            || manifest.capsule_type != CapsuleType::Data
            || manifest.viewer.as_deref() != Some(viewer)
            || !dir.join(&manifest.entrypoint).is_file()
            || !is_launchable_viewer_capsule(data_dir, viewer)
        {
            continue;
        }
        capsules.insert(
            manifest.name.clone(),
            ViewerBoundCapsule {
                name: manifest.name,
                description: manifest.description,
                viewer: viewer.to_string(),
                entrypoint: manifest.entrypoint,
                storage: manifest.permissions.storage,
            },
        );
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
    let candidate = installed_active_capsule_dir(data_dir, name)?;
    let manifest = load_capsule_manifest(&candidate, name)?;
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

    None
}

pub(crate) fn is_viewer_capsule(data_dir: &Path, viewer: &str) -> bool {
    is_launchable_viewer_capsule(data_dir, viewer)
}

fn resolve_browser_capsule(data_dir: &Path, app: &str) -> Result<BrowserCapsule, StatusCode> {
    let candidate = installed_active_capsule_dir(data_dir, app).ok_or(StatusCode::NOT_FOUND)?;
    load_browser_capsule(&candidate, app).ok_or(StatusCode::NOT_FOUND)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::gateway::WALLET_WALLETCONNECT_CAPSULE_ID;
    use std::fs;

    fn test_request_headers() -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::HOST, "localhost:61180".parse().unwrap());
        headers
    }

    fn activate_test_capsule(data_dir: &Path, name: &str) {
        let path = data_dir.join("components.json");
        let mut components = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_else(|| {
                serde_json::json!({
                    "external": {},
                    "capsules": {},
                    "profiles": {}
                })
            });
        components["external"][name] = serde_json::json!({
            "install_path": format!("capsules/{name}"),
            "platforms": {}
        });
        fs::write(path, serde_json::to_vec_pretty(&components).unwrap()).unwrap();
    }

    fn write_test_browser_capsule(data_dir: &Path, name: &str, description: &str, role: &str) {
        activate_test_capsule(data_dir, name);
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
        activate_test_capsule(data_dir, name);
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
        activate_test_capsule(data_dir, name);
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

    #[test]
    fn source_only_capsules_are_not_apps_or_viewer_content() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_components_manifest(data_dir.path(), &["browser", "gba-emulator", "gba-ucity"]);

        assert!(resolve_browser_capsule(data_dir.path(), "browser").is_err());
        assert!(resolve_browser_capsule(data_dir.path(), "gba-emulator").is_err());
        assert!(
            resolve_viewer_bound_capsule(data_dir.path(), "gba-ucity", "gba-emulator").is_none()
        );
        assert!(list_launchable_browser_capsules(data_dir.path()).is_empty());
    }

    #[tokio::test]
    async fn browser_capsule_documents_allow_isolated_home_embedding() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "test-browser", "Browser test", "app");

        let response = serve_browser_capsule_path(
            data_dir.path(),
            &test_request_headers(),
            "test-browser",
            None,
        )
        .await;
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
            Some(BROWSER_CAPSULE_DOCUMENT_CORP)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn browser_capsule_assets_cannot_follow_symlinks_outside_capsule_root() {
        use std::os::unix::fs::symlink;

        let data_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), "host secret").unwrap();
        write_test_browser_capsule(data_dir.path(), "test-browser", "Browser test", "app");
        let index = data_dir.path().join("capsules/test-browser/index.html");
        fs::remove_file(&index).unwrap();
        symlink(outside.path(), &index).unwrap();

        let response = serve_browser_capsule_path(
            data_dir.path(),
            &test_request_headers(),
            "test-browser",
            None,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_shell_documents_allow_only_the_home_host_to_embed_them() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_wasm_browser_capsule(data_dir.path(), "home-gui", "Home GUI", "shell");
        write_test_wasm_browser_capsule(data_dir.path(), "home-cli", "Home CLI", "shell");

        for shell in ["home-gui", "home-cli"] {
            let response =
                serve_browser_capsule_path(data_dir.path(), &test_request_headers(), shell, None)
                    .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get("cross-origin-resource-policy")
                    .and_then(|value| value.to_str().ok()),
                Some(BROWSER_CAPSULE_DOCUMENT_CORP)
            );
            let csp = response
                .headers()
                .get("content-security-policy")
                .and_then(|value| value.to_str().ok())
                .unwrap();
            assert!(csp.contains("frame-src http://localhost:61180"));
            assert!(csp.contains("frame-ancestors http://localhost:61180"));
            assert!(csp.contains("script-src http://localhost:61180"));
            assert!(!csp.contains("script-src 'self'"));
            if shell == "home-cli" {
                assert!(csp.contains("connect-src http://localhost:61180 ws://localhost:61180"));
            } else {
                assert!(csp.contains("connect-src http://localhost:61180;"));
                assert!(!csp.contains("ws://localhost:61180"));
            }
        }
    }

    #[tokio::test]
    async fn home_cli_tls_document_allows_only_its_same_origin_input_socket() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_wasm_browser_capsule(data_dir.path(), "home-cli", "Home CLI", "shell");
        let mut headers = test_request_headers();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());

        let response =
            serve_browser_capsule_path(data_dir.path(), &headers, "home-cli", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let csp = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        assert!(csp.contains("connect-src https://localhost:61180 wss://localhost:61180"));
        assert!(!csp.contains("ws://localhost:61180"));
    }

    #[test]
    fn canonical_capsule_route_stays_on_the_home_origin() {
        assert_eq!(
            canonical_browser_capsule_route(
                "/apps/browser/settings/?view=privacy#home_token=secret"
            )
            .unwrap(),
            "/apps/browser/settings/?view=privacy#home_token=secret"
        );
        assert!(canonical_browser_capsule_route("https://example.test/apps/browser/").is_err());
    }

    #[tokio::test]
    async fn browser_capsule_assets_can_load_in_an_opaque_frame() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(data_dir.path(), "browser", "Browser", "app");
        std::fs::write(
            data_dir.path().join("capsules/browser/browser.js"),
            "console.log('browser');",
        )
        .unwrap();

        let response = serve_browser_capsule_path(
            data_dir.path(),
            &test_request_headers(),
            "browser",
            Some("browser.js"),
        )
        .await;
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
            Some(BROWSER_CAPSULE_ASSET_CORP)
        );
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some(BROWSER_CAPSULE_OPAQUE_ORIGIN)
        );
    }

    #[tokio::test]
    async fn walletconnect_browser_capsule_requires_pinned_runtime_config() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(
            data_dir.path(),
            WALLET_WALLETCONNECT_CAPSULE_ID,
            "WalletConnect",
            "app",
        );

        let response = serve_browser_capsule_path(
            data_dir.path(),
            &test_request_headers(),
            WALLET_WALLETCONNECT_CAPSULE_ID,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn unconfigured_walletconnect_browser_capsule_is_not_launchable() {
        let data_dir = tempfile::tempdir().unwrap();
        write_test_browser_capsule(
            data_dir.path(),
            WALLET_WALLETCONNECT_CAPSULE_ID,
            "WalletConnect",
            "app",
        );

        assert!(!list_launchable_browser_capsules(data_dir.path())
            .iter()
            .any(|capsule| capsule.name == WALLET_WALLETCONNECT_CAPSULE_ID));
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
        write_test_browser_capsule(data_dir.path(), "removed-capsule", "Removed Capsule", "app");
        write_test_components_manifest(data_dir.path(), &["system"]);

        let names: Vec<_> = list_launchable_browser_capsules(data_dir.path())
            .into_iter()
            .map(|capsule| capsule.name)
            .collect();
        assert!(names.contains(&"system".to_string()));
        assert!(!names.contains(&"removed-capsule".to_string()));
        assert!(resolve_browser_capsule(data_dir.path(), "removed-capsule").is_err());
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
