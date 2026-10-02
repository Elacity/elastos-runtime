use super::*;
use crate::release_publication::Publication;
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt};

#[derive(Deserialize)]
struct PublicationReceipt {
    publisher_did: String,
    last_release_cid: String,
    last_head_cid: String,
    last_version: String,
}

// A receipt commit admits one verified snapshot. Later reads check identities
// and hash only the requested artifact. Concurrent requests wait their turn.
#[derive(Clone)]
pub(super) struct ReleaseReadGate {
    pub(super) permit: Arc<tokio::sync::Semaphore>,
    cache: Arc<std::sync::Mutex<Option<CachedPublication>>>,
    #[cfg(test)]
    pub(super) admissions: Arc<std::sync::atomic::AtomicUsize>,
}

impl ReleaseReadGate {
    pub(super) fn new() -> Self {
        Self {
            permit: Arc::new(tokio::sync::Semaphore::new(1)),
            cache: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            admissions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}

type ReceiptStamp = (u64, u64, u64, i64, i64, i64, i64, u32, u64);

fn receipt_stamp(metadata: &std::fs::Metadata) -> ReceiptStamp {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mode(),
        metadata.nlink(),
    )
}

#[derive(PartialEq, Eq)]
struct PublicationKey {
    root_path: std::path::PathBuf,
    receipt_stamp: ReceiptStamp,
}

struct PublicationContext {
    key: PublicationKey,
    receipt: PublicationReceipt,
    receipt_file: File,
}

struct CachedPublication {
    key: PublicationKey,
    // Retaining the receipt descriptor prevents inode reuse for this cache key.
    _receipt_file: File,
    // A changed snapshot stays refused until the publisher commits its receipt.
    publication: Option<Publication>,
}

fn publication_open_at(parent: &File, name: &str, directory: bool) -> std::io::Result<File> {
    let name = CString::new(name)?;
    let flags = libc::O_RDONLY
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | libc::O_CLOEXEC
        | if directory { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn publication_directory(path: &std::path::Path) -> std::io::Result<File> {
    use std::path::Component;
    let mut directory = File::open("/")?;
    if !path.is_absolute() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = name.to_str().ok_or(std::io::ErrorKind::InvalidInput)?;
                directory = publication_open_at(&directory, name, true)?;
            }
            _ => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        }
    }
    Ok(directory)
}

fn read_publication_receipt(
    root: &File,
) -> anyhow::Result<(PublicationReceipt, File, ReceiptStamp)> {
    let file = publication_open_at(root, "publish-state.json", false)?;
    let before = file.metadata()?;
    anyhow::ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.uid() == unsafe { libc::geteuid() }
            && before.mode() & 0o022 == 0
            && before.len() > 0
            && before.len() <= 64 * 1024,
        "unsafe publisher receipt"
    );
    let mut bytes = vec![0; before.len() as usize];
    file.read_exact_at(&mut bytes, 0)?;
    let stamp = receipt_stamp(&before);
    anyhow::ensure!(
        stamp == receipt_stamp(&file.metadata()?),
        "publisher receipt changed"
    );
    Ok((serde_json::from_slice(&bytes)?, file, stamp))
}

fn publication_context(data_dir: &std::path::Path) -> Result<PublicationContext, StatusCode> {
    // Runtime owns data_dir. Normalize its OS alias (e.g. macOS /var) before
    // the no-follow walk of the publication and its saved policy receipt.
    let base = data_dir.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    })?;
    let root_path = elastos_common::localhost::publisher_root_path(&base);
    let root = publication_directory(&root_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    })?;
    let validated = (|| -> anyhow::Result<PublicationContext> {
        let metadata = root.metadata()?;
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
            "unsafe publisher root"
        );
        let (receipt, receipt_file, stamp) = read_publication_receipt(&root)?;
        Ok(PublicationContext {
            key: PublicationKey {
                root_path,
                receipt_stamp: stamp,
            },
            receipt,
            receipt_file,
        })
    })();
    validated.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

fn load_release_publication(context: &PublicationContext) -> anyhow::Result<Publication> {
    // The operator's committed receipt owns the pin, rather than the envelope.
    let receipt = &context.receipt;
    let publication = Publication::open_published(&context.key.root_path, &receipt.publisher_did)?;
    anyhow::ensure!(
        receipt.last_release_cid == publication.release_cid()
            && receipt.last_version == publication.version(),
        "publisher receipt differs from signed set"
    );
    crate::update::verify_release_metadata_cid(&receipt.last_head_cid, publication.head_bytes())?;
    Ok(publication)
}

enum ReleaseFile {
    Head,
    Release,
    Installer,
    Artifact(String),
}

async fn signed_release_response(
    state: GatewayState,
    gate: ReleaseReadGate,
    file: ReleaseFile,
    media_type: &'static str,
) -> Response {
    let Ok(permit) = gate.permit.clone().acquire_owned().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Release publication unavailable",
        )
            .into_response();
    };
    let data_dir = state.data_dir;
    let result = tokio::task::spawn_blocking(move || {
        // Cancellation keeps the worker's permit until it finishes its checks.
        let _permit = permit;
        let context = publication_context(&data_dir)?;
        let mut cache = gate
            .cache
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if cache.as_ref().is_none_or(|saved| saved.key != context.key) {
            #[cfg(test)]
            gate.admissions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let publication = load_release_publication(&context).ok();
            *cache = Some(CachedPublication {
                key: context.key,
                _receipt_file: context.receipt_file,
                publication,
            });
        }
        let saved = cache.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let checked = (|| {
            let publication = saved
                .publication
                .as_ref()
                .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            publication
                .unchanged_published()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let response = match file {
                ReleaseFile::Head => Ok(publication.head_bytes().to_vec()),
                ReleaseFile::Release => Ok(publication.release_bytes().to_vec()),
                ReleaseFile::Installer => Ok(publication.installer_bytes().to_vec()),
                ReleaseFile::Artifact(name) => {
                    if publication
                        .artifacts()
                        .iter()
                        .any(|record| record.name == name)
                    {
                        publication
                            .read_verified_artifact(&name)
                            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
                    } else {
                        Err(StatusCode::NOT_FOUND)
                    }
                }
            };
            publication
                .unchanged_published()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let after =
                publication_context(&data_dir).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if after.key != saved.key {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            response
        })();
        if matches!(checked, Err(StatusCode::SERVICE_UNAVAILABLE)) {
            saved.publication = None;
        }
        checked
    })
    .await
    .unwrap_or(Err(StatusCode::SERVICE_UNAVAILABLE));
    match result {
        Ok(bytes) => (StatusCode::OK, [("content-type", media_type)], bytes).into_response(),
        Err(StatusCode::NOT_FOUND) => {
            (StatusCode::NOT_FOUND, "Release file not found").into_response()
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Release publication unavailable",
        )
            .into_response(),
    }
}

pub(super) async fn sandbox_content_response(mut response: Response) -> Response {
    // User content has an opaque origin and keeps its scripts and form controls.
    // Its scripts cannot use the gateway's opaque-app CORS path to read APIs.
    response.headers_mut().insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "sandbox allow-scripts allow-forms allow-popups; connect-src 'none'",
        ),
    );
    response
}

pub(super) async fn refuse_content_api_resources(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let headers = request.headers();
    let opaque_or_cross_site = headers.get("origin").is_some_and(|value| value == "null")
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site");
    let resource_or_navigation = headers
        .get("sec-fetch-dest")
        .is_some_and(|value| value != "empty")
        || headers
            .get("sec-fetch-mode")
            .is_some_and(|value| value == "navigate");
    // Content can use forms and links, but API documents and resource loads
    // cross the opaque boundary. App fetches and ticketed streams retain their
    // own capability checks; untrusted content's CSP blocks those connections.
    if request.uri().path().starts_with("/api/apps/")
        && opaque_or_cross_site
        && resource_or_navigation
    {
        return (
            StatusCode::FORBIDDEN,
            "API resource access requires an app connection",
        )
            .into_response();
    }
    next.run(request).await
}

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

pub(super) async fn serve_release_manifest(
    State(state): State<GatewayState>,
    Extension(gate): Extension<ReleaseReadGate>,
) -> Response {
    signed_release_response(state, gate, ReleaseFile::Release, "application/json").await
}

pub(super) async fn serve_release_head(
    State(state): State<GatewayState>,
    Extension(gate): Extension<ReleaseReadGate>,
) -> Response {
    signed_release_response(state, gate, ReleaseFile::Head, "application/json").await
}

pub(super) async fn serve_artifact_file(
    State(state): State<GatewayState>,
    Extension(gate): Extension<ReleaseReadGate>,
    Path(path): Path<String>,
) -> Response {
    if path.is_empty()
        || path.len() > 240
        || path.starts_with('.')
        || validate_file_path(&path).is_err()
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return (StatusCode::BAD_REQUEST, "Invalid artifact name").into_response();
    }
    let media_type = content_type(&path);
    signed_release_response(state, gate, ReleaseFile::Artifact(path), media_type).await
}

pub(super) async fn serve_install_script(
    State(state): State<GatewayState>,
    Extension(gate): Extension<ReleaseReadGate>,
    _headers: axum::http::HeaderMap,
) -> Response {
    signed_release_response(state, gate, ReleaseFile::Installer, "text/x-shellscript").await
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
            [("content-type", "application/octet-stream")],
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

    // Cache miss: fetch the individual file through the content provider.
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

pub(super) async fn serve_cid_file(
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
