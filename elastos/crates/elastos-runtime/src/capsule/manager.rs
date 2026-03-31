//! Capsule lifecycle manager
//!
//! Tracks all running capsules and integrates with:
//! - Capability tokens (for secure resource access)
//! - Metrics (for rate limiting and monitoring)
//! - Audit logging (for security and compliance)
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use elastos_common::{CapsuleManifest, ElastosError, Result};
use elastos_compute::{CapsuleHandle, ComputeProvider};

use crate::capability::CapabilityManager;
use crate::primitives::audit::{AuditLog, StopReason, TrustLevel};
use crate::primitives::metrics::MetricsManager;

/// Unique identifier for a capsule instance
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapsuleId(String);

impl CapsuleId {
    /// Generate a new random capsule ID
    pub fn new() -> Self {
        Self(format!("cap-{}", Uuid::new_v4().as_simple()))
    }

    /// Create from string (for deserialization)
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CapsuleId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CapsuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Capsule runtime state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapsuleState {
    /// Capsule is starting up
    Starting,
    /// Capsule is running normally
    Running,
    /// Capsule is suspended (paused)
    Suspended,
    /// Capsule is stopping
    Stopping,
    /// Capsule has stopped
    Stopped,
}

/// Information about a running capsule
#[derive(Debug)]
pub struct CapsuleInfo {
    /// Unique instance identifier
    pub id: CapsuleId,
    /// Capsule manifest
    pub manifest: CapsuleManifest,
    /// Current state
    pub state: CapsuleState,
    /// Content identifier (IPFS CID) if loaded from network
    pub cid: Option<String>,
    /// Trust level based on signature verification
    pub trust_level: TrustLevel,
    /// Handle to the compute instance
    pub handle: CapsuleHandle,
}

/// Manages all capsule instances
pub struct CapsuleManager {
    /// Running capsules by ID
    capsules: RwLock<HashMap<CapsuleId, CapsuleInfo>>,
    /// Compute provider for WASM execution
    compute: Arc<dyn ComputeProvider>,
    /// Capability manager for token operations
    capability_manager: Arc<CapabilityManager>,
    /// Metrics manager for tracking
    metrics: Arc<MetricsManager>,
    /// Audit log for security events
    audit_log: Arc<AuditLog>,
}

impl CapsuleManager {
    /// Create a new capsule manager
    pub fn new(
        compute: Arc<dyn ComputeProvider>,
        capability_manager: Arc<CapabilityManager>,
        metrics: Arc<MetricsManager>,
        audit_log: Arc<AuditLog>,
    ) -> Self {
        Self {
            capsules: RwLock::new(HashMap::new()),
            compute,
            capability_manager,
            metrics,
            audit_log,
        }
    }

    /// Launch a capsule from a local path
    pub async fn launch_local(
        &self,
        path: &Path,
        manifest: CapsuleManifest,
        trust_level: TrustLevel,
    ) -> Result<CapsuleId> {
        self.launch_internal(path, manifest, None, trust_level)
            .await
    }

    /// Launch a capsule from IPFS (path is temp directory with downloaded files)
    pub async fn launch_from_cid(
        &self,
        path: &Path,
        manifest: CapsuleManifest,
        cid: String,
        trust_level: TrustLevel,
    ) -> Result<CapsuleId> {
        self.launch_internal(path, manifest, Some(cid), trust_level)
            .await
    }

    /// Internal launch implementation
    async fn launch_internal(
        &self,
        path: &Path,
        manifest: CapsuleManifest,
        cid: Option<String>,
        trust_level: TrustLevel,
    ) -> Result<CapsuleId> {
        // Check if compute provider supports this type
        if !self.compute.supports(&manifest.capsule_type) {
            return Err(ElastosError::Compute(format!(
                "Unsupported capsule type: {:?}",
                manifest.capsule_type
            )));
        }

        // Generate unique ID
        let capsule_id = CapsuleId::new();

        // Start metrics tracking
        self.metrics.start_capsule(capsule_id.as_str());

        // Load the capsule
        let handle = self.compute.load(path, manifest.clone()).await?;

        // Create capsule info
        let info = CapsuleInfo {
            id: capsule_id.clone(),
            manifest: manifest.clone(),
            state: CapsuleState::Starting,
            cid: cid.clone(),
            trust_level,
            handle,
        };

        // Register capsule
        {
            let mut capsules = self.capsules.write().await;
            capsules.insert(capsule_id.clone(), info);
        }

        // Start the capsule — clean up on failure to prevent resource leak
        {
            let capsules = self.capsules.read().await;
            if let Some(info) = capsules.get(&capsule_id) {
                if let Err(e) = self.compute.start(&info.handle).await {
                    drop(capsules); // release read lock before write
                    let mut capsules = self.capsules.write().await;
                    capsules.remove(&capsule_id);
                    self.metrics.stop_capsule(capsule_id.as_str());
                    return Err(e);
                }
            }
        }

        // Update state to running
        {
            let mut capsules = self.capsules.write().await;
            if let Some(info) = capsules.get_mut(&capsule_id) {
                info.state = CapsuleState::Running;
            }
        }

        // Emit audit event
        self.audit_log.capsule_launch(
            capsule_id.as_str(),
            &manifest.name,
            cid.as_deref(),
            trust_level,
        );

        tracing::info!(
            "Launched capsule '{}' with ID: {}",
            manifest.name,
            capsule_id
        );

        Ok(capsule_id)
    }

    /// Stop a running capsule
    pub async fn stop(&self, capsule_id: &CapsuleId, reason: StopReason) -> Result<()> {
        // Update state to stopping
        let handle = {
            let mut capsules = self.capsules.write().await;
            match capsules.get_mut(capsule_id) {
                Some(info) => {
                    info.state = CapsuleState::Stopping;
                    info.handle.clone()
                }
                None => {
                    return Err(ElastosError::CapsuleNotFound(format!(
                        "Capsule not found: {}",
                        capsule_id
                    )));
                }
            }
        };

        // Stop the compute instance
        self.compute.stop(&handle).await?;

        // Memory clearing: WasmProvider::stop() drops the RunningInstance
        // (Engine + Module + compiled code), releasing all WASM linear memory.
        // CapsuleManager removes per-capsule metrics below.

        // Update state and cleanup
        {
            let mut capsules = self.capsules.write().await;
            if let Some(info) = capsules.get_mut(capsule_id) {
                info.state = CapsuleState::Stopped;
            }
        }

        // Stop metrics tracking and get final metrics
        let _final_metrics = self.metrics.stop_capsule(capsule_id.as_str());

        // Emit audit event
        self.audit_log.capsule_stop(capsule_id.as_str(), reason);

        tracing::info!("Stopped capsule: {}", capsule_id);

        Ok(())
    }

    /// Get information about a capsule
    pub async fn get(&self, capsule_id: &CapsuleId) -> Option<CapsuleInfo> {
        let capsules = self.capsules.read().await;
        capsules.get(capsule_id).map(|info| CapsuleInfo {
            id: info.id.clone(),
            manifest: info.manifest.clone(),
            state: info.state,
            cid: info.cid.clone(),
            trust_level: info.trust_level,
            handle: info.handle.clone(),
        })
    }

    /// List all capsule IDs
    pub async fn list(&self) -> Vec<CapsuleId> {
        let capsules = self.capsules.read().await;
        capsules.keys().cloned().collect()
    }

    /// List all running capsules
    pub async fn list_running(&self) -> Vec<CapsuleId> {
        let capsules = self.capsules.read().await;
        capsules
            .iter()
            .filter(|(_, info)| info.state == CapsuleState::Running)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get the capability manager (for granting tokens to capsules)
    pub fn capability_manager(&self) -> &Arc<CapabilityManager> {
        &self.capability_manager
    }

    /// Get the metrics manager
    pub fn metrics(&self) -> &Arc<MetricsManager> {
        &self.metrics
    }

    /// Clean up stopped capsules
    pub async fn cleanup_stopped(&self) {
        let mut capsules = self.capsules.write().await;
        capsules.retain(|_, info| info.state != CapsuleState::Stopped);
    }

    /// Get capsule count by state
    pub async fn count_by_state(&self) -> HashMap<CapsuleState, usize> {
        let capsules = self.capsules.read().await;
        let mut counts = HashMap::new();
        for info in capsules.values() {
            *counts.entry(info.state).or_insert(0) += 1;
        }
        counts
    }

    /// Stop all running capsules (for shutdown)
    pub async fn stop_all(&self, reason: StopReason) {
        let ids: Vec<CapsuleId> = self.list_running().await;
        for id in ids {
            if let Err(e) = self.stop(&id, reason.clone()).await {
                tracing::error!("Failed to stop capsule {}: {}", id, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityStore;
    use elastos_common::{
        CapsuleId as CommonCapsuleId, CapsuleStatus, CapsuleType, Permissions, ResourceLimits,
    };
    use elastos_compute::CapsuleInfo as ComputeCapsuleInfo;

    // Mock compute provider for testing
    struct MockComputeProvider;

    #[async_trait::async_trait]
    impl ComputeProvider for MockComputeProvider {
        fn supports(&self, _capsule_type: &CapsuleType) -> bool {
            true
        }

        async fn load(&self, _path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: CommonCapsuleId::new(format!("handle-{}", uuid::Uuid::new_v4())),
                manifest,
                args: vec![],
            })
        }

        async fn start(&self, _handle: &CapsuleHandle) -> Result<()> {
            Ok(())
        }

        async fn stop(&self, _handle: &CapsuleHandle) -> Result<()> {
            Ok(())
        }

        async fn status(&self, _handle: &CapsuleHandle) -> Result<CapsuleStatus> {
            Ok(CapsuleStatus::Running)
        }

        async fn info(&self, handle: &CapsuleHandle) -> Result<ComputeCapsuleInfo> {
            Ok(ComputeCapsuleInfo {
                id: handle.id.clone(),
                name: handle.manifest.name.clone(),
                status: CapsuleStatus::Running,
                memory_used_mb: 0,
            })
        }
    }

    fn create_test_manager() -> CapsuleManager {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        CapsuleManager::new(compute, capability_manager, metrics, audit_log)
    }

    fn create_test_manifest() -> CapsuleManifest {
        CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.to_string(),
            name: "test-capsule".to_string(),
            version: "0.1.0".to_string(),
            description: None,
            author: None,
            capsule_type: CapsuleType::Wasm,
            entrypoint: "main.wasm".to_string(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    #[tokio::test]
    async fn test_capsule_id() {
        let id1 = CapsuleId::new();
        let id2 = CapsuleId::new();
        assert_ne!(id1, id2);
        assert!(id1.as_str().starts_with("cap-"));
    }

    #[tokio::test]
    async fn test_launch_and_stop() {
        let manager = create_test_manager();
        let manifest = create_test_manifest();

        let temp_dir = tempfile::tempdir().unwrap();

        let capsule_id = manager
            .launch_local(temp_dir.path(), manifest, TrustLevel::Untrusted)
            .await
            .unwrap();

        // Check it's running
        let running = manager.list_running().await;
        assert_eq!(running.len(), 1);
        assert_eq!(running[0], capsule_id);

        // Get info
        let info = manager.get(&capsule_id).await.unwrap();
        assert_eq!(info.state, CapsuleState::Running);
        assert_eq!(info.manifest.name, "test-capsule");

        // Stop
        manager
            .stop(&capsule_id, StopReason::Requested)
            .await
            .unwrap();

        // Check it's stopped
        let running = manager.list_running().await;
        assert_eq!(running.len(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_stopped() {
        let manager = create_test_manager();
        let manifest = create_test_manifest();

        let temp_dir = tempfile::tempdir().unwrap();

        let capsule_id = manager
            .launch_local(temp_dir.path(), manifest, TrustLevel::Untrusted)
            .await
            .unwrap();

        manager
            .stop(&capsule_id, StopReason::Completed)
            .await
            .unwrap();

        // Still in list (just stopped)
        assert_eq!(manager.list().await.len(), 1);

        // Cleanup
        manager.cleanup_stopped().await;

        // Now removed
        assert_eq!(manager.list().await.len(), 0);
    }

    #[tokio::test]
    async fn test_count_by_state() {
        let manager = create_test_manager();
        let manifest = create_test_manifest();

        let temp_dir = tempfile::tempdir().unwrap();

        let id1 = manager
            .launch_local(temp_dir.path(), manifest.clone(), TrustLevel::Untrusted)
            .await
            .unwrap();

        let _id2 = manager
            .launch_local(temp_dir.path(), manifest, TrustLevel::Untrusted)
            .await
            .unwrap();

        let counts = manager.count_by_state().await;
        assert_eq!(counts.get(&CapsuleState::Running), Some(&2));

        manager.stop(&id1, StopReason::Requested).await.unwrap();

        let counts = manager.count_by_state().await;
        assert_eq!(counts.get(&CapsuleState::Running), Some(&1));
        assert_eq!(counts.get(&CapsuleState::Stopped), Some(&1));
    }
}
