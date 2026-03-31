//! Public ElastOS edge for site roots, publisher objects, and CID content.
//!
//! Owns the browser-facing HTTP application routes and resolves them from
//! runtime-owned state (`MyWebSite`, `ElastOS/SystemServices/Publisher`, and
//! `ElastOS/SystemServices/Edge`) plus read-only CID content.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine as _;
use elastos_common::localhost::{
    edge_binding_path, edge_site_head_path, publisher_artifacts_path,
    publisher_install_script_path, publisher_release_head_path, publisher_release_manifest_path,
    rooted_localhost_fs_path, MY_WEBSITE_URI,
};
use elastos_runtime::provider::ProviderRegistry;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Maximum size for a single file fetched through the gateway (100 MB).
const MAX_GATEWAY_FILE_SIZE: usize = 100 * 1024 * 1024;
const GATEWAY_VERSION: &str = env!("ELASTOS_VERSION");

#[derive(Clone)]
pub struct GatewayState {
    pub ipfs_provider_binary: Option<PathBuf>,
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub cache_dir: PathBuf,
    /// Runtime data directory backing rooted Publisher/Edge/MyWebSite state.
    pub data_dir: PathBuf,
    /// Cached ipfs-provider bridge (lazily initialized, reused across fetches).
    pub ipfs_bridge: Arc<Mutex<Option<Arc<elastos_runtime::provider::ProviderBridge>>>>,
}

pub fn gateway_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(serve_public_root))
        .route("/healthz", get(healthz))
        .route("/release.json", get(serve_release_manifest))
        .route("/release-head.json", get(serve_release_head))
        .route("/install.sh", get(serve_install_script))
        .route(
            "/.well-known/elastos/site-head.json",
            get(serve_site_head_document),
        )
        .route("/artifacts/*path", get(serve_artifact_file))
        .route("/s/:cid", get(redirect_cid_root))
        .route("/s/:cid/", get(serve_cid_root))
        .route("/s/:cid/*path", get(serve_cid_file))
        // IPFS-compatible paths so install.sh can use this gateway like ipfs.io
        .route("/ipfs/:cid", get(serve_ipfs_cid_root))
        .route("/ipfs/:cid/", get(serve_cid_root))
        .route("/ipfs/:cid/*path", get(serve_cid_file))
        .route("/*path", get(serve_public_site_path))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn landing_page() -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>ElastOS Gateway</title>
  <style>
    :root {{
      --bg: #0c1217;
      --panel: #111a21;
      --text: #e9f0f4;
      --muted: #9bb1bf;
      --accent: #29b6f6;
      --ok: #54d66d;
      --border: #203241;
    }}
    body {{
      margin: 0;
      font-family: "Segoe UI", "SF Pro Text", system-ui, sans-serif;
      background: radial-gradient(circle at 20% 0%, #122130 0%, var(--bg) 45%);
      color: var(--text);
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 1.25rem;
    }}
    main {{
      width: min(820px, 100%);
      background: linear-gradient(180deg, #111a21 0%, #0f171f 100%);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 1.25rem;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.35);
    }}
    h1 {{
      margin: 0 0 0.25rem;
      font-size: 1.5rem;
    }}
    .muted {{
      color: var(--muted);
      margin: 0.125rem 0 0.75rem;
    }}
    .version {{
      font-size: 0.875rem;
      color: var(--ok);
      margin-bottom: 1rem;
    }}
    form {{
      display: flex;
      gap: 0.5rem;
      margin: 1rem 0 0.75rem;
      flex-wrap: wrap;
    }}
    input[type="text"] {{
      flex: 1 1 420px;
      min-width: 220px;
      background: #0d141b;
      color: var(--text);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 0.7rem 0.8rem;
      font-size: 0.95rem;
    }}
    button {{
      background: var(--accent);
      color: #05131d;
      border: 0;
      border-radius: 10px;
      padding: 0.7rem 1rem;
      font-weight: 700;
      cursor: pointer;
    }}
    code {{
      background: #0c141b;
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 0.1rem 0.35rem;
    }}
    ul {{
      margin-top: 0.5rem;
      color: var(--muted);
    }}
    a {{
      color: var(--accent);
    }}
  </style>
</head>
<body>
  <main>
    <h1>ElastOS Gateway</h1>
    <p class="muted">Public ElastOS edge for MyWebSite, publisher objects, and read-only CID content.</p>
    <p class="version">Version {}</p>

    <form id="cid-form">
      <input id="cid-input" type="text" placeholder="Paste CID, elastos://CID, or gateway URL" autocomplete="off" />
      <button type="submit">Open</button>
    </form>

    <p class="muted">URL format: <code>/s/&lt;cid&gt;/</code></p>
    <ul>
      <li>Health check: <a href="/healthz">/healthz</a></li>
      <li>Site root: <code>/</code> from <code>localhost://MyWebSite</code> or a bound Edge target.</li>
      <li>Publisher objects: <code>/release-head.json</code>, <code>/release.json</code>, <code>/install.sh</code>, <code>/artifacts/...</code></li>
      <li>Content example: <code>/s/bafy.../</code></li>
    </ul>
  </main>

  <script>
    (function () {{
      function extractCid(input) {{
        var s = (input || "").trim().replace(/\/+$/, "");
        if (!s) return "";
        if (s.startsWith("elastos://")) return s.slice("elastos://".length).split("/")[0];
        var m1 = s.match(/\/ipfs\/([^\/?#]+)/);
        if (m1 && m1[1]) return m1[1];
        var m2 = s.match(/^https?:\/\/([^./]+)\.ipfs\./i);
        if (m2 && m2[1]) return m2[1];
        return s;
      }}

      var form = document.getElementById("cid-form");
      var input = document.getElementById("cid-input");
      if (!form || !input) return;
      form.addEventListener("submit", function (e) {{
        e.preventDefault();
        var cid = extractCid(input.value);
        if (!cid) return;
        window.location.href = "/s/" + encodeURIComponent(cid) + "/";
      }});
    }})();
  </script>
</body>
</html>"#,
        GATEWAY_VERSION
    ))
}

#[derive(Debug, Deserialize)]
struct EdgeBinding {
    target: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadPayload {
    schema: String,
    target: String,
    #[serde(default)]
    bundle_cid: Option<String>,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
    content_digest: String,
    entry_count: u64,
    total_bytes: u64,
    activated_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadEnvelope {
    payload: SiteHeadPayload,
    signature: String,
    signer_did: String,
}

struct ResolvedSiteRoot {
    target: String,
    explicit_binding: bool,
}

async fn serve_public_root(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, "").await {
        Ok(response) => response,
        Err(status) if resolved.explicit_binding => (status, "Not found").into_response(),
        Err(_) => landing_page().await.into_response(),
    }
}

async fn resolve_bound_site_root(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<ResolvedSiteRoot, StatusCode> {
    let Some(host) = request_host(headers) else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding_path = edge_binding_path(&state.data_dir, &host);
    let Ok(bytes) = tokio::fs::read(&binding_path).await else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding: EdgeBinding =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
    if rooted_localhost_fs_path(&state.data_dir, &binding.target).is_none() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(ResolvedSiteRoot {
        target: binding.target,
        explicit_binding: true,
    })
}

fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))?
        .to_str()
        .ok()?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix('[') {
        let end = stripped.find(']')?;
        return Some(stripped[..end].to_string());
    }
    Some(raw.split(':').next().unwrap_or("").to_string())
}

async fn healthz() -> &'static str {
    "OK"
}

async fn serve_release_manifest(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_manifest_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release.json not found").into_response(),
    }
}

async fn serve_release_head(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_head_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release-head.json not found").into_response(),
    }
}

async fn serve_artifact_file(
    State(state): State<GatewayState>,
    Path(path): Path<String>,
) -> Response {
    if let Err(msg) = validate_file_path(&path) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let artifacts_root = publisher_artifacts_path(&state.data_dir);
    let requested = artifacts_root.join(&path);
    let Ok(root_canonical) = tokio::fs::canonicalize(&artifacts_root).await else {
        return (StatusCode::NOT_FOUND, "artifacts not found").into_response();
    };
    let Ok(requested_canonical) = tokio::fs::canonicalize(&requested).await else {
        return (StatusCode::NOT_FOUND, "artifact not found").into_response();
    };
    if !requested_canonical.starts_with(&root_canonical) {
        return (StatusCode::BAD_REQUEST, "Path traversal not allowed").into_response();
    }

    match tokio::fs::read(&requested_canonical).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", content_type(&path))],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "artifact not found").into_response(),
    }
}

async fn serve_install_script(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let path = publisher_install_script_path(&state.data_dir);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        // Dynamically stamp the publisher gateway URL so `curl <gw>/install.sh | bash`
        // automatically embeds this gateway for future `elastos update`.
        let script = String::from_utf8_lossy(&bytes);
        let stamped = if script.contains("__PUBLISHER_GATEWAY__") {
            if let Some(host) = headers
                .get("x-forwarded-host")
                .or_else(|| headers.get("host"))
                .and_then(|v| v.to_str().ok())
            {
                let scheme = headers
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("https");
                let gateway_url = format!("{}://{}", scheme, host.trim_end_matches('/'));
                script
                    .replace("__PUBLISHER_GATEWAY__", &gateway_url)
                    .into_bytes()
            } else {
                bytes
            }
        } else {
            bytes
        };
        return (
            StatusCode::OK,
            [("content-type", "text/x-shellscript")],
            stamped,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "install.sh not found").into_response()
}

async fn serve_site_head_document(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    let Some(site_head) = load_site_head(&state, &resolved.target).await else {
        return (StatusCode::NOT_FOUND, "site head not found").into_response();
    };
    match serde_json::to_vec(&site_head) {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "site head encode failed").into_response(),
    }
}

async fn serve_public_site_path(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, &path).await {
        Ok(response) => response,
        Err(status) => (status, "Not found").into_response(),
    }
}

async fn load_site_head(state: &GatewayState, site_root_uri: &str) -> Option<SiteHeadEnvelope> {
    let path = edge_site_head_path(&state.data_dir, site_root_uri);
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn redirect_cid_root(Path(cid): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/s/{}/", cid))
}

async fn serve_cid_root(State(state): State<GatewayState>, Path(cid): Path<String>) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    serve_directory_root(&state, &cid).await
}

async fn serve_ipfs_cid_root(
    State(state): State<GatewayState>,
    Path(cid): Path<String>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    let raw_cache = state.cache_dir.join(format!("{}.raw", cid));
    if let Ok(bytes) = tokio::fs::read(&raw_cache).await {
        return (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response();
    }

    match fetch_file_inline(&state, &cid, "").await {
        Ok(bytes) => {
            let _ = tokio::fs::create_dir_all(&state.cache_dir).await;
            let _ = tokio::fs::write(&raw_cache, &bytes).await;
            (
                StatusCode::OK,
                [("content-type", "application/octet-stream")],
                bytes,
            )
                .into_response()
        }
        Err(_) => serve_directory_root(&state, &cid).await,
    }
}

async fn serve_directory_root(state: &GatewayState, cid: &str) -> Response {
    match serve_cid_path_result(state, cid, "index.html").await {
        Ok(response) => response,
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found in CID bundle").into_response(),
    }
}

async fn serve_cid_path_result(
    state: &GatewayState,
    cid: &str,
    file_path: &str,
) -> Result<Response, StatusCode> {
    validate_file_path(file_path).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Fast path: check local cache.
    let cid_dir = state.cache_dir.join(cid);
    let requested = cid_dir.join(file_path);
    if cid_dir.is_dir() {
        let canonical_cid_dir = cid_dir.canonicalize().unwrap_or_else(|_| cid_dir.clone());
        let canonical_requested = requested
            .canonicalize()
            .unwrap_or_else(|_| cid_dir.join(file_path));
        if canonical_requested.starts_with(&canonical_cid_dir) {
            if let Ok(bytes) = tokio::fs::read(&requested).await {
                let ct = content_type(file_path);
                return Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response());
            }
        }
    }

    // Cache miss — fetch the individual file inline via cat.
    match fetch_file_inline(state, cid, file_path).await {
        Ok(bytes) => {
            let cache_path = state.cache_dir.join(cid).join(file_path);
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &bytes).await;
            let ct = content_type(file_path);
            Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response())
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

fn stamp_site_headers(
    response: &mut Response,
    site_root_uri: &str,
    site_head: Option<&SiteHeadEnvelope>,
) -> Result<(), StatusCode> {
    let site_origin = HeaderValue::from_str(site_root_uri).map_err(|_| StatusCode::BAD_GATEWAY)?;
    response
        .headers_mut()
        .insert("X-Elastos-Site-Origin", site_origin);
    if let Some(site_head) = site_head {
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Schema",
            HeaderValue::from_str(&site_head.payload.schema)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Digest",
            HeaderValue::from_str(&site_head.payload.content_digest)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Signer",
            HeaderValue::from_str(&site_head.signer_did).map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Cid",
                HeaderValue::from_str(bundle_cid).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(release_name) = site_head.payload.release_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Release",
                HeaderValue::from_str(release_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(channel_name) = site_head.payload.channel_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Channel",
                HeaderValue::from_str(channel_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
    }
    Ok(())
}

async fn serve_site_file(
    state: &GatewayState,
    site_root_uri: &str,
    request_path: &str,
) -> Result<Response, StatusCode> {
    let requested = request_path.trim_start_matches('/');
    if !requested.is_empty() {
        validate_file_path(requested).map_err(|_| StatusCode::BAD_REQUEST)?;
    }

    let site_head = load_site_head(state, site_root_uri).await;
    if let Some(site_head) = site_head.as_ref() {
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            let bundle_candidates: Vec<String> = if requested.is_empty() {
                vec!["index.html".to_string()]
            } else {
                vec![requested.to_string(), format!("{}/index.html", requested)]
            };
            for bundle_path in bundle_candidates {
                if let Ok(mut response) =
                    serve_cid_path_result(state, bundle_cid, &bundle_path).await
                {
                    stamp_site_headers(&mut response, site_root_uri, Some(site_head))?;
                    return Ok(response);
                }
            }
            return Err(StatusCode::NOT_FOUND);
        }
    }

    let site_root =
        rooted_localhost_fs_path(&state.data_dir, site_root_uri).ok_or(StatusCode::BAD_GATEWAY)?;
    let mut candidates = Vec::new();
    if requested.is_empty() {
        candidates.push(site_root.join("index.html"));
    } else {
        candidates.push(site_root.join(requested));
        candidates.push(site_root.join(requested).join("index.html"));
    }

    let root_canonical = tokio::fs::canonicalize(&site_root)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    for candidate in candidates {
        let Ok(metadata) = tokio::fs::metadata(&candidate).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(candidate_canonical) = tokio::fs::canonicalize(&candidate).await else {
            continue;
        };
        if !candidate_canonical.starts_with(&root_canonical) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = tokio::fs::read(&candidate_canonical)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let path_for_type = candidate_canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index.html");
        let mut response = (
            StatusCode::OK,
            [("content-type", content_type(path_for_type))],
            bytes,
        )
            .into_response();
        stamp_site_headers(&mut response, site_root_uri, site_head.as_ref())?;
        return Ok(response);
    }

    Err(StatusCode::NOT_FOUND)
}

async fn serve_cid_file(
    State(state): State<GatewayState>,
    Path((cid, file_path)): Path<(String, String)>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    match serve_cid_path_result(&state, &cid, &file_path).await {
        Ok(response) => response,
        Err(StatusCode::BAD_REQUEST) => {
            (StatusCode::BAD_REQUEST, "Invalid file path").into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "File not found").into_response(),
    }
}

/// Fetch a single file from IPFS inline via the `cat` operation.
/// Returns raw bytes. Works with VM-based providers (data returned over vsock).
async fn fetch_file_inline(state: &GatewayState, cid: &str, path: &str) -> anyhow::Result<Vec<u8>> {
    let req = serde_json::json!({
        "op": "cat",
        "cid": cid,
        "path": path,
    });
    let resp = send_ipfs_raw(state, &req).await?;
    let status = resp
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("error");
    if status != "ok" {
        let msg = resp
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("{}", msg);
    }
    let data_b64 = resp
        .get("data")
        .and_then(|d| d.get("data"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("no data in cat response"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64)?;
    if bytes.len() > MAX_GATEWAY_FILE_SIZE {
        anyhow::bail!("file exceeds size limit");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------------

/// Validate a request file path — reject traversal, absolute paths, backslashes.
fn validate_file_path(path: &str) -> Result<(), &'static str> {
    // Reject absolute paths
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("Absolute paths not allowed");
    }
    // Reject backslashes (Windows-style)
    if path.contains('\\') {
        return Err("Backslashes not allowed");
    }
    // Reject traversal (raw and URL-encoded)
    if path.contains("..") {
        return Err("Path traversal not allowed");
    }
    // Check URL-encoded traversal variants
    let decoded = path.replace("%2e", ".").replace("%2E", ".");
    if decoded.contains("..") {
        return Err("Encoded path traversal not allowed");
    }
    let decoded_slash = path.replace("%2f", "/").replace("%2F", "/");
    if decoded_slash.contains("..") {
        return Err("Encoded path traversal not allowed");
    }

    Ok(())
}

/// Get or create a cached ipfs-provider bridge.
async fn get_or_create_bridge(
    state: &GatewayState,
) -> anyhow::Result<Arc<elastos_runtime::provider::ProviderBridge>> {
    let mut guard = state.ipfs_bridge.lock().await;
    if let Some(ref bridge) = *guard {
        return Ok(Arc::clone(bridge));
    }

    let ipfs_binary = state.ipfs_provider_binary.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "ipfs-provider not found. Build: cd capsules/ipfs-provider && cargo build --release"
        )
    })?;

    let bridge = Arc::new(
        elastos_runtime::provider::ProviderBridge::spawn(
            ipfs_binary,
            elastos_runtime::provider::BridgeProviderConfig::default(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ipfs-provider: {}", e))?,
    );

    *guard = Some(Arc::clone(&bridge));
    Ok(bridge)
}

/// Send a raw request to ipfs-provider.
///
/// Preferred path: use runtime provider registry (VM-backed provider route).
/// Fallback path: spawn/hold a direct ProviderBridge from ipfs-provider binary.
async fn send_ipfs_raw(
    state: &GatewayState,
    request: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if let Some(registry) = state.provider_registry.as_ref() {
        return registry
            .send_raw("ipfs", request)
            .await
            .map_err(|e| anyhow::anyhow!("provider registry ipfs request failed: {}", e));
    }

    let bridge = get_or_create_bridge(state).await?;
    bridge
        .send_raw(request)
        .await
        .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))
}

// ---------------------------------------------------------------------------
// MIME types
// ---------------------------------------------------------------------------

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("txt" | "sh") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// CID validation (one-liner, avoids depending on main.rs)
// ---------------------------------------------------------------------------

fn is_valid_cid(s: &str) -> bool {
    cid::Cid::try_from(s).is_ok()
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

pub async fn start_gateway_server(
    addr: &str,
    ipfs_provider_binary: Option<PathBuf>,
    provider_registry: Option<Arc<ProviderRegistry>>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    let state = GatewayState {
        ipfs_provider_binary,
        provider_registry,
        cache_dir,
        data_dir,
        ipfs_bridge: Arc::new(Mutex::new(None)),
    };
    let app = gateway_router(state);
    let listener = TcpListener::bind(addr).await?;
    println!("ElastOS Gateway v{}", GATEWAY_VERSION);
    println!("  Listening: http://{}", addr);
    println!("  Content:   http://{}/s/<cid>/", addr);
    println!();
    println!("  Cache is unbounded (Tier 1) — delete cache dir to reclaim space");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            println!("\nShutting down gateway...");
        })
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate.recv() => {},
            }
        } else {
            ctrl_c.await;
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    // Real CIDs that pass cid crate validation
    const TEST_CIDV0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
    const TEST_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    fn test_state(cache_dir: &std::path::Path) -> GatewayState {
        GatewayState {
            ipfs_provider_binary: None,
            provider_registry: None,
            cache_dir: cache_dir.to_path_buf(),
            data_dir: cache_dir.to_path_buf(),
            ipfs_bridge: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn test_content_type_mapping() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("style.css"), "text/css");
        assert_eq!(content_type("app.js"), "application/javascript");
        assert_eq!(content_type("data.json"), "application/json");
        assert_eq!(content_type("README.md"), "text/markdown; charset=utf-8");
        assert_eq!(content_type("image.png"), "image/png");
        assert_eq!(content_type("photo.jpg"), "image/jpeg");
        assert_eq!(content_type("photo.jpeg"), "image/jpeg");
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("module.wasm"), "application/wasm");
        assert_eq!(content_type("unknown.xyz"), "application/octet-stream");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[test]
    fn test_validate_file_path() {
        assert!(validate_file_path("index.html").is_ok());
        assert!(validate_file_path("sub/dir/file.js").is_ok());
        assert!(validate_file_path("a.b.c.txt").is_ok());

        assert!(validate_file_path("../etc/passwd").is_err());
        assert!(validate_file_path("foo/../../etc/passwd").is_err());
        assert!(validate_file_path("/absolute/path").is_err());
        assert!(validate_file_path("foo\\bar").is_err());
        assert!(validate_file_path("\\windows\\path").is_err());
    }

    #[test]
    fn test_validate_file_path_encoded() {
        assert!(validate_file_path("%2e%2e/etc/passwd").is_err());
        assert!(validate_file_path("%2E%2E/etc/passwd").is_err());
        assert!(validate_file_path("foo%2F..%2Fetc/passwd").is_err());
        assert!(validate_file_path("foo/%2e%2e/bar").is_err());
    }

    #[tokio::test]
    async fn test_landing_page_200() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("ElastOS Gateway"));
    }

    #[tokio::test]
    async fn test_root_serves_mywebsite_when_staged() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>pc2 site</html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-origin")
                .and_then(|v| v.to_str().ok()),
            Some("localhost://MyWebSite")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>pc2 site</html>");
    }

    #[tokio::test]
    async fn test_healthz_200() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_invalid_cid_400() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/s/not-a-cid/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_cid_without_trailing_slash_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(location, format!("/s/{}/", TEST_CIDV1));
    }

    #[tokio::test]
    async fn test_ipfs_cid_root_serves_cached_raw_file_without_redirect() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{}.raw", TEST_CIDV1)),
            b"raw-binary",
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/ipfs/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/octet-stream");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"raw-binary");
    }

    #[tokio::test]
    async fn test_ipfs_cid_root_falls_back_to_cached_directory_index() {
        let dir = tempfile::tempdir().unwrap();
        let cid_dir = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cid_dir).unwrap();
        std::fs::write(cid_dir.join("index.html"), "<html>ok</html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/ipfs/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "text/html; charset=utf-8");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>ok</html>");
    }

    #[tokio::test]
    async fn test_traversal_400() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}/../etc/passwd", TEST_CIDV0))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_missing_file_404() {
        let dir = tempfile::tempdir().unwrap();
        // Pre-populate cache so we don't need IPFS
        let cid_dir = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cid_dir).unwrap();
        std::fs::write(cid_dir.join("index.html"), "<html></html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}/no-such-file.txt", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_release_head_200() {
        let dir = tempfile::tempdir().unwrap();
        let head = r#"{"payload":{"schema":"elastos.release.head/v1"}}"#;
        let publisher_root = publisher_release_head_path(dir.path());
        std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
        std::fs::write(publisher_root, head).unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_release_head_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_release_json_200() {
        let dir = tempfile::tempdir().unwrap();
        let release = r#"{"payload":{"schema":"elastos.release/v1"}}"#;
        let publisher_root = publisher_release_manifest_path(dir.path());
        std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
        std::fs::write(publisher_root, release).unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_install_sh_200() {
        let dir = tempfile::tempdir().unwrap();
        let install_path = publisher_install_script_path(dir.path());
        std::fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        std::fs::write(install_path, "#!/bin/bash\necho hi").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/install.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "text/x-shellscript");
    }

    #[tokio::test]
    async fn test_install_sh_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/install.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_artifact_file_200() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts_dir = publisher_artifacts_path(dir.path());
        std::fs::create_dir_all(&artifacts_dir).unwrap();
        std::fs::write(artifacts_dir.join("components-linux-amd64.json"), "{}").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/artifacts/components-linux-amd64.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_domain_binding_serves_bound_root() {
        let dir = tempfile::tempdir().unwrap();
        let public_site = dir.path().join("Public").join("docs");
        std::fs::create_dir_all(&public_site).unwrap();
        std::fs::write(public_site.join("index.html"), "<html>bound site</html>").unwrap();

        let binding_path = edge_binding_path(dir.path(), "docs.example.com");
        std::fs::create_dir_all(binding_path.parent().unwrap()).unwrap();
        std::fs::write(
            &binding_path,
            r#"{"domain":"docs.example.com","target":"localhost://Public/docs"}"#,
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", "docs.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-origin")
                .and_then(|v| v.to_str().ok()),
            Some("localhost://Public/docs")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>bound site</html>");
    }

    #[tokio::test]
    async fn test_site_head_document_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>pc2 site</html>").unwrap();
        let cached_bundle = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cached_bundle).unwrap();
        std::fs::write(
            cached_bundle.join("index.html"),
            "<html>published bundle</html>",
        )
        .unwrap();

        let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
        std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
        std::fs::write(
            &head_path,
            r#"{"payload":{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","release_name":"v1","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":21,"activated_at":123},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}"#,
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let root_resp = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root_resp.status(), StatusCode::OK);
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-schema")
                .and_then(|v| v.to_str().ok()),
            Some("elastos.site.head.v1")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-digest")
                .and_then(|v| v.to_str().ok()),
            Some("sha256:abc123")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-cid")
                .and_then(|v| v.to_str().ok()),
            Some(TEST_CIDV1)
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-release")
                .and_then(|v| v.to_str().ok()),
            Some("v1")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-channel")
                .and_then(|v| v.to_str().ok()),
            Some("live")
        );

        let head_resp = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/elastos/site-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(head_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"schema\":\"elastos.site.head.v1\""));
        assert!(text.contains("\"target\":\"localhost://MyWebSite\""));
        assert!(text.contains(&format!("\"bundle_cid\":\"{}\"", TEST_CIDV1)));
        assert!(text.contains("\"release_name\":\"v1\""));
        assert!(text.contains("\"channel_name\":\"live\""));
    }

    #[tokio::test]
    async fn test_active_site_head_prefers_bundle_cid() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>working tree</html>").unwrap();

        let cached_bundle = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cached_bundle).unwrap();
        std::fs::write(
            cached_bundle.join("index.html"),
            "<html>published bundle</html>",
        )
        .unwrap();

        let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
        std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
        std::fs::write(
            &head_path,
            format!(
                r#"{{"payload":{{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"{}","release_name":"v2","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":28,"activated_at":123}},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}}"#,
                TEST_CIDV1
            ),
        )
        .unwrap();

        let app = gateway_router(test_state(dir.path()));
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-cid")
                .and_then(|v| v.to_str().ok()),
            Some(TEST_CIDV1)
        );
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-release")
                .and_then(|v| v.to_str().ok()),
            Some("v2")
        );
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-channel")
                .and_then(|v| v.to_str().ok()),
            Some("live")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>published bundle</html>");
    }
}
