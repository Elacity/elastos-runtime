//! crosvm compute provider implementation

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::{
    CapsuleId, CapsuleManifest, CapsuleStatus, CapsuleType, ElastosError, Result,
};
use elastos_compute::{CapsuleHandle, CapsuleInfo, ComputeProvider};

use crate::config::{CrosvmConfig, VmConfig};
use crate::network::NetworkConfig;
use crate::rootfs::RootfsManager;
use crate::vm::RunningVm;

/// crosvm compute provider for running MicroVM capsules
pub struct CrosvmProvider {
    /// Configuration
    config: CrosvmConfig,

    /// Running VMs indexed by capsule ID
    vms: Arc<RwLock<HashMap<CapsuleId, RunningVm>>>,

    /// Rootfs manager for overlays
    rootfs_manager: RootfsManager,
}

impl CrosvmProvider {
    /// Create a new crosvm provider
    pub fn new(config: CrosvmConfig) -> Result<Self> {
        config
            .validate()
            .map_err(|e| ElastosError::Compute(e.to_string()))?;

        let rootfs_manager = RootfsManager::new(&config.rootfs_cache_dir);

        Ok(Self {
            config,
            vms: Arc::new(RwLock::new(HashMap::new())),
            rootfs_manager,
        })
    }

    /// Create a provider with default config (will fail if crosvm not installed)
    pub fn with_defaults() -> Result<Self> {
        Self::new(CrosvmConfig::default())
    }

    /// Initialize the provider (create directories, etc.)
    pub async fn init(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.config.socket_dir)
            .await
            .map_err(|e| ElastosError::Compute(format!("Failed to create socket dir: {}", e)))?;

        self.rootfs_manager.init().await?;

        Ok(())
    }

    /// Get the socket path for a VM
    fn socket_path(&self, capsule_id: &CapsuleId) -> std::path::PathBuf {
        self.config
            .socket_dir
            .join(format!("{}.sock", capsule_id.0))
    }
}

#[async_trait]
impl ComputeProvider for CrosvmProvider {
    async fn load(&self, path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
        if manifest.capsule_type != CapsuleType::MicroVM {
            return Err(ElastosError::Compute(format!(
                "CrosvmProvider only supports MicroVM capsules, got: {:?}",
                manifest.capsule_type
            )));
        }

        let rootfs_path = path.join(&manifest.entrypoint);
        if !rootfs_path.exists() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "Rootfs not found: {}",
                rootfs_path.display()
            )));
        }

        let vm_config = VmConfig::from_manifest(&manifest, path, &self.config.kernel_path);

        if !vm_config.kernel_path.exists() {
            return Err(ElastosError::Compute(format!(
                "Kernel not found: {}",
                vm_config.kernel_path.display()
            )));
        }

        let id = CapsuleId::new(format!("microvm-{}", uuid::Uuid::new_v4()));
        let socket_path = self.socket_path(&id);

        let overlay_path = self
            .rootfs_manager
            .get_or_create_overlay(&vm_config.vm_id, &rootfs_path)
            .await?;

        let mut vm_config = vm_config;
        vm_config.rootfs_path = overlay_path;

        if let Some(size_mb) = manifest
            .microvm
            .as_ref()
            .and_then(|m| m.persistent_storage_mb)
        {
            if size_mb > 0 {
                let data_path = self
                    .rootfs_manager
                    .get_or_create_data_disk(&manifest.name, size_mb)
                    .await?;
                vm_config.data_disk_path = Some(data_path);
            }
        }

        let vm = RunningVm::new(vm_config, manifest.clone(), socket_path);

        self.vms.write().await.insert(id.clone(), vm);

        tracing::info!("Loaded MicroVM capsule '{}' with ID {}", manifest.name, id);

        Ok(CapsuleHandle {
            id,
            manifest,
            args: vec![],
        })
    }

    async fn start(&self, handle: &CapsuleHandle) -> Result<()> {
        let mut vms = self.vms.write().await;

        let vm = vms
            .get_mut(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        vm.start(&self.config.crosvm_bin).await?;

        Ok(())
    }

    async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        let mut vms = self.vms.write().await;

        if let Some(vm) = vms.get_mut(&handle.id) {
            vm.stop().await?;

            self.rootfs_manager.remove_overlay(&vm.config.vm_id).await?;
        }

        Ok(())
    }

    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus> {
        let vms = self.vms.read().await;

        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        if vm.is_running() {
            Ok(CapsuleStatus::Running)
        } else {
            Ok(vm.status)
        }
    }

    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
        let vms = self.vms.read().await;

        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        Ok(CapsuleInfo {
            id: handle.id.clone(),
            name: vm.manifest.name.clone(),
            status: vm.status,
            memory_used_mb: vm.config.mem_size_mib,
        })
    }

    fn supports(&self, capsule_type: &CapsuleType) -> bool {
        matches!(capsule_type, CapsuleType::MicroVM)
    }
}

impl CrosvmProvider {
    /// Configure session credentials for a VM before starting it
    pub async fn set_session_for_vm(
        &self,
        capsule_id: &CapsuleId,
        token: &str,
        api_addr: &str,
    ) -> Result<()> {
        let mut vms = self.vms.write().await;

        let vm = vms
            .get_mut(capsule_id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(capsule_id.0.clone()))?;

        vm.config = vm.config.clone().with_session(token, api_addr);

        tracing::info!(
            "Configured session for VM {}: token={}..., api={}",
            capsule_id,
            &token[..8.min(token.len())],
            api_addr
        );

        Ok(())
    }

    /// Attach explicit guest-network TAP for a VM before start.
    pub async fn set_network_for_vm(
        &self,
        capsule_id: &CapsuleId,
        network: NetworkConfig,
    ) -> Result<()> {
        let mut vms = self.vms.write().await;

        let vm = vms
            .get_mut(capsule_id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(capsule_id.0.clone()))?;

        vm.config.network = Some(network.clone());

        tracing::info!(
            "Configured guest-network TAP for VM {}: host={} guest={}",
            capsule_id,
            network.host_ip,
            network.guest_ip
        );

        Ok(())
    }

    /// Get the network config for a VM (if TAP was configured).
    pub async fn get_network_for_vm(&self, capsule_id: &CapsuleId) -> Option<NetworkConfig> {
        let vms = self.vms.read().await;
        vms.get(capsule_id).and_then(|vm| vm.config.network.clone())
    }

    /// Append boot arguments to a VM before start.
    pub async fn append_boot_args_for_vm(&self, capsule_id: &CapsuleId, args: &str) -> Result<()> {
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(capsule_id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(capsule_id.0.clone()))?;
        vm.config.boot_args = format!("{} {}", vm.config.boot_args, args);
        Ok(())
    }

    /// Get the VM ID for a capsule (needed for session creation)
    pub async fn get_vm_id(&self, capsule_id: &CapsuleId) -> Option<String> {
        let vms = self.vms.read().await;
        vms.get(capsule_id).map(|vm| vm.config.vm_id.clone())
    }

    /// Get the HTTP port configured for a VM (if any)
    pub async fn http_port(&self, handle: &CapsuleHandle) -> Result<Option<u16>> {
        let vms = self.vms.read().await;
        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;
        Ok(vm.http_port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crosvm_provider_supports() {
        assert!(matches!(CapsuleType::MicroVM, CapsuleType::MicroVM));
        assert!(!matches!(CapsuleType::Wasm, CapsuleType::MicroVM));
        assert!(!matches!(CapsuleType::Oci, CapsuleType::MicroVM));
    }
}
