use super::*;

#[derive(Debug, Deserialize)]
struct EdgeBinding {
    target: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct SiteHeadPayload {
    pub(super) schema: String,
    pub(super) target: String,
    #[serde(default)]
    pub(super) bundle_cid: Option<String>,
    #[serde(default)]
    pub(super) release_name: Option<String>,
    #[serde(default)]
    pub(super) channel_name: Option<String>,
    pub(super) content_digest: String,
    pub(super) entry_count: u64,
    pub(super) total_bytes: u64,
    pub(super) activated_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct SiteHeadEnvelope {
    pub(super) payload: SiteHeadPayload,
    pub(super) signature: String,
    pub(super) signer_did: String,
}

struct ResolvedSiteRoot {
    target: String,
    explicit_binding: bool,
}

pub(super) async fn serve_public_root(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
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
    let host = match request_host(headers) {
        Ok(Some(host)) => host,
        Ok(None) => {
            return Ok(ResolvedSiteRoot {
                target: MY_WEBSITE_URI.to_string(),
                explicit_binding: false,
            });
        }
        Err(_) => return Err(StatusCode::BAD_REQUEST),
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

pub(super) fn request_host(headers: &HeaderMap) -> anyhow::Result<Option<String>> {
    if headers.get("host").is_none() {
        return Ok(None);
    }
    Ok(Some(validated_gateway_host(headers)?))
}

pub(super) async fn healthz() -> &'static str {
    "OK"
}

pub(super) async fn serve_release_manifest(State(state): State<GatewayState>) -> Response {
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

pub(super) async fn serve_release_head(State(state): State<GatewayState>) -> Response {
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

pub(super) async fn serve_artifact_file(
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

pub(super) async fn serve_install_script(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let path = publisher_install_script_path(&state.data_dir);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        // Dynamically stamp the publisher gateway URL so `curl <gw>/install.sh | bash`
        // automatically embeds this gateway for future `elastos update`.
        let script = String::from_utf8_lossy(&bytes);
        let stamped = if script.contains("__PUBLISHER_GATEWAY__") {
            match effective_gateway_origin(&headers) {
                Ok(origin) => script
                    .replace("__PUBLISHER_GATEWAY__", origin.origin())
                    .into_bytes(),
                Err(_) if headers.get("host").is_none() => bytes,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "invalid gateway origin").into_response();
                }
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

pub(super) async fn serve_site_head_document(
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

pub(super) async fn serve_public_site_path(
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

pub(super) async fn redirect_cid_root(Path(cid): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/s/{}/", cid))
}

pub(super) async fn serve_cid_root(
    State(state): State<GatewayState>,
    Path(cid): Path<String>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    serve_directory_root(&state, &cid).await
}

/// Published CID content is embeddable by a capsule page.
///
/// A capsule runs under `Cross-Origin-Embedder-Policy: require-corp` in an
/// opaque origin, so every subresource it loads must say it may be embedded.
/// Without this the fetch succeeds and the browser throws the bytes away:
/// `ERR_BLOCKED_BY_RESPONSE.NotSameOriginAfterDefaultedToSameOriginByCoep`,
/// a 200 that renders nothing. Capsule assets already answer `cross-origin`
/// for the same reason, and this content is public by construction.
const CID_CONTENT_CORP: &str = "cross-origin";

/// Published CID content is also readable by a capsule's own `fetch`.
///
/// `Cross-Origin-Resource-Policy` lets a capsule EMBED this content -- an
/// `<img>`, which is a no-cors request. Reading it with `fetch`, as the shelf
/// does for an item's metadata document, is a CORS request from an opaque
/// origin, and without this it fails before any status is seen: not a 404 a
/// caller can act on, but a network error it cannot tell from a missing file.
///
/// `null` is the capsule origin, the same value the capsule's own assets are
/// served with.
const CID_CONTENT_ORIGIN: &str = "null";

/// What a bare CID's bytes actually are.
///
/// A CID carries no name, so this route answered `application/octet-stream`
/// for everything -- which a browser treats as a file to save rather than a
/// picture to show, and which made every channel cover arrive in the
/// downloads tray instead of on the card.
///
/// Decided from the bytes' own magic number, never from anything a caller
/// said. Only a short list, and deliberately no `text/html` or `image/svg+xml`
/// from an unnamed blob: both are documents that can carry script, and this
/// origin is the one the Home's own pages are served from. A named path inside
/// a CID bundle is a different case, handled by `content_type` and by the
/// publishing flow that put a name there.
pub(super) fn sniffed_cid_content_type(bytes: &[u8]) -> &'static str {
    const UNKNOWN: &str = "application/octet-stream";
    if bytes.len() < 12 {
        return UNKNOWN;
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return "image/png";
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return "image/jpeg";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return "image/x-icon";
    }
    if bytes.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    // ISO base media: `ftyp` at offset 4, with the brand deciding which.
    if &bytes[4..8] == b"ftyp" {
        return match &bytes[8..11] {
            b"M4A" => "audio/mp4",
            b"qt " => "video/quicktime",
            _ => "video/mp4",
        };
    }
    if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return "video/webm";
    }
    if bytes.starts_with(b"OggS") {
        return "audio/ogg";
    }
    if bytes.starts_with(b"ID3") || bytes.starts_with(&[0xff, 0xfb]) {
        return "audio/mpeg";
    }
    UNKNOWN
}

pub(super) async fn serve_ipfs_cid_root(
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
            [
                ("content-type", sniffed_cid_content_type(&bytes)),
                ("cross-origin-resource-policy", CID_CONTENT_CORP),
                ("access-control-allow-origin", CID_CONTENT_ORIGIN),
            ],
            bytes,
        )
            .into_response();
    }

    let cached_index = state.cache_dir.join(&cid).join("index.html");
    if cached_index.is_file() {
        return serve_directory_root(&state, &cid).await;
    }

    match fetch_file_inline(&state, &cid, "").await {
        Ok(bytes) => {
            let _ = tokio::fs::create_dir_all(&state.cache_dir).await;
            let _ = tokio::fs::write(&raw_cache, &bytes).await;
            (
                StatusCode::OK,
                [
                    ("content-type", sniffed_cid_content_type(&bytes)),
                    ("cross-origin-resource-policy", CID_CONTENT_CORP),
                    ("access-control-allow-origin", CID_CONTENT_ORIGIN),
                ],
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
                return Ok((
                    StatusCode::OK,
                    [
                        ("content-type", ct),
                        ("cross-origin-resource-policy", CID_CONTENT_CORP),
                        ("access-control-allow-origin", CID_CONTENT_ORIGIN),
                    ],
                    bytes,
                )
                    .into_response());
            }
        }
    }

    // Cache miss: fetch the individual file through the content provider.
    match fetch_file_inline(state, cid, file_path).await {
        Ok(bytes) => {
            let cache_path = state.cache_dir.join(cid).join(file_path);
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &bytes).await;
            let ct = content_type(file_path);
            Ok((
                StatusCode::OK,
                [
                    ("content-type", ct),
                    ("cross-origin-resource-policy", CID_CONTENT_CORP),
                    ("access-control-allow-origin", CID_CONTENT_ORIGIN),
                ],
                bytes,
            )
                .into_response())
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

pub(super) async fn serve_cid_file(
    State(state): State<GatewayState>,
    Path((cid, file_path)): Path<(String, String)>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    match serve_cid_path_result(&state, &cid, &file_path).await {
        Ok(response) => response,
        Err(StatusCode::BAD_REQUEST) => (
            StatusCode::BAD_REQUEST,
            [("access-control-allow-origin", CID_CONTENT_ORIGIN)],
            "Invalid file path",
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            [("access-control-allow-origin", CID_CONTENT_ORIGIN)],
            "File not found",
        )
            .into_response(),
    }
}

/// Fetch a single file through the content availability provider.
/// Returns raw bytes; the provider decides whether the current backend is local IPFS,
/// an availability replica, or a future repair/fetch path.
async fn fetch_file_inline(state: &GatewayState, cid: &str, path: &str) -> anyhow::Result<Vec<u8>> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("gateway provider registry unavailable"))?;
    let bytes =
        crate::content::fetch_bytes_via_provider(registry, cid, (!path.is_empty()).then_some(path))
            .await?;
    if bytes.len() > MAX_GATEWAY_FILE_SIZE {
        anyhow::bail!("file exceeds size limit");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------------

/// Validate a request file path — reject traversal, absolute paths, backslashes.
pub(in crate::api) fn validate_file_path(path: &str) -> Result<(), &'static str> {
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

// ---------------------------------------------------------------------------
// MIME types
// ---------------------------------------------------------------------------

pub(in crate::api) fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js" | "mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
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
