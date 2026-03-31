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

        // For simplicity, create a copy of the rootfs
        // This is inefficient but works for development
        tokio::fs::copy(base_rootfs, &overlay_path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_rootfs_manager_init() {
        let temp = tempdir().unwrap();
        let manager = RootfsManager::new(temp.path().join("cache"));

        manager.init().await.unwrap();

        assert!(manager.cache_dir().exists());
        assert!(manager.overlay_dir().exists());
    }

    #[tokio::test]
    async fn test_data_disk_creation() {
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
