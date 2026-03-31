//! Runtime builder and lifecycle management
//!
//! The ElastosRuntime struct ties together all components and provides
//! a clean interface for starting and stopping the runtime.
use std::path::PathBuf;
use std::sync::Arc;

use elastos_compute::ComputeProvider;

use crate::capability::{CapabilityManager, CapabilityStore};
use crate::capsule::CapsuleManager;
use crate::content::{ContentFetcher, ContentResolver, ResolverConfig};
use crate::handler::RequestHandler;
use crate::messaging::MessageChannel;
use crate::primitives::audit::AuditLog;
use crate::primitives::metrics::MetricsManager;
use crate::primitives::time::SecureTimeSource;

use super::shell::{ShellConfig, ShellManager};

const RUNTIME_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Runtime configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Base directory for runtime data
    pub data_dir: PathBuf,
    /// Enable audit logging
    pub enable_audit: bool,
    /// Audit log path (if None, uses data_dir/audit.log)
    pub audit_log_path: Option<PathBuf>,
    /// IPFS gateways for content resolution
    pub ipfs_gateways: Vec<String>,
    /// Enable content caching
    pub enable_cache: bool,
    /// Maximum content size (bytes)
    pub max_content_size: usize,
    /// Runtime version
    pub version: String,
    /// Shell configuration
    pub shell: ShellConfig,
    /// Trusted publisher public keys (hex-encoded Ed25519).
    /// Capsules signed by these keys skip the trust prompt.
    pub trusted_keys: Vec<String>,
    /// Dev mode: skip trusted-root verification.
    pub dev_mode: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("/tmp/elastos"),
            enable_audit: true,
            audit_log_path: None,
            ipfs_gateways: Vec::new(),
            enable_cache: true,
            max_content_size: 100 * 1024 * 1024, // 100 MB
            version: RUNTIME_VERSION.to_string(),
            shell: ShellConfig::default(),
            trusted_keys: Vec::new(),
            dev_mode: false,
        }
    }
}

/// On-disk config file representation (`config.toml`).
///
/// All fields are optional — absent fields fall back to [`RuntimeConfig`] defaults.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub enable_audit: Option<bool>,
    #[serde(default)]
    pub audit_log_path: Option<String>,
    #[serde(default)]
    pub ipfs_gateways: Option<Vec<String>>,
    #[serde(default)]
    pub enable_cache: Option<bool>,
    #[serde(default)]
    pub max_content_size_mb: Option<usize>,
    #[serde(default)]
    pub trusted_keys: Option<Vec<String>>,
    #[serde(default)]
    pub dev_mode: Option<bool>,
}

impl RuntimeConfig {
    /// Load configuration from `{data_dir}/config.toml`, falling back to defaults.
    ///
    /// Returns `(config, is_first_run)` — `is_first_run` is `true` when the
    /// config file did not previously exist (and has just been created with defaults).
    pub fn load(data_dir: &std::path::Path) -> (Self, bool) {
        let config_path = data_dir.join("config.toml");
        let mut base = Self {
            data_dir: data_dir.to_path_buf(),
            ..Self::default()
        };

        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(contents) => match toml::from_str::<ConfigFile>(&contents) {
                    Ok(cf) => {
                        base.apply_file(&cf);
                        return (base, false);
                    }
                    Err(e) => {
                        tracing::warn!("Invalid config.toml, using defaults: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Could not read config.toml: {}", e);
                }
            }
            (base, false)
        } else {
            // First run — create default config file
            let _ = std::fs::create_dir_all(data_dir);
            let default_toml = toml::to_string_pretty(&ConfigFile::default()).unwrap_or_default();
            let _ = std::fs::write(&config_path, default_toml);
            (base, true)
        }
    }

    /// Return the set of trusted publisher keys: config-file keys plus built-in roots.
    ///
    /// Built-in roots are the ElastOS project's signing keys for core capsules
    /// (shell, localhost-provider, did-provider, etc.). They are skipped
    /// when `dev_mode` is true.
    pub fn effective_trusted_keys(&self) -> Vec<String> {
        let keys = self.trusted_keys.clone();
        if !self.dev_mode {
            // Built-in trusted root keys for core capsules.
            // Add hex-encoded Ed25519 public keys here once the project has
            // stable release keys. Example:
            //   keys.push("abcdef0123456789...".to_string());
        }
        keys
    }

    /// Overlay values from a parsed config file onto this config.
    fn apply_file(&mut self, cf: &ConfigFile) {
        if let Some(ref d) = cf.data_dir {
            self.data_dir = PathBuf::from(d);
        }
        if let Some(v) = cf.enable_audit {
            self.enable_audit = v;
        }
        if let Some(ref p) = cf.audit_log_path {
            self.audit_log_path = Some(PathBuf::from(p));
        }
        if let Some(ref g) = cf.ipfs_gateways {
            self.ipfs_gateways = g.clone();
        }
        if let Some(v) = cf.enable_cache {
            self.enable_cache = v;
        }
        if let Some(mb) = cf.max_content_size_mb {
            self.max_content_size = mb * 1024 * 1024;
        }
        if let Some(ref keys) = cf.trusted_keys {
            self.trusted_keys = keys.clone();
        }
        if let Some(v) = cf.dev_mode {
            self.dev_mode = v;
        }
    }
}

/// The main ElastOS runtime
///
/// This struct owns all the runtime components and provides
/// methods for starting, stopping, and interacting with capsules.
pub struct ElastosRuntime {
    /// Configuration
    config: RuntimeConfig,
    /// Secure time source
    time_source: Arc<SecureTimeSource>,
    /// Audit log
    audit_log: Arc<AuditLog>,
    /// Metrics manager
    metrics: Arc<MetricsManager>,
    /// Capability store
    capability_store: Arc<CapabilityStore>,
    /// Capability manager
    capability_manager: Arc<CapabilityManager>,
    /// Capsule manager
    capsule_manager: Arc<CapsuleManager>,
    /// Message channel
    message_channel: Arc<MessageChannel>,
    /// Content resolver
    content_resolver: Arc<ContentResolver>,
    /// Request handler for capsule messages
    request_handler: Arc<RequestHandler>,
    /// Shell manager
    shell_manager: ShellManager,
    /// Whether the runtime has been started
    started: bool,
}

impl ElastosRuntime {
    /// Build a new runtime with the given configuration, compute provider, and content fetcher.
    ///
    /// The `fetcher` provides transport-specific content fetching (e.g. HTTP).
    /// Pass `Arc::new(NullFetcher)` for offline/test mode.
    pub async fn build(
        config: RuntimeConfig,
        compute: Arc<dyn ComputeProvider>,
        fetcher: Arc<dyn ContentFetcher>,
    ) -> Result<Self, BuildError> {
        // Ensure data directory exists
        std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| BuildError::Io(format!("Failed to create data directory: {}", e)))?;

        // Initialize time source with persistence
        let time_path = config.data_dir.join("time_counter");
        let time_source = Arc::new(
            SecureTimeSource::with_persistence(&time_path)
                .map_err(|e| BuildError::Io(format!("Failed to initialize time source: {}", e)))?,
        );

        // Initialize audit log
        let audit_log = if config.enable_audit {
            let audit_path = config
                .audit_log_path
                .clone()
                .unwrap_or_else(|| config.data_dir.join("audit.log"));
            Arc::new(
                AuditLog::with_file(&audit_path)
                    .map_err(|e| BuildError::Io(format!("Failed to create audit log: {}", e)))?,
            )
        } else {
            Arc::new(AuditLog::new())
        };

        // Initialize metrics
        let metrics = Arc::new(MetricsManager::new());

        // Initialize capability store with persistence
        let cap_store_path = config.data_dir.join("capability_store");
        let capability_store = Arc::new(
            CapabilityStore::with_persistence(&cap_store_path)
                .await
                .map_err(|e| BuildError::Io(format!("Failed to create capability store: {}", e)))?,
        );

        // Initialize capability manager (loads persisted signing key or generates new one)
        let capability_manager = Arc::new(CapabilityManager::load_or_generate(
            &config.data_dir,
            capability_store.clone(),
            audit_log.clone(),
            metrics.clone(),
        ));

        // Initialize capsule manager
        let capsule_manager = Arc::new(CapsuleManager::new(
            compute,
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        // Initialize message channel
        let message_channel = Arc::new(MessageChannel::new(
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        // Initialize content resolver
        let resolver_config = ResolverConfig {
            cache_dir: Some(config.data_dir.join("content_cache")),
            ipfs_gateways: config.ipfs_gateways.clone(),
            max_content_size: config.max_content_size,
            enable_cache: config.enable_cache,
        };
        let content_resolver = Arc::new(ContentResolver::new(
            resolver_config,
            audit_log.clone(),
            fetcher.clone(),
        ));

        // Initialize request handler
        let request_handler = Arc::new(RequestHandler::new(
            capsule_manager.clone(),
            capability_manager.clone(),
            message_channel.clone(),
            content_resolver.clone(),
            audit_log.clone(),
            config.version.clone(),
            None, // Provider registry set separately for serve mode
        ));

        // Initialize shell manager
        let shell_manager = ShellManager::new(config.shell.clone(), fetcher);

        Ok(Self {
            config,
            time_source,
            audit_log,
            metrics,
            capability_store,
            capability_manager,
            capsule_manager,
            message_channel,
            content_resolver,
            request_handler,
            shell_manager,
            started: false,
        })
    }

    /// Start the runtime
    pub async fn start(&mut self) -> Result<(), StartError> {
        if self.started {
            return Err(StartError::AlreadyStarted);
        }

        tracing::info!("Starting ElastOS Runtime v{}", self.config.version);

        // Emit runtime start audit event
        self.audit_log.runtime_start(&self.config.version);

        // Bootstrap the shell capsule
        match self
            .shell_manager
            .bootstrap(
                &self.capsule_manager,
                &self.request_handler,
                &self.message_channel,
            )
            .await
        {
            Ok(shell_id) => {
                tracing::info!("Shell capsule started with ID: {}", shell_id);
            }
            Err(e) => {
                tracing::error!("Failed to bootstrap shell: {}", e);
                return Err(StartError::ShellBootstrap(e.to_string()));
            }
        }

        self.started = true;

        tracing::info!("ElastOS Runtime started successfully");
        Ok(())
    }

    /// Stop the runtime gracefully
    pub async fn stop(&mut self) -> Result<(), StopError> {
        if !self.started {
            return Err(StopError::NotStarted);
        }

        tracing::info!("Stopping ElastOS Runtime...");

        // Stop the shell first
        if let Err(e) = self
            .shell_manager
            .stop(&self.capsule_manager, &self.message_channel)
            .await
        {
            tracing::error!("Failed to stop shell: {}", e);
        }

        // Stop all other capsules
        self.capsule_manager
            .stop_all(crate::primitives::audit::StopReason::Requested)
            .await;

        // Persist capability store
        if let Err(e) = self.capability_store.persist().await {
            tracing::error!("Failed to persist capability store: {}", e);
        }

        // Persist time source
        if let Err(e) = self.time_source.persist() {
            tracing::error!("Failed to persist time source: {}", e);
        }

        // Emit runtime stop audit event
        self.audit_log.runtime_stop();

        self.started = false;

        tracing::info!("ElastOS Runtime stopped");
        Ok(())
    }

    /// Check if runtime is running
    pub fn is_running(&self) -> bool {
        self.started
    }

    /// Get the capsule manager
    pub fn capsule_manager(&self) -> &Arc<CapsuleManager> {
        &self.capsule_manager
    }

    /// Get the capability manager
    pub fn capability_manager(&self) -> &Arc<CapabilityManager> {
        &self.capability_manager
    }

    /// Get the message channel
    pub fn message_channel(&self) -> &Arc<MessageChannel> {
        &self.message_channel
    }

    /// Get the content resolver
    pub fn content_resolver(&self) -> &Arc<ContentResolver> {
        &self.content_resolver
    }

    /// Get the request handler
    pub fn request_handler(&self) -> &Arc<RequestHandler> {
        &self.request_handler
    }

    /// Get the metrics manager
    pub fn metrics(&self) -> &Arc<MetricsManager> {
        &self.metrics
    }

    /// Get the audit log
    pub fn audit_log(&self) -> &Arc<AuditLog> {
        &self.audit_log
    }

    /// Get runtime configuration
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
}

/// Errors during runtime building
#[derive(Debug)]
pub enum BuildError {
    /// IO error
    Io(String),
    /// Configuration error
    Config(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Io(e) => write!(f, "IO error: {}", e),
            BuildError::Config(e) => write!(f, "configuration error: {}", e),
        }
    }
}

impl std::error::Error for BuildError {}

/// Errors during runtime startup
#[derive(Debug)]
pub enum StartError {
    /// Runtime already started
    AlreadyStarted,
    /// Failed to bootstrap shell
    ShellBootstrap(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyStarted => write!(f, "runtime already started"),
            StartError::ShellBootstrap(e) => write!(f, "failed to bootstrap shell: {}", e),
        }
    }
}

impl std::error::Error for StartError {}

/// Errors during runtime shutdown
#[derive(Debug)]
pub enum StopError {
    /// Runtime not started
    NotStarted,
}

impl std::fmt::Display for StopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopError::NotStarted => write!(f, "runtime not started"),
        }
    }
}

impl std::error::Error for StopError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::NullFetcher;
    use elastos_common::{CapsuleManifest, CapsuleStatus, CapsuleType};
    use elastos_compute::CapsuleHandle;
    use elastos_compute::CapsuleInfo as ComputeCapsuleInfo;
    use std::path::Path;

    // Mock compute provider for testing
    struct MockComputeProvider;

    #[async_trait::async_trait]
    impl ComputeProvider for MockComputeProvider {
        fn supports(&self, _capsule_type: &CapsuleType) -> bool {
            true
        }

        async fn load(
            &self,
            _path: &Path,
            manifest: CapsuleManifest,
        ) -> elastos_common::Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: elastos_common::CapsuleId::new(format!("handle-{}", uuid::Uuid::new_v4())),
                manifest,
                args: vec![],
            })
        }

        async fn start(&self, _handle: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn stop(&self, _handle: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn status(&self, _handle: &CapsuleHandle) -> elastos_common::Result<CapsuleStatus> {
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

    #[tokio::test]
    async fn test_runtime_build() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            data_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let compute = Arc::new(MockComputeProvider);
        let runtime = ElastosRuntime::build(config, compute, Arc::new(NullFetcher))
            .await
            .unwrap();

        assert!(!runtime.is_running());
    }

    #[tokio::test]
    async fn test_runtime_start_stop() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            data_dir: temp_dir.path().to_path_buf(),
            enable_audit: false, // Disable for test simplicity
            ..Default::default()
        };

        let compute = Arc::new(MockComputeProvider);
        let mut runtime = ElastosRuntime::build(config, compute, Arc::new(NullFetcher))
            .await
            .unwrap();

        // Start
        runtime.start().await.unwrap();
        assert!(runtime.is_running());

        // Can't start twice
        assert!(runtime.start().await.is_err());

        // Stop
        runtime.stop().await.unwrap();
        assert!(!runtime.is_running());

        // Can't stop twice
        assert!(runtime.stop().await.is_err());
    }

    #[tokio::test]
    async fn test_runtime_accessors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            data_dir: temp_dir.path().to_path_buf(),
            enable_audit: false,
            ..Default::default()
        };

        let compute = Arc::new(MockComputeProvider);
        let runtime = ElastosRuntime::build(config, compute, Arc::new(NullFetcher))
            .await
            .unwrap();

        // All managers should be accessible
        let _capsule_manager = runtime.capsule_manager();
        let _capability_manager = runtime.capability_manager();
        let _message_channel = runtime.message_channel();
        let _content_resolver = runtime.content_resolver();
        let _metrics = runtime.metrics();
        let _audit_log = runtime.audit_log();
    }

    #[test]
    fn test_config_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let (config, first_run) = RuntimeConfig::load(dir.path());
        assert!(first_run, "should be first run");
        assert_eq!(config.data_dir, dir.path());
        assert!(config.ipfs_gateways.is_empty());
        // Config file should have been created
        assert!(dir.path().join("config.toml").exists());
    }

    #[test]
    fn test_config_reload() {
        let dir = tempfile::tempdir().unwrap();
        // First run
        let _ = RuntimeConfig::load(dir.path());
        // Second run — not first run
        let (_, first_run) = RuntimeConfig::load(dir.path());
        assert!(!first_run);
    }

    #[test]
    fn test_config_custom_values() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
dev_mode = true
enable_cache = false
trusted_keys = ["abc123"]
"#;
        std::fs::write(dir.path().join("config.toml"), toml_content).unwrap();
        let (config, first_run) = RuntimeConfig::load(dir.path());
        assert!(!first_run);
        assert!(config.dev_mode);
        assert!(!config.enable_cache);
        assert_eq!(config.trusted_keys, vec!["abc123".to_string()]);
    }
}
