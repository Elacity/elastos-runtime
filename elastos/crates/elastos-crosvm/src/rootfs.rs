//! Rootfs caching and overlay management

use std::path::{Path, PathBuf};

use elastos_common::{ElastosError, Result};

/// Manages rootfs images and overlays
pub struct RootfsManager {
    /// Directory for cached rootfs images
    cache_dir: PathBuf,

    /// Directory for VM-specific overlays
    overlay_dir: PathBuf,
}

impl RootfsManager {
    /// Create a new rootfs manager
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        let cache_dir = cache_dir.into();
        let overlay_dir = cache_dir.join("overlays");

        Self {
            cache_dir,
            overlay_dir,
        }
    }

    /// Initialize the rootfs manager (create directories)
    pub async fn init(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.cache_dir)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to create cache dir: {}", e)))?;

        tokio::fs::create_dir_all(&self.overlay_dir)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to create overlay dir: {}", e)))?;

        Ok(())
    }

    /// Get or create an overlay for a VM
    ///
    /// For now, this just copies the base rootfs to create a writable copy.
    /// In the future, this could use device-mapper snapshots or overlayfs.
    pub async fn get_or_create_overlay(&self, vm_id: &str, base_rootfs: &Path) -> Result<PathBuf> {
        let overlay_path = self.overlay_dir.join(format!("{}.ext4", vm_id));

        if overlay_path.exists() {
            tracing::debug!("Using existing overlay: {}", overlay_path.display());
            return Ok(overlay_path);
        }

        tracing::info!(
            "Creating rootfs overlay for VM '{}' from: {}",
            vm_id,
            base_rootfs.display()
        );

        // Create a writable copy of the base rootfs. Uses a reflink (CoW) clone
        // when the cache filesystem supports it — an O(1) metadata op instead of
        // a full byte copy of the (hundreds-of-MB) image — and transparently
        // falls back to a full copy otherwise.
        reflink_or_copy(base_rootfs, &overlay_path)
            .await
            .map_err(|e| {
                ElastosError::Storage(format!(
                    "Failed to create rootfs overlay: {} -> {}: {}",
                    base_rootfs.display(),
                    overlay_path.display(),
                    e
                ))
            })?;

        Ok(overlay_path)
    }

    /// Remove an overlay for a VM
    pub async fn remove_overlay(&self, vm_id: &str) -> Result<()> {
        let overlay_path = self.overlay_dir.join(format!("{}.ext4", vm_id));

        if overlay_path.exists() {
            tokio::fs::remove_file(&overlay_path)
                .await
                .map_err(|e| ElastosError::Storage(format!("Failed to remove overlay: {}", e)))?;
        }

        Ok(())
    }

    /// Get or create a persistent data disk for a capsule.
    ///
    /// The disk is a sparse ext4 file that survives VM restarts.
    /// Stored in `{cache_dir}/data-disks/{capsule_name}-data.ext4`.
    pub async fn get_or_create_data_disk(
        &self,
        capsule_name: &str,
        size_mb: u32,
    ) -> Result<PathBuf> {
        let data_dir = self.cache_dir.join("data-disks");
        tokio::fs::create_dir_all(&data_dir).await.map_err(|e| {
            ElastosError::Storage(format!("Failed to create data-disks dir: {}", e))
        })?;

        let disk_path = data_dir.join(format!("{}-data.ext4", capsule_name));

        if disk_path.exists() {
            tracing::info!(
                "Reusing existing data disk: {} ({}MB)",
                disk_path.display(),
                size_mb
            );
            return Ok(disk_path);
        }

        tracing::info!(
            "Creating data disk for '{}': {} ({}MB sparse)",
            capsule_name,
            disk_path.display(),
            size_mb
        );

        // Create sparse file with truncate
        let size_bytes = (size_mb as u64) * 1024 * 1024;
        let file = tokio::fs::File::create(&disk_path)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to create data disk: {}", e)))?;
        file.set_len(size_bytes)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to set data disk size: {}", e)))?;
        drop(file);

        // Format as ext4
        let output = tokio::process::Command::new("mkfs.ext4")
            .args(["-F", "-q"])
            .arg(&disk_path)
            .output()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to run mkfs.ext4: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up failed disk
            let _ = tokio::fs::remove_file(&disk_path).await;
            return Err(ElastosError::Storage(format!(
                "mkfs.ext4 failed: {}",
                stderr
            )));
        }

        Ok(disk_path)
    }

    /// Get the cache directory path
    #[cfg(test)]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Get the overlay directory path
    #[cfg(test)]
    pub fn overlay_dir(&self) -> &Path {
        &self.overlay_dir
    }
}

/// Create `dst` as a writable, fully-independent copy of `src`.
///
/// Attempts a reflink (copy-on-write) clone first via `cp --reflink=always`,
/// which is an O(1) metadata operation on CoW filesystems (btrfs, xfs, zfs,
/// bcachefs). On any failure — a non-CoW filesystem, a cross-device
/// destination, or `cp` being unavailable — it falls back to a full byte copy.
/// Either path yields an independent writable file with identical contents, so
/// the caller's semantics are unchanged; only the cost differs. Used for VM
/// rootfs overlays, which are ~hundreds of MB copied on every launch.
pub async fn reflink_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let reflinked = tokio::process::Command::new("cp")
        .arg("--reflink=always")
        .arg("-f")
        .arg("--")
        .arg(src)
        .arg(dst)
        .output()
        .await
        .map(|out| out.status.success())
        .unwrap_or(false);
    if reflinked {
        return Ok(());
    }
    // Reflink unavailable/failed: remove any partial file the clone may have
    // left, then fall back to a guaranteed full byte copy.
    let _ = tokio::fs::remove_file(dst).await;
    tokio::fs::copy(src, dst).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reflink_or_copy_produces_independent_identical_file() {
        // Exercises whichever path the host filesystem supports (reflink or the
        // byte-copy fallback); both must yield an independent file with
        // identical contents, and mutating the copy must not touch the source.
        let temp = tempdir().unwrap();
        let src = temp.path().join("base.bin");
        let dst = temp.path().join("overlay.bin");
        let payload = vec![0xABu8; 1024 * 64];
        tokio::fs::write(&src, &payload).await.unwrap();

        reflink_or_copy(&src, &dst).await.unwrap();

        assert_eq!(tokio::fs::read(&dst).await.unwrap(), payload);
        // Independence: writing the destination leaves the source unchanged.
        tokio::fs::write(&dst, vec![0x00u8; 32]).await.unwrap();
        assert_eq!(tokio::fs::read(&src).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn test_rootfs_manager_init() {
        let temp = tempdir().unwrap();
        let manager = RootfsManager::new(temp.path().join("cache"));

        manager.init().await.unwrap();

        assert!(manager.cache_dir().exists());
        assert!(manager.overlay_dir().exists());
    }

    // Shells out to `mkfs.ext4`, which is part of e2fsprogs on Linux. macOS and
    // Windows hosts do not ship it, so the test is scoped to Linux. The
    // production code that calls mkfs.ext4 is itself only reached on Linux via
    // the /dev/kvm fail-closed path in vm.rs::start().
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_data_disk_creation() {
        if std::process::Command::new("mkfs.ext4")
            .arg("-V")
            .output()
            .is_err()
        {
            eprintln!("skipping data disk creation test: mkfs.ext4 unavailable");
            return;
        }
        let temp = tempdir().unwrap();
        let manager = RootfsManager::new(temp.path().join("cache"));
        manager.init().await.unwrap();

        let disk_path = manager
            .get_or_create_data_disk("test-capsule", 16)
            .await
            .unwrap();

        // Verify file exists at expected path
        assert!(disk_path.exists());
        assert_eq!(
            disk_path,
            temp.path().join("cache/data-disks/test-capsule-data.ext4")
        );

        // Verify sparse file (logical size = 16MB)
        let metadata = std::fs::metadata(&disk_path).unwrap();
        assert_eq!(metadata.len(), 16 * 1024 * 1024);

        // Calling again reuses existing disk
        let disk_path2 = manager
            .get_or_create_data_disk("test-capsule", 16)
            .await
            .unwrap();
        assert_eq!(disk_path, disk_path2);
    }
}
