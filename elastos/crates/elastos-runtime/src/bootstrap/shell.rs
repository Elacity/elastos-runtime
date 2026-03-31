//! Shell capsule bootstrap
//!
//! The shell is a special capsule that acts as the orchestrator.
//! It has elevated privileges:
//! - Can list/launch/stop other capsules
//! - Can grant/revoke capabilities
//! - Acts as the user's interface to the runtime
use std::path::PathBuf;
use std::sync::Arc;

use crate::capsule::{CapsuleId, CapsuleManager};
use crate::content::ContentFetcher;
use crate::handler::{CapsuleIoBridge, RequestHandler};
use crate::messaging::MessageChannel;
use crate::primitives::audit::TrustLevel;

/// Shell configuration
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Path to the shell capsule (local filesystem)
    pub path: Option<PathBuf>,
    /// IPFS CID of the shell capsule
    pub cid: Option<String>,
    /// Trust level for the shell (default: Trusted)
    pub trust_level: TrustLevel,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            path: None,
            cid: None,
            trust_level: TrustLevel::Trusted,
        }
    }
}

impl ShellConfig {
    /// Create a config for a local shell
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            cid: None,
            trust_level: TrustLevel::Trusted,
        }
    }

    /// Create a config for a CID-based shell
    pub fn from_cid(cid: impl Into<String>) -> Self {
        Self {
            path: None,
            cid: Some(cid.into()),
            trust_level: TrustLevel::Trusted,
        }
    }

    /// Check if shell is configured
    pub fn is_configured(&self) -> bool {
        self.path.is_some() || self.cid.is_some()
    }
}

/// Shell manager handles the shell capsule lifecycle
pub struct ShellManager {
    /// The shell capsule ID (once launched)
    shell_id: Option<CapsuleId>,
    /// Shell configuration
    config: ShellConfig,
}

impl ShellManager {
    /// Create a new shell manager
    pub fn new(config: ShellConfig, _fetcher: Arc<dyn ContentFetcher>) -> Self {
        Self {
            shell_id: None,
            config,
        }
    }

    /// Get the shell capsule ID
    pub fn shell_id(&self) -> Option<&CapsuleId> {
        self.shell_id.as_ref()
    }

    /// Check if shell is running
    pub fn is_running(&self) -> bool {
        self.shell_id.is_some()
    }

    /// Bootstrap the shell capsule
    ///
    /// This launches the shell and registers it with the request handler.
    pub async fn bootstrap(
        &mut self,
        capsule_manager: &Arc<CapsuleManager>,
        request_handler: &Arc<RequestHandler>,
        message_channel: &Arc<MessageChannel>,
    ) -> Result<CapsuleId, ShellError> {
        if self.shell_id.is_some() {
            return Err(ShellError::AlreadyRunning);
        }

        // Determine shell source
        let shell_id = if let Some(path) = &self.config.path {
            self.launch_local_shell(path, capsule_manager).await?
        } else if let Some(cid) = &self.config.cid {
            self.launch_cid_shell(cid, capsule_manager).await?
        } else {
            // No shell configured - create a virtual shell ID for API use
            let id = CapsuleId::new();
            tracing::info!(
                "No shell capsule configured, using virtual shell ID: {}",
                id
            );
            id
        };

        // Register shell with request handler (grants elevated privileges)
        request_handler.set_shell(shell_id.clone()).await;

        // Register shell with message channel (set shell ID for auth exemption)
        message_channel
            .set_shell_id(shell_id.as_str().to_string())
            .await;
        let _rx = message_channel.register(shell_id.as_str()).await;

        self.shell_id = Some(shell_id.clone());

        tracing::info!("Shell capsule bootstrapped with ID: {}", shell_id);

        Ok(shell_id)
    }

    /// Launch shell from local path
    async fn launch_local_shell(
        &self,
        path: &std::path::Path,
        capsule_manager: &Arc<CapsuleManager>,
    ) -> Result<CapsuleId, ShellError> {
        tracing::info!("Launching shell from local path: {}", path.display());

        // Read manifest
        let manifest_path = path.join("capsule.json");
        let manifest_data = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| ShellError::Launch(format!("Failed to read shell manifest: {}", e)))?;

        let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)
            .map_err(|e| ShellError::Launch(format!("Failed to parse shell manifest: {}", e)))?;

        manifest
            .validate()
            .map_err(|e| ShellError::Launch(format!("Invalid manifest: {}", e)))?;

        // Launch via capsule manager
        capsule_manager
            .launch_local(path, manifest, self.config.trust_level)
            .await
            .map_err(|e| ShellError::Launch(format!("Failed to launch shell: {}", e)))
    }

    /// Launch shell from CID.
    ///
    /// The trusted runtime core no longer fabricates public-web fetch paths
    /// for shell bootstrap. Resolve the shell capsule through a higher trusted
    /// source / Carrier path first, then hand the runtime a local shell path.
    async fn launch_cid_shell(
        &self,
        cid: &str,
        _capsule_manager: &Arc<CapsuleManager>,
    ) -> Result<CapsuleId, ShellError> {
        Err(ShellError::Launch(format!(
            "CID shell bootstrap is not supported in the trusted runtime core. Resolve {} through a trusted source first, then launch the shell from a local path.",
            cid
        )))
    }

    /// Stop the shell capsule
    pub async fn stop(
        &mut self,
        capsule_manager: &Arc<CapsuleManager>,
        message_channel: &Arc<MessageChannel>,
    ) -> Result<(), ShellError> {
        if let Some(shell_id) = self.shell_id.take() {
            // Unregister from message channel
            message_channel.unregister(shell_id.as_str()).await;

            // Stop the capsule (if it's a real capsule, not virtual)
            if capsule_manager.get(&shell_id).await.is_some() {
                capsule_manager
                    .stop(&shell_id, crate::primitives::audit::StopReason::Requested)
                    .await
                    .map_err(|e| ShellError::Stop(format!("Failed to stop shell: {}", e)))?;
            }

            tracing::info!("Shell capsule stopped");
        }

        Ok(())
    }

    /// Create an I/O bridge for the shell
    pub fn create_io_bridge(
        &self,
        request_handler: Arc<RequestHandler>,
    ) -> Option<CapsuleIoBridge> {
        self.shell_id
            .as_ref()
            .map(|id| CapsuleIoBridge::new(id.clone(), request_handler))
    }
}

/// Shell-related errors
#[derive(Debug)]
pub enum ShellError {
    /// Shell is already running
    AlreadyRunning,
    /// Failed to launch shell
    Launch(String),
    /// Failed to stop shell
    Stop(String),
    /// Shell not configured
    NotConfigured,
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::AlreadyRunning => write!(f, "shell is already running"),
            ShellError::Launch(e) => write!(f, "failed to launch shell: {}", e),
            ShellError::Stop(e) => write!(f, "failed to stop shell: {}", e),
            ShellError::NotConfigured => write!(f, "shell not configured"),
        }
    }
}

impl std::error::Error for ShellError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityManager, CapabilityStore};
    use crate::content::{ContentResolver, NullFetcher, ResolverConfig};
    use crate::primitives::audit::AuditLog;
    use crate::primitives::metrics::MetricsManager;
    use elastos_common::{CapsuleManifest, CapsuleStatus, CapsuleType};
    use elastos_compute::{CapsuleHandle, CapsuleInfo as ComputeCapsuleInfo, ComputeProvider};
    use std::path::Path;

    fn test_shell_manager(config: ShellConfig) -> ShellManager {
        ShellManager::new(config, Arc::new(NullFetcher))
    }

    // Mock compute provider
    struct MockComputeProvider;

    #[async_trait::async_trait]
    impl ComputeProvider for MockComputeProvider {
        fn supports(&self, _: &CapsuleType) -> bool {
            true
        }

        async fn load(
            &self,
            _: &Path,
            manifest: CapsuleManifest,
        ) -> elastos_common::Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: elastos_common::CapsuleId::new("shell-handle"),
                manifest,
                args: vec![],
            })
        }

        async fn start(&self, _: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn stop(&self, _: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn status(&self, _: &CapsuleHandle) -> elastos_common::Result<CapsuleStatus> {
            Ok(CapsuleStatus::Running)
        }

        async fn info(&self, handle: &CapsuleHandle) -> elastos_common::Result<ComputeCapsuleInfo> {
            Ok(ComputeCapsuleInfo {
                id: handle.id.clone(),
                name: handle.manifest.name.clone(),
                status: CapsuleStatus::Running,
                memory_used_mb: 0,
            })
        }
    }

    async fn create_test_components() -> (
        Arc<CapsuleManager>,
        Arc<RequestHandler>,
        Arc<MessageChannel>,
    ) {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());

        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        let capsule_manager = Arc::new(CapsuleManager::new(
            compute,
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let message_channel = Arc::new(MessageChannel::new(
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let content_resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            audit_log.clone(),
            Arc::new(NullFetcher),
        ));

        let request_handler = Arc::new(RequestHandler::new(
            capsule_manager.clone(),
            capability_manager,
            message_channel.clone(),
            content_resolver,
            audit_log,
            "0.1.0".to_string(),
            None,
        ));

        (capsule_manager, request_handler, message_channel)
    }

    #[tokio::test]
    async fn test_shell_config_default() {
        let config = ShellConfig::default();
        assert!(!config.is_configured());
    }

    #[tokio::test]
    async fn test_shell_config_local() {
        let config = ShellConfig::local("/path/to/shell");
        assert!(config.is_configured());
        assert!(config.path.is_some());
    }

    #[tokio::test]
    async fn test_shell_config_cid() {
        let config = ShellConfig::from_cid("Qm123");
        assert!(config.is_configured());
        assert!(config.cid.is_some());
    }

    #[tokio::test]
    async fn test_shell_manager_bootstrap_no_config() {
        let (capsule_manager, request_handler, message_channel) = create_test_components().await;

        let mut shell_manager = test_shell_manager(ShellConfig::default());

        // Should create a virtual shell ID
        let result = shell_manager
            .bootstrap(&capsule_manager, &request_handler, &message_channel)
            .await;

        assert!(result.is_ok());
        assert!(shell_manager.is_running());
    }

    #[tokio::test]
    async fn test_shell_manager_double_bootstrap() {
        let (capsule_manager, request_handler, message_channel) = create_test_components().await;

        let mut shell_manager = test_shell_manager(ShellConfig::default());

        // First bootstrap succeeds
        shell_manager
            .bootstrap(&capsule_manager, &request_handler, &message_channel)
            .await
            .unwrap();

        // Second bootstrap fails
        let result = shell_manager
            .bootstrap(&capsule_manager, &request_handler, &message_channel)
            .await;

        assert!(matches!(result, Err(ShellError::AlreadyRunning)));
    }

    #[tokio::test]
    async fn test_shell_manager_stop() {
        let (capsule_manager, request_handler, message_channel) = create_test_components().await;

        let mut shell_manager = test_shell_manager(ShellConfig::default());

        shell_manager
            .bootstrap(&capsule_manager, &request_handler, &message_channel)
            .await
            .unwrap();

        assert!(shell_manager.is_running());

        shell_manager
            .stop(&capsule_manager, &message_channel)
            .await
            .unwrap();

        assert!(!shell_manager.is_running());
    }

    #[tokio::test]
    async fn test_shell_manager_cid_bootstrap_fails_closed() {
        let (capsule_manager, request_handler, message_channel) = create_test_components().await;
        let mut shell_manager = test_shell_manager(ShellConfig::from_cid("Qm123"));

        let result = shell_manager
            .bootstrap(&capsule_manager, &request_handler, &message_channel)
            .await;

        match result {
            Err(ShellError::Launch(message)) => {
                assert!(message.contains("not supported"));
                assert!(message.contains("trusted source"));
            }
            other => panic!("Expected fail-closed CID bootstrap error, got {:?}", other),
        }
    }
}
