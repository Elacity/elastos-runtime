//! IpfsBridge: typed Rust wrapper around the ipfs-provider wire protocol.
//!
//! All IPFS operations in the runtime go through this bridge. The bridge
//! delegates to a `ProviderBridge` subprocess that speaks JSON over stdin/stdout.

use std::sync::Arc;

use base64::Engine;

use elastos_runtime::provider;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct IpfsStatus {
    #[serde(default)]
    pub api_endpoint: Option<String>,
    #[serde(default)]
    pub gateway_endpoint: Option<String>,
    #[serde(default)]
    pub kubo_pid: Option<u32>,
}

/// Thin wrapper that converts typed Rust calls to ipfs-provider wire protocol JSON.
pub struct IpfsBridge {
    bridge: Arc<provider::ProviderBridge>,
}

impl IpfsBridge {
    /// Create an IpfsBridge from an already-spawned provider bridge.
    pub fn new(bridge: Arc<provider::ProviderBridge>) -> Self {
        Self { bridge }
    }

    // ── Write ops ───────────────────────────────────────────────

    pub async fn add_bytes(&self, content: &[u8], filename: &str) -> anyhow::Result<String> {
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(content);
        let req = serde_json::json!({
            "op": "add_bytes",
            "data": data_b64,
            "filename": filename,
            "pin": true,
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        self.extract_cid(&resp)
    }

    pub async fn add_path(&self, path: &std::path::Path) -> anyhow::Result<String> {
        let req = serde_json::json!({
            "op": "add_path",
            "path": path.to_string_lossy(),
            "pin": true,
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        self.extract_cid(&resp)
    }

    pub async fn add_directory_from_path(&self, dir: &std::path::Path) -> anyhow::Result<String> {
        let mut files = Vec::new();
        collect_files_for_ipfs(dir, dir, &mut files)?;

        if files.is_empty() {
            anyhow::bail!("No files found in {}", dir.display());
        }

        let mut file_entries = Vec::new();
        for rel_path in &files {
            let abs_path = dir.join(rel_path);
            let bytes = std::fs::read(&abs_path)?;
            let data_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            file_entries.push(serde_json::json!({
                "path": rel_path.to_string_lossy().replace('\\', "/"),
                "data": data_b64,
            }));
        }

        let req = serde_json::json!({
            "op": "add_directory",
            "files": file_entries,
            "pin": true,
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        self.extract_cid(&resp)
    }

    // ── Read ops ────────────────────────────────────────────────

    pub async fn cat(&self, cid: &str) -> anyhow::Result<Vec<u8>> {
        let req = serde_json::json!({
            "op": "cat",
            "cid": cid,
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        self.extract_bytes(&resp)
    }

    pub async fn cat_with_path(&self, cid: &str, path: &str) -> anyhow::Result<Vec<u8>> {
        let req = serde_json::json!({
            "op": "cat",
            "cid": cid,
            "path": path,
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        self.extract_bytes(&resp)
    }

    /// Stream CID content directly to a file path (avoids buffering in memory).
    /// Use this for large files like MicroVM rootfs images.
    pub async fn cat_to_path(&self, cid: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        let req = serde_json::json!({
            "op": "cat_to_path",
            "cid": cid,
            "dest": dest.to_string_lossy(),
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;

        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let msg = resp
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("ipfs cat_to_path failed: {}", msg);
            }
        }
        Ok(())
    }

    pub async fn download_directory(
        &self,
        cid: &str,
        dest: &std::path::Path,
    ) -> anyhow::Result<Vec<String>> {
        let req = serde_json::json!({
            "op": "download_directory",
            "cid": cid,
            "dest": dest.to_string_lossy(),
        });
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;

        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let msg = resp
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("ipfs download_directory failed: {}", msg);
            }
        }

        let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
        let files: Vec<String> = data
            .get("files")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(files)
    }

    // ── Lifecycle ───────────────────────────────────────────────

    pub async fn health(&self) -> anyhow::Result<bool> {
        let req = serde_json::json!({"op": "health"});
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
        Ok(data
            .get("healthy")
            .and_then(|h| h.as_bool())
            .unwrap_or(false))
    }

    pub async fn status(&self) -> anyhow::Result<IpfsStatus> {
        let req = serde_json::json!({"op": "status"});
        let resp = self
            .bridge
            .send_raw(&req)
            .await
            .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))?;
        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let msg = resp
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("ipfs-provider error: {}", msg);
            }
        }
        let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
        serde_json::from_value(data)
            .map_err(|e| anyhow::anyhow!("Invalid ipfs-provider status response: {}", e))
    }

    // ── Response helpers ────────────────────────────────────────

    fn extract_cid(&self, resp: &serde_json::Value) -> anyhow::Result<String> {
        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let msg = resp
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("ipfs-provider error: {}", msg);
            }
        }
        resp.get("data")
            .and_then(|d| d.get("cid"))
            .and_then(|c| c.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("No CID in ipfs-provider response"))
    }

    fn extract_bytes(&self, resp: &serde_json::Value) -> anyhow::Result<Vec<u8>> {
        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let msg = resp
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("ipfs-provider error: {}", msg);
            }
        }
        let data_b64 = resp
            .get("data")
            .and_then(|d| d.get("data"))
            .and_then(|d| d.as_str())
            .ok_or_else(|| anyhow::anyhow!("No data in ipfs-provider response"))?;
        base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|e| anyhow::anyhow!("Invalid base64 from ipfs-provider: {}", e))
    }
}

// ── Capsule IPFS operations ──────────────────────────────────────────

/// Prepare a capsule directory from a CID for serving.
///
/// Downloads the manifest and prepares the capsule directory.
/// For MicroVM capsules, this also handles rootfs caching.
pub async fn prepare_capsule_from_cid(
    ipfs: &IpfsBridge,
    cid: &str,
) -> anyhow::Result<std::path::PathBuf> {
    // Fetch manifest via ipfs-provider bridge
    let manifest_bytes = ipfs.cat_with_path(cid, "capsule.json").await?;
    let manifest_data = String::from_utf8(manifest_bytes)
        .map_err(|e| anyhow::anyhow!("Manifest is not valid UTF-8 for CID {}: {}", cid, e))?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid manifest from CID {}: {}", cid, e))?;

    tracing::info!(
        "Loading capsule '{}' ({:?}) from CID",
        manifest.name,
        manifest.capsule_type
    );

    // Create temp directory
    let temp_dir = tempfile::Builder::new()
        .prefix("elastos-capsule-")
        .tempdir()?;

    #[allow(deprecated)]
    let capsule_dir = temp_dir.into_path();

    // Write manifest
    tokio::fs::write(capsule_dir.join("capsule.json"), &manifest_data).await?;

    // Handle MicroVM capsules specially
    if manifest.capsule_type == elastos_common::CapsuleType::MicroVM {
        let microvm = manifest
            .microvm
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("MicroVM capsule missing microvm configuration"))?;

        let rootfs_cid = microvm.rootfs_cid.as_ref().ok_or_else(|| {
            anyhow::anyhow!("MicroVM capsule loaded from CID must specify rootfs_cid")
        })?;

        // Get or create large file cache
        let cache = elastos_storage::LargeFileCache::with_defaults()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create cache: {}", e))?;

        // Check cache for rootfs
        let rootfs_path = if let Some(cached) = cache.get(rootfs_cid).await {
            println!("Using cached rootfs: {}", cached.display());
            cached
        } else {
            println!("Downloading rootfs from IPFS...");
            println!("CID: {}", rootfs_cid);
            if let Some(size) = microvm.rootfs_size {
                println!("Size: {} MB", size / (1024 * 1024));
            }
            println!("This may take a while for large files.");

            let temp_path = cache.temp_path(rootfs_cid);
            ipfs.cat_to_path(rootfs_cid, &temp_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to download rootfs: {}", e))?;

            let metadata = tokio::fs::metadata(&temp_path).await?;
            cache
                .register(rootfs_cid, metadata.len())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to cache rootfs: {}", e))?
        };

        // Create symlink to cached rootfs
        let rootfs_link = capsule_dir.join(&manifest.entrypoint);
        #[cfg(unix)]
        tokio::fs::symlink(&rootfs_path, &rootfs_link).await?;
        #[cfg(not(unix))]
        {
            println!("Copying rootfs (symlinks not supported on this platform)...");
            tokio::fs::copy(&rootfs_path, &rootfs_link).await?;
        }

        // Handle kernel if specified
        if let (Some(ref kernel_cid), Some(ref kernel_name)) =
            (&microvm.kernel_cid, &microvm.kernel)
        {
            let kernel_path = if let Some(cached) = cache.get(kernel_cid).await {
                cached
            } else {
                println!("Downloading kernel...");
                let temp_path = cache.temp_path(kernel_cid);
                ipfs.cat_to_path(kernel_cid, &temp_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to download kernel: {}", e))?;

                let metadata = tokio::fs::metadata(&temp_path).await?;
                cache
                    .register(kernel_cid, metadata.len())
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to cache kernel: {}", e))?
            };

            let kernel_link = capsule_dir.join(kernel_name);
            if let Some(parent) = kernel_link.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            #[cfg(unix)]
            tokio::fs::symlink(&kernel_path, &kernel_link).await?;
            #[cfg(not(unix))]
            tokio::fs::copy(&kernel_path, &kernel_link).await?;
        }

        println!("MicroVM capsule prepared at: {}", capsule_dir.display());
    } else if manifest.capsule_type == elastos_common::CapsuleType::Data {
        // Data capsules are multi-file bundles — download all files from the IPFS directory
        ipfs.download_directory(cid, &capsule_dir).await?;
        let entrypoint_path = capsule_dir.join(&manifest.entrypoint);
        if !entrypoint_path.is_file() {
            anyhow::bail!(
                "Data capsule entrypoint '{}' missing after download from CID {}",
                manifest.entrypoint,
                cid
            );
        }
    } else {
        // For WASM capsules, download just the entrypoint
        let entrypoint_bytes = ipfs.cat_with_path(cid, &manifest.entrypoint).await?;
        tokio::fs::write(capsule_dir.join(&manifest.entrypoint), &entrypoint_bytes).await?;
    }

    Ok(capsule_dir)
}

/// Publish a MicroVM capsule to IPFS.
///
/// MicroVM capsules require special handling because their rootfs files are
/// typically 2GB+ and need to be uploaded via streaming. The manifest is
/// updated with CID references to the uploaded files.
pub async fn publish_microvm_capsule(
    path: &std::path::Path,
    manifest: &mut elastos_common::CapsuleManifest,
    ipfs: &IpfsBridge,
) -> anyhow::Result<()> {
    println!("Publishing MicroVM capsule '{}' to IPFS...", manifest.name);
    println!("This requires ipfs-provider for upload of large files.");

    // Check IPFS availability
    if !ipfs.health().await.unwrap_or(false) {
        anyhow::bail!(
            "ipfs-provider not healthy.\n\
             Publishing MicroVM capsules requires a running IPFS daemon.\n\
             Start it with: ipfs daemon"
        );
    }

    // Upload rootfs
    let rootfs_path = path.join(&manifest.entrypoint);
    let rootfs_metadata = std::fs::metadata(&rootfs_path)?;
    let rootfs_size = rootfs_metadata.len();

    println!("\nUploading rootfs ({} MB)...", rootfs_size / (1024 * 1024));

    let rootfs_cid = ipfs
        .add_path(&rootfs_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to upload rootfs: {}", e))?;

    println!("Rootfs uploaded: {}", rootfs_cid);

    // Upload kernel if specified
    let kernel_cid = {
        let microvm = manifest.microvm.get_or_insert_with(Default::default);
        if let Some(ref kernel_name) = microvm.kernel {
            let kernel_path = path.join(kernel_name);
            if kernel_path.exists() {
                println!("\nUploading kernel...");

                let cid = ipfs
                    .add_path(&kernel_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to upload kernel: {}", e))?;

                println!("Kernel uploaded: {}", cid);
                Some(cid)
            } else {
                None
            }
        } else {
            None
        }
    };

    // Update manifest with CID references
    {
        let microvm = manifest.microvm.get_or_insert_with(Default::default);
        microvm.rootfs_cid = Some(rootfs_cid.clone());
        microvm.rootfs_size = Some(rootfs_size);
        if let Some(ref cid) = kernel_cid {
            microvm.kernel_cid = Some(cid.clone());
        }
    }

    // Create updated manifest JSON and upload as wrapped directory via bridge.
    // Must use add_directory (not add_bytes) so the CID has directory semantics —
    // prepare_capsule_from_cid fetches cid/capsule.json, which requires a directory CID.
    let updated_manifest = serde_json::to_string_pretty(manifest)?;

    println!("\nUploading manifest...");

    let tmp_dir = tempfile::Builder::new()
        .prefix("elastos-microvm-pub-")
        .tempdir()?;
    std::fs::write(tmp_dir.path().join("capsule.json"), &updated_manifest)?;
    let cid = ipfs.add_directory_from_path(tmp_dir.path()).await?;

    println!("\n========================================");
    println!("MicroVM capsule published successfully!");
    println!("========================================");
    println!();
    println!("Capsule CID: {}", cid);
    println!("Rootfs CID:  {}", rootfs_cid);
    if let Some(ref k) = kernel_cid {
        println!("Kernel CID:  {}", k);
    }
    println!();
    println!("Run with:");
    println!("  sudo elastos serve --cid {} --forward 4100:4100", cid);
    println!();
    println!(
        "Note: First run will download the rootfs ({} MB) from IPFS.",
        rootfs_size / (1024 * 1024)
    );
    println!("Subsequent runs will use the cached rootfs.");

    Ok(())
}

/// Find a viewer capsule directory by name from the installed runtime only.
fn viewer_root_is_valid(dir: &std::path::Path) -> bool {
    dir.join("capsule.json").exists() || dir.join("index.html").exists()
}

pub fn find_viewer_dir(name: &str) -> anyhow::Result<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let from_share = exe_dir.join("../share/elastos/capsules").join(name);
            if viewer_root_is_valid(&from_share) {
                return Ok(from_share);
            }
        }
    }

    // Standard install location: $XDG_DATA_HOME/elastos/capsules/<name>
    let installed = crate::sources::default_data_dir()
        .join("capsules")
        .join(name);
    if viewer_root_is_valid(&installed) {
        return Ok(installed);
    }

    anyhow::bail!(
        "Viewer '{}' not installed.\n\n\
         Run first:\n\n\
         \x20 elastos setup --with md-viewer\n\n\
         Then try again.",
        name
    )
}

// ── File collection helpers ─────────────────────────────────────────

/// Recursively collect relative file paths under `base`, skipping hidden files
/// and common build artifacts.
pub fn collect_files_for_ipfs(
    base: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files_for_ipfs(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| anyhow::anyhow!("path strip error: {}", e))?;
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::viewer_root_is_valid;

    #[test]
    fn viewer_root_accepts_index_html_only_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let viewer = tmp.path().join("capsules/md-viewer");
        std::fs::create_dir_all(&viewer).unwrap();
        std::fs::write(viewer.join("index.html"), "<html></html>").unwrap();

        assert!(viewer_root_is_valid(&viewer));
        assert!(!viewer_root_is_valid(&tmp.path().join("capsules/missing")));
    }
}
