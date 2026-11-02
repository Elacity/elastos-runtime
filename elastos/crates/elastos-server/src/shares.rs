//! Share catalog types, provenance attestation, and channel head signing.

use crate::crypto::{decode_did_key, encode_did_key};
use crate::ipfs::{collect_files_for_ipfs, find_viewer_dir};
use crate::sources::default_data_dir;
use ed25519_dalek::Signer;
use elastos_runtime::signature;
use sha2::Digest;
use std::path::PathBuf;

// --- Share catalog types ---

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelStatus {
    #[default]
    Active,
    Archived,
    Revoked,
}

impl std::fmt::Display for ChannelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelStatus::Active => write!(f, "active"),
            ChannelStatus::Archived => write!(f, "archived"),
            ChannelStatus::Revoked => write!(f, "revoked"),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct ShareCatalog {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub channels: std::collections::BTreeMap<String, ShareChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_did: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct ShareChannel {
    #[serde(default)]
    pub latest_cid: String,
    #[serde(default)]
    pub latest_version: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub history: Vec<ShareEntry>,
    #[serde(default)]
    pub status: ChannelStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_cid: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ShareEntry {
    pub cid: String,
    pub version: u64,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_cid: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct ShareMeta {
    pub schema: String,
    pub share_id: String,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    pub created_at: u64,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_did: Option<String>,
}

pub fn share_catalog_path() -> PathBuf {
    default_data_dir().join("shares").join("catalog.json")
}

pub fn derive_share_id(path: &std::path::Path, explicit: Option<&str>) -> anyhow::Result<String> {
    if let Some(raw) = explicit {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Channel name cannot be empty");
        }
        return Ok(trimmed.to_string());
    }

    let name = if path.is_dir() {
        path.file_name()
    } else {
        path.file_stem()
    }
    .and_then(|s| s.to_str())
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .ok_or_else(|| anyhow::anyhow!("Cannot derive share channel from {}", path.display()))?;

    Ok(name.to_string())
}

fn copy_markdown_file(
    src_root: &std::path::Path,
    src_path: &std::path::Path,
    bundle_dir: &std::path::Path,
) -> anyhow::Result<String> {
    let rel = src_path
        .strip_prefix(src_root)
        .unwrap_or(src_path)
        .to_path_buf();
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let dest = bundle_dir.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src_path, dest)?;
    Ok(rel_str)
}

pub fn compute_content_digest(files: &[(String, Vec<u8>)]) -> anyhow::Result<String> {
    if files.is_empty() {
        anyhow::bail!("No markdown files to share");
    }

    let mut sorted = files.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut pairs = Vec::with_capacity(sorted.len());
    for (path, bytes) in &sorted {
        let file_hash = format!("{:x}", sha2::Sha256::digest(bytes));
        pairs.push(serde_json::json!({
            "path": path,
            "sha256": file_hash,
        }));
    }
    let canonical = serde_json::to_vec(&pairs)?;
    let digest = format!("{:x}", sha2::Sha256::digest(&canonical));
    Ok(format!("sha256:{}", digest))
}

/// Upper bounds for the viewer files copied into a share bundle.
const MAX_VIEWER_FILES: usize = 256;
const MAX_VIEWER_BYTES: u64 = 16 * 1024 * 1024;

/// The viewer's web entrypoint, opened through directory file descriptors.
struct ViewerEntrypoint {
    /// The entrypoint's directory: the web root copied into the bundle.
    web_root: std::fs::File,
    file: std::fs::File,
    name: std::ffi::OsString,
}

/// Checks that the installed viewer has a usable web entrypoint in its
/// `capsule.json` (`browser/index.html` for Documents).
pub fn check_viewer_entrypoint(viewer_dir: &std::path::Path) -> anyhow::Result<()> {
    open_viewer_entrypoint(viewer_dir).map(|_| ())
}

/// Opens the manifest entrypoint one component at a time with `openat` and
/// `O_NOFOLLOW` from the viewer directory, so it must be a relative path inside
/// the capsule and no symlink on the way is followed, even one swapped in late.
fn open_viewer_entrypoint(viewer_dir: &std::path::Path) -> anyhow::Result<ViewerEntrypoint> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let manifest_path = viewer_dir.join("capsule.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).map_err(|err| {
            anyhow::anyhow!("viewer manifest {}: {err}", manifest_path.display())
        })?)?;
    let entrypoint = manifest
        .get("entrypoint")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} has no entrypoint", manifest_path.display()))?;
    let mut parts = Vec::new();
    for part in std::path::Path::new(entrypoint).components() {
        let std::path::Component::Normal(part) = part else {
            anyhow::bail!("viewer entrypoint must stay inside the capsule: {entrypoint}");
        };
        parts.push(part.to_os_string());
    }
    let name = parts
        .pop()
        .ok_or_else(|| anyhow::anyhow!("viewer entrypoint is empty"))?;
    let mut web_root = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(viewer_dir)
        .map_err(|err| anyhow::anyhow!("viewer {}: {err}", viewer_dir.display()))?;
    let mut shown = std::path::PathBuf::new();
    for part in &parts {
        shown.push(part);
        web_root = viewer_open_at(&web_root, part, libc::O_DIRECTORY, &shown)?;
    }
    shown.push(&name);
    let file = viewer_open_at(&web_root, &name, 0, &shown)?;
    if !file.metadata()?.is_file() {
        anyhow::bail!("viewer entrypoint is not a file: {}", shown.display());
    }
    Ok(ViewerEntrypoint {
        web_root,
        file,
        name,
    })
}

fn viewer_open_at(
    dir: &std::fs::File,
    name: &std::ffi::OsStr,
    flags: libc::c_int,
    shown: &std::path::Path,
) -> anyhow::Result<std::fs::File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    let c_name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c_name.as_ptr(),
            flags | libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow::bail!("viewer file missing: {}", shown.display());
        }
        anyhow::bail!(
            "viewer path is a symlink or not the expected kind: {} ({err})",
            shown.display()
        );
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// Names in an open directory, read through a duplicate of its descriptor.
fn viewer_dir_names(dir: &std::fs::File) -> anyhow::Result<Vec<std::ffi::OsString>> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    let raw = unsafe { libc::dup(dir.as_raw_fd()) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(raw) };
    if stream.is_null() {
        unsafe { libc::close(raw) };
        return Err(std::io::Error::last_os_error().into());
    }
    unsafe { libc::rewinddir(stream) };
    let mut names = Vec::new();
    let result = loop {
        // readdir returns NULL for both the end and an error; errno tells them apart.
        set_errno_zero();
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let err = std::io::Error::last_os_error();
            break match err.raw_os_error() {
                Some(0) | None => Ok(()),
                Some(_) => Err(anyhow::anyhow!("read viewer directory: {err}")),
            };
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name = std::ffi::OsStr::from_bytes(name.to_bytes());
        if name != "." && name != ".." {
            names.push(name.to_os_string());
        }
    };
    unsafe { libc::closedir(stream) };
    result.map(|()| names)
}

fn set_errno_zero() {
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0;
    }
}

/// Lists the regular files under `dir` (relative paths), refusing symlinks.
fn list_viewer_files(
    dir: &std::fs::File,
    rel: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    for name in viewer_dir_names(dir)? {
        let text = name.to_string_lossy();
        if text.starts_with('.') || text == "target" || text == "node_modules" {
            continue;
        }
        let child = rel.join(&name);
        let c_name = std::ffi::CString::new(name.as_bytes())?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                c_name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFLNK => anyhow::bail!("viewer file is a symlink: {}", child.display()),
            libc::S_IFDIR => {
                let sub = viewer_open_at(dir, &name, libc::O_DIRECTORY, &child)?;
                list_viewer_files(&sub, &child, out)?;
            }
            libc::S_IFREG => {
                if out.len() >= MAX_VIEWER_FILES {
                    anyhow::bail!("viewer has more than {MAX_VIEWER_FILES} files");
                }
                out.push(child);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Copies listed files from `web_root` to the bundle, reopening every path
/// component through `openat(O_NOFOLLOW)` so a symlink swapped in after the
/// listing is refused. Skips the entrypoint and the viewer's own Markdown.
fn copy_listed_viewer_files(
    web_root: &std::fs::File,
    files: &[std::path::PathBuf],
    entry_name: &std::ffi::OsStr,
    bundle_dir: &std::path::Path,
) -> anyhow::Result<u64> {
    let mut total = 0u64;
    for rel in files {
        let is_markdown = rel
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if rel.as_os_str() == entry_name || is_markdown {
            continue;
        }
        let mut dir = None;
        let mut shown = std::path::PathBuf::new();
        let mut parts = rel.components().peekable();
        while let Some(std::path::Component::Normal(part)) = parts.next() {
            shown.push(part);
            let parent = dir.as_ref().unwrap_or(web_root);
            if parts.peek().is_some() {
                dir = Some(viewer_open_at(parent, part, libc::O_DIRECTORY, &shown)?);
            } else {
                let file = viewer_open_at(parent, part, 0, &shown)?;
                total += copy_viewer_file(file, &bundle_dir.join(rel), &shown)?;
            }
        }
        if total > MAX_VIEWER_BYTES {
            anyhow::bail!("viewer files exceed {MAX_VIEWER_BYTES} bytes");
        }
    }
    Ok(total)
}

/// Copies the viewer's web files (the entrypoint's directory, without its own
/// Markdown) to the bundle root, with the entrypoint as `index.html`. Refuses
/// symlinks anywhere in that tree and bounds the file count and total size.
fn copy_viewer_entrypoint(
    viewer_dir: &std::path::Path,
    bundle_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let entry = open_viewer_entrypoint(viewer_dir)?;
    let mut files = Vec::new();
    list_viewer_files(&entry.web_root, std::path::Path::new(""), &mut files)?;
    let mut total = copy_listed_viewer_files(&entry.web_root, &files, &entry.name, bundle_dir)?;
    total += copy_viewer_file(
        entry.file,
        &bundle_dir.join("index.html"),
        std::path::Path::new(&entry.name),
    )?;
    if total > MAX_VIEWER_BYTES {
        anyhow::bail!("viewer files exceed {MAX_VIEWER_BYTES} bytes");
    }
    Ok(())
}

fn copy_viewer_file(
    file: std::fs::File,
    dest: &std::path::Path,
    shown: &std::path::Path,
) -> anyhow::Result<u64> {
    if !file.metadata()?.is_file() {
        anyhow::bail!("viewer file is not a regular file: {}", shown.display());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::fs::File::create(dest)?;
    Ok(std::io::copy(
        &mut std::io::Read::take(file, MAX_VIEWER_BYTES + 1),
        &mut out,
    )?)
}

pub fn build_share_bundle(
    input_path: &std::path::Path,
    share_id: &str,
    version: u64,
    prev_cid: Option<&str>,
    author_did: Option<&str>,
) -> anyhow::Result<(tempfile::TempDir, ShareMeta)> {
    let viewer_dir = find_viewer_dir("documents")?;
    build_share_bundle_with_viewer_dir(
        input_path,
        share_id,
        version,
        prev_cid,
        author_did,
        &viewer_dir,
    )
}

pub fn build_share_bundle_with_viewer_dir(
    input_path: &std::path::Path,
    share_id: &str,
    version: u64,
    prev_cid: Option<&str>,
    author_did: Option<&str>,
    viewer_dir: &std::path::Path,
) -> anyhow::Result<(tempfile::TempDir, ShareMeta)> {
    if !input_path.exists() {
        anyhow::bail!("Share path not found: {}", input_path.display());
    }

    let bundle_dir = tempfile::Builder::new()
        .prefix("elastos-share-")
        .tempdir()?;

    copy_viewer_entrypoint(viewer_dir, bundle_dir.path())?;

    let mut markdown_paths = Vec::new();
    if input_path.is_file() {
        let is_markdown = input_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !is_markdown {
            anyhow::bail!(
                "Single-file share supports Markdown files only (.md). Got: {}",
                input_path.display()
            );
        }
        markdown_paths.push(copy_markdown_file(
            input_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
            input_path,
            bundle_dir.path(),
        )?);
    } else {
        let mut files = Vec::new();
        collect_files_for_ipfs(input_path, input_path, &mut files)?;
        for rel in files {
            if rel
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
            {
                let src = input_path.join(&rel);
                markdown_paths.push(copy_markdown_file(input_path, &src, bundle_dir.path())?);
            }
        }
    }

    markdown_paths.sort();
    if markdown_paths.is_empty() {
        anyhow::bail!("No Markdown files found in {}", input_path.display());
    }

    let mut digest_files = Vec::with_capacity(markdown_paths.len());
    for rel in &markdown_paths {
        let bytes = std::fs::read(bundle_dir.path().join(rel))?;
        digest_files.push((rel.clone(), bytes));
    }
    let content_digest = compute_content_digest(&digest_files)?;
    let created_at = now_unix_secs();

    std::fs::write(
        bundle_dir.path().join("_files.json"),
        serde_json::to_string_pretty(&markdown_paths)? + "\n",
    )?;

    let capsule_json = serde_json::json!({
        "schema": elastos_common::SCHEMA_V1,
        "version": "0.1.0",
        "name": share_id,
        "description": format!("{} — shared documents", share_id),
        "role": "content",
        "type": "data",
        "entrypoint": "index.html",
        "requires": [],
        "capabilities": [],
        "resources": { "memory_mb": 16, "cpu_shares": 50 },
        "permissions": { "storage": [], "messaging": [] }
    });
    std::fs::write(
        bundle_dir.path().join("capsule.json"),
        serde_json::to_string_pretty(&capsule_json)? + "\n",
    )?;

    let meta = ShareMeta {
        schema: "elastos.share.meta/v1".to_string(),
        share_id: share_id.to_string(),
        version,
        prev: prev_cid.map(|s| s.to_string()),
        created_at,
        content_digest,
        author_did: author_did.map(|s| s.to_string()),
    };
    std::fs::write(
        bundle_dir.path().join("_share.json"),
        serde_json::to_string_pretty(&meta)? + "\n",
    )?;

    Ok((bundle_dir, meta))
}

pub fn load_share_catalog() -> anyhow::Result<ShareCatalog> {
    let path = share_catalog_path();
    if !path.exists() {
        return Ok(ShareCatalog {
            schema: "elastos.share.catalog/v1".to_string(),
            ..Default::default()
        });
    }
    let data = std::fs::read_to_string(&path)?;
    let mut catalog: ShareCatalog = serde_json::from_str(&data)?;
    if catalog.schema.is_empty() {
        catalog.schema = "elastos.share.catalog/v1".to_string();
    }
    Ok(catalog)
}

pub fn save_share_catalog(catalog: &ShareCatalog) -> anyhow::Result<()> {
    let path = share_catalog_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(catalog)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    // Atomic replace: write to tmp then rename
    if cfg!(windows) && path.exists() {
        let bak = path.with_extension("json.bak");
        let _ = std::fs::remove_file(&bak);
        std::fs::rename(&path, &bak)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::rename(&bak, &path);
            return Err(e.into());
        }
        let _ = std::fs::remove_file(&bak);
    } else {
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

// --- Share signing key management ---

pub fn share_signing_key_path() -> PathBuf {
    default_data_dir().join("shares").join("signing.key")
}

/// Load the share signing key, or generate and persist a new one.
/// Key format: hex-encoded 32-byte Ed25519 secret (same as `elastos keys generate`).
pub fn load_or_create_share_key() -> anyhow::Result<signature::SigningKey> {
    load_or_create_share_key_at(&share_signing_key_path())
}

pub fn load_or_create_share_key_at(
    path: &std::path::Path,
) -> anyhow::Result<signature::SigningKey> {
    if path.exists() {
        let hex_str = std::fs::read_to_string(path)?;
        let bytes = hex::decode(hex_str.trim())
            .map_err(|e| anyhow::anyhow!("Invalid share signing key: {}", e))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Share signing key must be 32 bytes"))?;
        Ok(signature::SigningKey::from_bytes(&arr))
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (signing_key, verifying_key) = signature::generate_keypair();
        std::fs::write(path, hex::encode(signing_key.to_bytes()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        eprintln!("Generated share signing key: {}", path.display());
        eprintln!("  DID: {}", encode_did_key(&verifying_key)?);
        Ok(signing_key)
    }
}

// --- Provenance attestation (typed structs + domain-separated signing) ---

/// Domain separator for provenance signing — prevents cross-protocol signature reuse
pub const PROVENANCE_DOMAIN: &[u8] = b"elastos.provenance.v1\0";

/// Unsigned provenance — the fields that get signed.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ProvenancePayload {
    pub schema: String,
    pub subject_cid: String,
    pub content_digest: String,
    pub builder_did: String,
    pub built_at: u64,
    pub tool_version: String,
}

/// Signed provenance — payload + signature published through the content provider.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Provenance {
    #[serde(flatten)]
    pub payload: ProvenancePayload,
    pub signature: String,
}

impl ProvenancePayload {
    /// Build the deterministic signing message: domain separator + canonical JSON bytes.
    pub fn signing_bytes(&self) -> anyhow::Result<Vec<u8>> {
        use sha2::Digest;
        let json = serde_json::to_vec(self)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(PROVENANCE_DOMAIN);
        hasher.update(&json);
        Ok(hasher.finalize().to_vec())
    }
}

/// Create a signed provenance attestation for a published CID.
pub fn create_provenance(
    subject_cid: &str,
    content_digest: &str,
    signing_key: &signature::SigningKey,
) -> anyhow::Result<Vec<u8>> {
    let verifying_key = signing_key.verifying_key();
    let builder_did = encode_did_key(&verifying_key)?;

    let payload = ProvenancePayload {
        schema: "elastos.share.provenance/v1".to_string(),
        subject_cid: subject_cid.to_string(),
        content_digest: content_digest.to_string(),
        builder_did,
        built_at: now_unix_secs(),
        tool_version: format!("elastos {}", env!("ELASTOS_VERSION")),
    };

    let digest = payload.signing_bytes()?;
    let sig = signing_key.sign(&digest);

    let prov = Provenance {
        payload,
        signature: hex::encode(sig.to_bytes()),
    };

    Ok(serde_json::to_vec_pretty(&prov)?)
}

/// Verify a provenance attestation. Returns the provenance on success.
/// CID comparison is semantic (parsed via `cid::Cid`) to handle v0/v1 differences.
pub fn verify_provenance(prov_bytes: &[u8], expected_cid: &str) -> anyhow::Result<Provenance> {
    let prov: Provenance = serde_json::from_slice(prov_bytes)?;

    if prov.payload.schema != "elastos.share.provenance/v1" {
        anyhow::bail!("Unknown provenance schema: {}", prov.payload.schema);
    }

    // Semantic CID comparison: parse both to handle CIDv0/v1 representation differences
    let subject_parsed = cid::Cid::try_from(prov.payload.subject_cid.as_str())
        .map_err(|e| anyhow::anyhow!("Invalid subject_cid in provenance: {}", e))?;
    let expected_parsed = cid::Cid::try_from(expected_cid)
        .map_err(|e| anyhow::anyhow!("Invalid expected CID: {}", e))?;
    if subject_parsed != expected_parsed {
        anyhow::bail!(
            "Provenance subject_cid ({}) does not match expected CID ({})",
            prov.payload.subject_cid,
            expected_cid
        );
    }

    // Reconstruct signing message from payload
    let digest = prov.payload.signing_bytes()?;

    let verifying_key = decode_did_key(&prov.payload.builder_did)?;
    let sig_bytes = hex::decode(&prov.signature)
        .map_err(|e| anyhow::anyhow!("Invalid signature hex: {}", e))?;
    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid Ed25519 signature: {}", e))?;

    verifying_key
        .verify_strict(&digest, &sig)
        .map_err(|_| anyhow::anyhow!("Signature verification failed"))?;

    Ok(prov)
}

// --- Channel head (typed structs + domain-separated signing) ---

/// Domain separator for channel head signing — distinct from provenance domain
pub const CHANNEL_HEAD_DOMAIN: &[u8] = b"elastos.channel.head.v1\0";

/// Unsigned channel head — the fields that get signed.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ChannelHeadPayload {
    pub schema: String,
    pub channel: String,
    pub latest_cid: String,
    pub latest_version: u64,
    pub status: ChannelStatus,
    pub updated_at: u64,
    pub signer_did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_head_cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke_reason: Option<String>,
}

/// Signed channel head — payload + signature published through the content provider.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ChannelHead {
    #[serde(flatten)]
    pub payload: ChannelHeadPayload,
    pub signature: String,
}

impl ChannelHeadPayload {
    /// Build the deterministic signing message: domain separator + canonical JSON bytes.
    pub fn signing_bytes(&self) -> anyhow::Result<Vec<u8>> {
        use sha2::Digest;
        let json = serde_json::to_vec(self)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(CHANNEL_HEAD_DOMAIN);
        hasher.update(&json);
        Ok(hasher.finalize().to_vec())
    }
}

/// Create a signed channel head. Returns JSON bytes for IPFS publish.
#[allow(clippy::too_many_arguments)]
pub fn create_channel_head(
    channel: &str,
    latest_cid: &str,
    latest_version: u64,
    status: &ChannelStatus,
    provenance_cid: Option<&str>,
    prev_head_cid: Option<&str>,
    revoke_reason: Option<&str>,
    signing_key: &signature::SigningKey,
) -> anyhow::Result<Vec<u8>> {
    let verifying_key = signing_key.verifying_key();
    let signer_did = encode_did_key(&verifying_key)?;

    let payload = ChannelHeadPayload {
        schema: "elastos.share.head/v1".to_string(),
        channel: channel.to_string(),
        latest_cid: latest_cid.to_string(),
        latest_version,
        status: status.clone(),
        updated_at: now_unix_secs(),
        signer_did,
        provenance_cid: provenance_cid.map(|s| s.to_string()),
        prev_head_cid: prev_head_cid.map(|s| s.to_string()),
        revoke_reason: revoke_reason.map(|s| s.to_string()),
    };

    let digest = payload.signing_bytes()?;
    let sig = signing_key.sign(&digest);

    let head = ChannelHead {
        payload,
        signature: hex::encode(sig.to_bytes()),
    };

    Ok(serde_json::to_vec_pretty(&head)?)
}

/// Verify a signed channel head. Returns the head on success.
/// Validates signature, schema, and embedded CID formats.
/// Trust (signer_did vs expected DID) is the caller's responsibility.
pub fn verify_channel_head(head_bytes: &[u8]) -> anyhow::Result<ChannelHead> {
    let head: ChannelHead = serde_json::from_slice(head_bytes)?;

    if head.payload.schema != "elastos.share.head/v1" {
        anyhow::bail!("Unknown head schema: {}", head.payload.schema);
    }

    // Validate embedded CIDs
    if !is_valid_cid(&head.payload.latest_cid) {
        anyhow::bail!("Invalid latest_cid in head: {}", head.payload.latest_cid);
    }
    if let Some(ref pcid) = head.payload.provenance_cid {
        if !is_valid_cid(pcid) {
            anyhow::bail!("Invalid provenance_cid in head: {}", pcid);
        }
    }
    if let Some(ref phcid) = head.payload.prev_head_cid {
        if !is_valid_cid(phcid) {
            anyhow::bail!("Invalid prev_head_cid in head: {}", phcid);
        }
    }

    // Reconstruct signing message from payload
    let digest = head.payload.signing_bytes()?;

    let verifying_key = decode_did_key(&head.payload.signer_did)?;
    let sig_bytes = hex::decode(&head.signature)
        .map_err(|e| anyhow::anyhow!("Invalid signature hex: {}", e))?;
    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid Ed25519 signature: {}", e))?;

    verifying_key
        .verify_strict(&digest, &sig)
        .map_err(|_| anyhow::anyhow!("Head signature verification failed"))?;

    Ok(head)
}

pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Validate a CID string using the `cid` crate.
pub fn is_valid_cid(s: &str) -> bool {
    cid::Cid::try_from(s).is_ok()
}

/// Create and publish a signed channel head through the content availability provider.
///
/// Returns the head CID if successful, or None if signing/publishing fails with warnings on stderr.
#[allow(clippy::too_many_arguments)]
pub async fn publish_channel_head_via_provider(
    registry: &elastos_runtime::provider::ProviderRegistry,
    channel: &str,
    latest_cid: &str,
    latest_version: u64,
    status: &ChannelStatus,
    provenance_cid: Option<&str>,
    prev_head_cid: Option<&str>,
    revoke_reason: Option<&str>,
    publisher_did: Option<&str>,
    signing_key: &signature::SigningKey,
) -> Option<String> {
    let head_bytes = match create_channel_head(
        channel,
        latest_cid,
        latest_version,
        status,
        provenance_cid,
        prev_head_cid,
        revoke_reason,
        signing_key,
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Warning: head creation failed: {}", e);
            return None;
        }
    };
    match crate::content::publish_bytes_via_provider(
        registry,
        "head.json",
        &head_bytes,
        Some(channel),
        publisher_did,
    )
    .await
    {
        Ok(hcid) => {
            println!("  Head:       elastos://{}", hcid);
            Some(hcid)
        }
        Err(e) => {
            eprintln!("Warning: head publish failed: {}", e);
            None
        }
    }
}

/// Parse a share URI into a bare CID string.
///
/// Supported formats:
/// - `elastos://<cid>`
/// - Bare CID (v0 or v1)
/// - Path-based gateway: `https://ipfs.io/ipfs/<cid>/...`
/// - Subdomain gateway: `https://<cid>.ipfs.dweb.link/...`
pub fn parse_share_uri(uri: &str) -> anyhow::Result<String> {
    let trimmed = uri.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("Empty URI");
    }
    // elastos:// protocol
    if let Some(cid_str) = trimmed.strip_prefix("elastos://") {
        if cid_str.is_empty() {
            anyhow::bail!("Empty CID in elastos:// URI");
        }
        if !is_valid_cid(cid_str) {
            anyhow::bail!("Invalid CID in elastos:// URI: {}", cid_str);
        }
        return Ok(cid_str.to_string());
    }
    // HTTP(S) gateway URLs
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|e| anyhow::anyhow!("Invalid URL: {}: {}", trimmed, e))?;
        // Path-based: https://ipfs.io/ipfs/<cid>/...
        let path = parsed.path();
        if let Some(rest) = path.strip_prefix("/ipfs/") {
            let cid_str = rest.split('/').next().unwrap_or(rest);
            if !cid_str.is_empty() && is_valid_cid(cid_str) {
                return Ok(cid_str.to_string());
            }
        }
        // Subdomain-based: https://<cid>.ipfs.dweb.link/...
        if let Some(host) = parsed.host_str() {
            if let Some(dot_ipfs) = host.find(".ipfs.") {
                let cid_str = &host[..dot_ipfs];
                if !cid_str.is_empty() && is_valid_cid(cid_str) {
                    return Ok(cid_str.to_string());
                }
            }
        }
        anyhow::bail!("URL does not contain a valid CID: {}", uri);
    }
    // Bare CID
    if is_valid_cid(trimmed) {
        return Ok(trimmed.to_string());
    }
    anyhow::bail!(
        "Cannot parse '{}' as a share URI.\n\
         Expected: elastos://<cid>, bare CID, or https://gateway/ipfs/<cid>/",
        uri
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::signature::generate_keypair;

    // Real IPFS CIDs that pass cid crate validation
    const TEST_CIDV0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
    const TEST_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    fn viewer_with_entrypoint(entrypoint: &str) -> tempfile::TempDir {
        let viewer = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(viewer.path().join("browser")).unwrap();
        std::fs::write(viewer.path().join("browser/index.html"), "<html></html>").unwrap();
        std::fs::write(viewer.path().join("browser/app.js"), "export {};").unwrap();
        std::fs::write(
            viewer.path().join("capsule.json"),
            serde_json::json!({ "entrypoint": entrypoint }).to_string(),
        )
        .unwrap();
        viewer
    }

    fn share_input() -> tempfile::TempDir {
        let input = tempfile::tempdir().unwrap();
        std::fs::write(input.path().join("note.md"), "# Note\n").unwrap();
        input
    }

    #[test]
    fn share_bundle_copies_viewer_from_manifest_entrypoint() {
        let viewer = viewer_with_entrypoint("browser/index.html");
        let input = share_input();
        let (bundle, _) =
            build_share_bundle_with_viewer_dir(input.path(), "notes", 1, None, None, viewer.path())
                .unwrap();
        assert!(bundle.path().join("index.html").is_file());
        assert!(bundle.path().join("app.js").is_file());
        assert!(!bundle.path().join("browser").exists());
    }

    #[test]
    fn share_bundle_refuses_entrypoints_outside_the_viewer() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), "secret").unwrap();
        let absolute = outside.path().join("secret.html");
        for entrypoint in [
            absolute.to_str().unwrap(),
            "../secret.html",
            "browser/../../secret.html",
        ] {
            let viewer = viewer_with_entrypoint(entrypoint);
            let input = share_input();
            let err =
                build_share_bundle_with_viewer_dir(input.path(), "n", 1, None, None, viewer.path())
                    .unwrap_err();
            assert!(
                err.to_string().contains("inside the capsule"),
                "{entrypoint}: {err}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn share_bundle_refuses_symlinked_viewer_files() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("id_rsa"), "private").unwrap();

        let asset = viewer_with_entrypoint("browser/index.html");
        std::os::unix::fs::symlink(
            outside.path().join("id_rsa"),
            asset.path().join("browser/leak.js"),
        )
        .unwrap();
        let input = share_input();
        let err =
            build_share_bundle_with_viewer_dir(input.path(), "n", 1, None, None, asset.path())
                .unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");

        let dir_link = viewer_with_entrypoint("web/index.html");
        std::os::unix::fs::symlink(asset.path().join("browser"), dir_link.path().join("web"))
            .unwrap();
        let err =
            build_share_bundle_with_viewer_dir(input.path(), "n", 1, None, None, dir_link.path())
                .unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");

        let entry_link = viewer_with_entrypoint("browser/entry.html");
        std::os::unix::fs::symlink(
            outside.path().join("id_rsa"),
            entry_link.path().join("browser/entry.html"),
        )
        .unwrap();
        let err =
            build_share_bundle_with_viewer_dir(input.path(), "n", 1, None, None, entry_link.path())
                .unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn share_bundle_refuses_directory_swapped_for_symlink_after_listing() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("leak.js"), "private").unwrap();
        let viewer = viewer_with_entrypoint("browser/index.html");
        std::fs::create_dir_all(viewer.path().join("browser/assets")).unwrap();
        std::fs::write(viewer.path().join("browser/assets/leak.js"), "ok").unwrap();

        let entry = open_viewer_entrypoint(viewer.path()).unwrap();
        let mut files = Vec::new();
        list_viewer_files(&entry.web_root, std::path::Path::new(""), &mut files).unwrap();
        assert!(files.contains(&std::path::PathBuf::from("assets/leak.js")));

        // A writer replaces the listed directory with a symlink before the copy.
        std::fs::remove_dir_all(viewer.path().join("browser/assets")).unwrap();
        std::os::unix::fs::symlink(outside.path(), viewer.path().join("browser/assets")).unwrap();

        let bundle = tempfile::tempdir().unwrap();
        let err = copy_listed_viewer_files(&entry.web_root, &files, &entry.name, bundle.path())
            .unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(!bundle.path().join("assets/leak.js").exists());
    }

    #[test]
    fn test_empty_catalog_deserializes() {
        let json = "{}";
        let cat: ShareCatalog = serde_json::from_str(json).unwrap();
        assert!(cat.channels.is_empty());
        assert_eq!(cat.author_did, None);
    }

    #[test]
    fn test_minimal_channel_deserializes() {
        let json = r#"{"channels":{"docs":{"latest_cid":"bafy123"}}}"#;
        let cat: ShareCatalog = serde_json::from_str(json).unwrap();
        let ch = &cat.channels["docs"];
        assert_eq!(ch.latest_cid, "bafy123");
        assert_eq!(ch.status, ChannelStatus::Active);
        assert_eq!(ch.latest_version, 0);
        assert_eq!(ch.revoke_reason, None);
        assert_eq!(ch.author_did, None);
    }

    #[test]
    fn test_full_catalog_roundtrips() {
        let mut cat = ShareCatalog {
            schema: "elastos.share.catalog/v1".to_string(),
            author_did: Some("did:key:z6MkTest".to_string()),
            ..Default::default()
        };
        cat.channels.insert(
            "docs".to_string(),
            ShareChannel {
                latest_cid: "bafy123".to_string(),
                latest_version: 3,
                status: ChannelStatus::Revoked,
                revoke_reason: Some("sensitive".to_string()),
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&cat).unwrap();
        let cat2: ShareCatalog = serde_json::from_str(&json).unwrap();
        assert_eq!(cat2.channels["docs"].status, ChannelStatus::Revoked);
        assert_eq!(
            cat2.channels["docs"].revoke_reason,
            Some("sensitive".to_string())
        );
        assert_eq!(cat2.author_did, Some("did:key:z6MkTest".to_string()));
    }

    #[test]
    fn test_share_entry_with_content_digest() {
        let entry = ShareEntry {
            cid: "bafy123".to_string(),
            version: 1,
            created_at: 1000,
            content_digest: Some("sha256:abc".to_string()),
            provenance_cid: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("content_digest"));
        let entry2: ShareEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry2.content_digest, Some("sha256:abc".to_string()));
    }

    #[test]
    fn test_share_entry_without_content_digest() {
        let json = r#"{"cid":"bafy","version":1,"created_at":1000}"#;
        let entry: ShareEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.content_digest, None);
    }

    #[test]
    fn test_parse_share_uri_elastos() {
        assert_eq!(
            parse_share_uri(&format!("elastos://{}", TEST_CIDV0)).unwrap(),
            TEST_CIDV0
        );
        assert_eq!(
            parse_share_uri(&format!("elastos://{}", TEST_CIDV1)).unwrap(),
            TEST_CIDV1
        );
    }

    #[test]
    fn test_parse_share_uri_path_gateway() {
        assert_eq!(
            parse_share_uri(&format!("https://ipfs.io/ipfs/{}/", TEST_CIDV0)).unwrap(),
            TEST_CIDV0
        );
        assert_eq!(
            parse_share_uri(&format!("https://dweb.link/ipfs/{}", TEST_CIDV1)).unwrap(),
            TEST_CIDV1
        );
    }

    #[test]
    fn test_parse_share_uri_subdomain_gateway() {
        assert_eq!(
            parse_share_uri(&format!("https://{}.ipfs.dweb.link/", TEST_CIDV1)).unwrap(),
            TEST_CIDV1
        );
        assert_eq!(
            parse_share_uri(&format!("https://{}.ipfs.cf-ipfs.com/path", TEST_CIDV1)).unwrap(),
            TEST_CIDV1
        );
    }

    #[test]
    fn test_parse_share_uri_bare_cid() {
        assert_eq!(parse_share_uri(TEST_CIDV0).unwrap(), TEST_CIDV0);
        assert_eq!(parse_share_uri(TEST_CIDV1).unwrap(), TEST_CIDV1);
    }

    #[test]
    fn test_parse_share_uri_invalid() {
        assert!(parse_share_uri("").is_err());
        assert!(parse_share_uri("elastos://").is_err());
        assert!(parse_share_uri("https://example.com/page").is_err());
        assert!(parse_share_uri("not-a-cid").is_err());
        assert!(parse_share_uri("bafyNOTREAL").is_err());
    }

    #[test]
    fn test_derive_share_id() {
        assert_eq!(
            derive_share_id(std::path::Path::new("docs/ARCHITECTURE.md"), None).unwrap(),
            "ARCHITECTURE"
        );
        assert_eq!(
            derive_share_id(std::path::Path::new("docs"), None).unwrap(),
            "docs"
        );
        assert_eq!(
            derive_share_id(std::path::Path::new("docs"), Some("manual-channel")).unwrap(),
            "manual-channel"
        );
    }

    #[test]
    fn test_compute_content_digest_is_order_insensitive() {
        let a = vec![
            ("README.md".to_string(), b"# Hello".to_vec()),
            ("docs/ROADMAP.md".to_string(), b"Plan".to_vec()),
        ];
        let b = vec![
            ("docs/ROADMAP.md".to_string(), b"Plan".to_vec()),
            ("README.md".to_string(), b"# Hello".to_vec()),
        ];

        assert_eq!(
            compute_content_digest(&a).unwrap(),
            compute_content_digest(&b).unwrap()
        );
    }

    #[test]
    fn test_encode_decode_did_key_roundtrip() {
        let (_, vk) = generate_keypair();
        let did = encode_did_key(&vk).unwrap();
        assert!(did.starts_with("did:key:z"));
        let decoded = decode_did_key(&did).unwrap();
        assert_eq!(vk.as_bytes(), decoded.as_bytes());
    }

    #[test]
    fn test_decode_did_key_invalid() {
        assert!(decode_did_key("").is_err());
        assert!(decode_did_key("did:key:z").is_err());
        assert!(decode_did_key("did:key:zNOTBASE58!!!").is_err());
        assert!(decode_did_key("not-a-did").is_err());
    }

    #[test]
    fn test_create_provenance_has_all_fields() {
        let (sk, _) = generate_keypair();
        let prov_bytes = create_provenance("QmTest123", "sha256:abc", &sk).unwrap();
        let prov: serde_json::Value = serde_json::from_slice(&prov_bytes).unwrap();
        assert_eq!(prov["schema"], "elastos.share.provenance/v1");
        assert_eq!(prov["subject_cid"], "QmTest123");
        assert_eq!(prov["content_digest"], "sha256:abc");
        assert!(prov["builder_did"]
            .as_str()
            .unwrap()
            .starts_with("did:key:z"));
        assert!(prov["built_at"].as_u64().unwrap() > 0);
        assert!(prov["tool_version"]
            .as_str()
            .unwrap()
            .starts_with("elastos "));
        assert!(prov["signature"].as_str().is_some());
    }

    #[test]
    fn test_provenance_signature_verifies() {
        let (sk, _) = generate_keypair();
        let prov_bytes = create_provenance(TEST_CIDV0, "sha256:def", &sk).unwrap();
        let prov = verify_provenance(&prov_bytes, TEST_CIDV0).unwrap();
        assert!(prov.payload.builder_did.starts_with("did:key:z"));
    }

    #[test]
    fn test_provenance_wrong_cid_fails() {
        let (sk, _) = generate_keypair();
        let prov_bytes = create_provenance(TEST_CIDV0, "sha256:abc", &sk).unwrap();
        assert!(verify_provenance(&prov_bytes, TEST_CIDV1).is_err());
    }

    #[test]
    fn test_provenance_tampered_payload_fails() {
        let (sk, _) = generate_keypair();
        let prov_bytes = create_provenance(TEST_CIDV0, "sha256:abc", &sk).unwrap();
        let mut prov: Provenance = serde_json::from_slice(&prov_bytes).unwrap();
        prov.payload.content_digest = "sha256:tampered".to_string();
        let tampered = serde_json::to_vec(&prov).unwrap();
        assert!(verify_provenance(&tampered, TEST_CIDV0).is_err());
    }

    #[test]
    fn test_provenance_cid_semantic_comparison() {
        let (sk, _) = generate_keypair();
        let prov_bytes = create_provenance(TEST_CIDV0, "sha256:abc", &sk).unwrap();
        assert!(verify_provenance(&prov_bytes, TEST_CIDV1).is_err());
        assert!(verify_provenance(&prov_bytes, TEST_CIDV0).is_ok());
    }

    #[test]
    fn test_share_entry_provenance_cid_migration() {
        let json = r#"{"cid":"bafy123","version":1,"created_at":1000}"#;
        let entry: ShareEntry = serde_json::from_str(json).unwrap();
        assert!(entry.provenance_cid.is_none());

        let json2 = r#"{"cid":"bafy123","version":1,"created_at":1000,"provenance_cid":"QmProv"}"#;
        let entry2: ShareEntry = serde_json::from_str(json2).unwrap();
        assert_eq!(entry2.provenance_cid.as_deref(), Some("QmProv"));
    }

    #[test]
    fn test_load_or_create_share_key_persists() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test-signing.key");
        let key1 = load_or_create_share_key_at(&key_path).unwrap();
        let key2 = load_or_create_share_key_at(&key_path).unwrap();
        assert_eq!(key1.to_bytes(), key2.to_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn test_share_key_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test-signing-perms.key");
        let _ = load_or_create_share_key_at(&key_path).unwrap();
        let perms = std::fs::metadata(&key_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn test_provenance_domain_separator_prevents_reuse() {
        let (sk, _) = generate_keypair();
        let payload = ProvenancePayload {
            schema: "elastos.share.provenance/v1".to_string(),
            subject_cid: TEST_CIDV0.to_string(),
            content_digest: "sha256:abc".to_string(),
            builder_did: crate::crypto::encode_signing_key_did(&sk),
            built_at: 1000,
            tool_version: "test".to_string(),
        };
        let domain_digest = payload.signing_bytes().unwrap();
        let raw_json = serde_json::to_vec(&payload).unwrap();
        let raw_digest = <sha2::Sha256 as sha2::Digest>::digest(&raw_json).to_vec();
        assert_ne!(domain_digest, raw_digest);
    }

    #[test]
    fn test_channel_head_payload_signing_bytes() {
        let (sk, _) = generate_keypair();
        let payload = ChannelHeadPayload {
            schema: "elastos.share.head/v1".to_string(),
            channel: "docs".to_string(),
            latest_cid: TEST_CIDV0.to_string(),
            latest_version: 1,
            status: ChannelStatus::Active,
            updated_at: 1000,
            signer_did: crate::crypto::encode_signing_key_did(&sk),
            provenance_cid: None,
            prev_head_cid: None,
            revoke_reason: None,
        };
        let domain_digest = payload.signing_bytes().unwrap();
        let raw_json = serde_json::to_vec(&payload).unwrap();
        let raw_digest = <sha2::Sha256 as sha2::Digest>::digest(&raw_json).to_vec();
        assert_ne!(domain_digest, raw_digest);
    }

    #[test]
    fn test_create_channel_head_has_all_fields() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            3,
            &ChannelStatus::Active,
            Some(TEST_CIDV1),
            Some(TEST_CIDV0),
            None,
            &sk,
        )
        .unwrap();
        let head: serde_json::Value = serde_json::from_slice(&head_bytes).unwrap();
        assert_eq!(head["schema"], "elastos.share.head/v1");
        assert_eq!(head["channel"], "docs");
        assert_eq!(head["latest_cid"], TEST_CIDV0);
        assert_eq!(head["latest_version"], 3);
        assert_eq!(head["status"], "active");
        assert!(head["updated_at"].as_u64().unwrap() > 0);
        assert!(head["signer_did"]
            .as_str()
            .unwrap()
            .starts_with("did:key:z"));
        assert_eq!(head["provenance_cid"], TEST_CIDV1);
        assert_eq!(head["prev_head_cid"], TEST_CIDV0);
        assert!(head["signature"].as_str().is_some());
    }

    #[test]
    fn test_channel_head_signature_verifies() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let head = verify_channel_head(&head_bytes).unwrap();
        assert!(head.payload.signer_did.starts_with("did:key:z"));
        assert_eq!(head.payload.channel, "docs");
    }

    #[test]
    fn test_channel_head_tampered_payload_fails() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let mut head: ChannelHead = serde_json::from_slice(&head_bytes).unwrap();
        head.payload.latest_version = 999;
        let tampered = serde_json::to_vec(&head).unwrap();
        assert!(verify_channel_head(&tampered).is_err());
    }

    #[test]
    fn test_channel_head_wrong_signer_fails() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let mut head: ChannelHead = serde_json::from_slice(&head_bytes).unwrap();
        let (sk2, _) = generate_keypair();
        head.payload.signer_did = crate::crypto::encode_signing_key_did(&sk2);
        let modified = serde_json::to_vec(&head).unwrap();
        assert!(verify_channel_head(&modified).is_err());
    }

    #[test]
    fn test_channel_head_revoked_status() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            2,
            &ChannelStatus::Revoked,
            None,
            None,
            Some("sensitive content"),
            &sk,
        )
        .unwrap();
        let head = verify_channel_head(&head_bytes).unwrap();
        assert_eq!(head.payload.status, ChannelStatus::Revoked);
        assert_eq!(
            head.payload.revoke_reason.as_deref(),
            Some("sensitive content")
        );
    }

    #[test]
    fn test_channel_head_archived_status() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            2,
            &ChannelStatus::Archived,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let head = verify_channel_head(&head_bytes).unwrap();
        assert_eq!(head.payload.status, ChannelStatus::Archived);
    }

    #[test]
    fn test_channel_head_invalid_schema_rejected() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let mut head: ChannelHead = serde_json::from_slice(&head_bytes).unwrap();
        head.payload.schema = "wrong.schema/v1".to_string();
        let modified = serde_json::to_vec(&head).unwrap();
        assert!(verify_channel_head(&modified).is_err());
    }

    #[test]
    fn test_channel_head_domain_differs_from_provenance() {
        assert_ne!(CHANNEL_HEAD_DOMAIN, PROVENANCE_DOMAIN);
    }

    #[test]
    fn test_share_channel_head_cid_migration() {
        let json = r#"{"channels":{"docs":{"latest_cid":"bafy123","latest_version":1}}}"#;
        let cat: ShareCatalog = serde_json::from_str(json).unwrap();
        assert_eq!(cat.channels["docs"].head_cid, None);

        let json2 = r#"{"channels":{"docs":{"latest_cid":"bafy123","latest_version":1,"head_cid":"QmHead"}}}"#;
        let cat2: ShareCatalog = serde_json::from_str(json2).unwrap();
        assert_eq!(cat2.channels["docs"].head_cid.as_deref(), Some("QmHead"));
    }

    #[test]
    fn test_channel_head_optional_fields_omitted() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&head_bytes).unwrap();
        assert!(json.get("provenance_cid").is_none());
        assert!(json.get("prev_head_cid").is_none());
        assert!(json.get("revoke_reason").is_none());
    }

    #[test]
    fn test_channel_head_prev_head_chain() {
        let (sk, _) = generate_keypair();
        let head1_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        let head1 = verify_channel_head(&head1_bytes).unwrap();
        assert!(head1.payload.prev_head_cid.is_none());

        let head2_bytes = create_channel_head(
            "docs",
            TEST_CIDV1,
            2,
            &ChannelStatus::Active,
            None,
            Some(TEST_CIDV0),
            None,
            &sk,
        )
        .unwrap();
        let head2 = verify_channel_head(&head2_bytes).unwrap();
        assert_eq!(head2.payload.prev_head_cid.as_deref(), Some(TEST_CIDV0));
    }

    #[test]
    fn test_channel_head_validates_embedded_cids() {
        let (sk, _) = generate_keypair();
        let head_bytes = create_channel_head(
            "docs",
            "not-a-valid-cid",
            1,
            &ChannelStatus::Active,
            None,
            None,
            None,
            &sk,
        )
        .unwrap();
        assert!(verify_channel_head(&head_bytes).is_err());

        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            Some("bad-prov-cid"),
            None,
            None,
            &sk,
        )
        .unwrap();
        assert!(verify_channel_head(&head_bytes).is_err());

        let head_bytes = create_channel_head(
            "docs",
            TEST_CIDV0,
            1,
            &ChannelStatus::Active,
            None,
            Some("bad-prev-cid"),
            None,
            &sk,
        )
        .unwrap();
        assert!(verify_channel_head(&head_bytes).is_err());
    }

    #[test]
    fn test_sign_payload_domain_separation() {
        use crate::crypto::domain_separated_sign;
        let (sk, _) = generate_keypair();
        let payload = b"test payload";
        let (sig_a, _) = domain_separated_sign(&sk, "domain.a", payload);
        let (sig_b, _) = domain_separated_sign(&sk, "domain.b", payload);
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn test_sign_payload_deterministic() {
        use crate::crypto::domain_separated_sign;
        let bytes = [42u8; 32];
        let sk = signature::SigningKey::from_bytes(&bytes);
        let (sig1, _) = domain_separated_sign(&sk, "elastos.release.v1", b"payload");
        let (sig2, _) = domain_separated_sign(&sk, "elastos.release.v1", b"payload");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_sign_payload_output_shape() {
        use crate::crypto::domain_separated_sign;
        let (sk, _) = generate_keypair();
        let (sig_hex, did) = domain_separated_sign(&sk, "test", b"payload");
        assert_eq!(sig_hex.len(), 128);
        assert!(did.starts_with("did:key:z6Mk"));
    }

    #[test]
    fn test_verify_release_envelope_valid() {
        use crate::crypto::verify_release_envelope;
        let (sk, _) = generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&sk);
        let payload = serde_json::json!({
            "schema": "elastos.release.head/v1",
            "version": "0.10.0",
            "latest_release_cid": TEST_CIDV0,
        });
        let envelope = make_test_envelope(&sk, "elastos.release.head.v1", &payload);
        let result = verify_release_envelope(&envelope, "elastos.release.head.v1", &did);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v["payload"]["version"], "0.10.0");
    }

    #[test]
    fn test_verify_release_envelope_wrong_did() {
        use crate::crypto::verify_release_envelope;
        let (sk, _) = generate_keypair();
        let (_, other_vk) = generate_keypair();
        let wrong_did = encode_did_key(&other_vk).unwrap();
        let payload = serde_json::json!({
            "schema": "elastos.release.head/v1",
            "version": "0.10.0",
        });
        let envelope = make_test_envelope(&sk, "elastos.release.head.v1", &payload);
        let result = verify_release_envelope(&envelope, "elastos.release.head.v1", &wrong_did);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Signer DID mismatch"));
    }

    #[test]
    fn test_verify_release_envelope_accepts_any_trusted_did() {
        use crate::crypto::verify_release_envelope_against_dids;
        let (sk, _) = generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&sk);
        let (_, other_vk) = generate_keypair();
        let payload = serde_json::json!({
            "schema": "elastos.release.head/v1",
            "version": "0.10.0",
        });
        let envelope = make_test_envelope(&sk, "elastos.release.head.v1", &payload);
        let result = verify_release_envelope_against_dids(
            &envelope,
            "elastos.release.head.v1",
            &[encode_did_key(&other_vk).unwrap(), did.clone()],
        );
        assert!(result.is_ok());
        let (_, signer) = result.unwrap();
        assert_eq!(signer, did);
    }

    #[test]
    fn test_verify_release_envelope_tampered() {
        use crate::crypto::verify_release_envelope;
        let (sk, _) = generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&sk);
        let payload = serde_json::json!({
            "schema": "elastos.release.head/v1",
            "version": "0.10.0",
        });
        let envelope = make_test_envelope(&sk, "elastos.release.head.v1", &payload);
        let mut v: serde_json::Value = serde_json::from_slice(&envelope).unwrap();
        v["payload"]["version"] = serde_json::json!("0.99.0");
        let tampered = serde_json::to_vec(&v).unwrap();
        let result = verify_release_envelope(&tampered, "elastos.release.head.v1", &did);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
    }

    #[test]
    fn test_verify_release_envelope_wrong_domain() {
        use crate::crypto::verify_release_envelope;
        let (sk, _) = generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&sk);
        let payload = serde_json::json!({
            "schema": "elastos.release/v1",
            "version": "0.10.0",
        });
        let envelope = make_test_envelope(&sk, "elastos.release.v1", &payload);
        let result = verify_release_envelope(&envelope, "elastos.release.head.v1", &did);
        assert!(result.is_err());
    }

    /// Helper: create a signed release envelope for testing.
    fn make_test_envelope(
        sk: &signature::SigningKey,
        domain: &str,
        payload: &serde_json::Value,
    ) -> Vec<u8> {
        use crate::crypto::domain_separated_sign;
        let canonical = serde_json::to_string(payload).unwrap();
        let (sig_hex, did) = domain_separated_sign(sk, domain, canonical.as_bytes());
        let envelope = serde_json::json!({
            "payload": payload,
            "signature": sig_hex,
            "signer_did": did,
        });
        serde_json::to_vec(&envelope).unwrap()
    }
}
