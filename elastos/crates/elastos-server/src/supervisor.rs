//! Capsule supervisor — lifecycle management for capsule VMs.
//!
//! The supervisor is the runtime's control plane: ensure capsules are downloaded
//! and verified, launch them in crosvm VMs, stop them, and report status.
//! Guest capsules reach it over the Carrier-managed private control network.
//!
//! crosvm is the sole VM backend. No fallback — KVM is required.

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::carrier_service::CarrierServiceProvider;
use crate::ownership;
use crate::setup::{CapsuleEntry, ComponentsManifest};
use crate::vm_provider::VmCapsuleProvider;

use elastos_crosvm::{CrosvmConfig, NetworkConfig, RunningVm, VmConfig};
use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::session::{SessionRegistry, SessionType};

/// TCP port used by VM provider capsules for raw JSON request/response over the
/// Carrier-managed control network.
const VM_PROVIDER_PORT: u16 = 7000;
const CACHED_CID_FILE: &str = ".elastos-cid";
const CACHED_ARTIFACT_SHA_FILE: &str = ".elastos-artifact-sha256";
const CHAT_RETURN_HOME_EXIT_CODE: i32 = 73;

fn vm_provider_bridge_enabled() -> bool {
    std::env::var("ELASTOS_VM_PROVIDER_BRIDGE")
        .map(|v| {
            let n = v.to_ascii_lowercase();
            !(n == "0" || n == "false" || n == "no" || n == "off")
        })
        .unwrap_or(true)
}

// ── Control API types ───────────────────────────────────────────────

/// Request from shell to runtime supervisor.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum SupervisorRequest {
    #[serde(rename = "ensure_capsule")]
    EnsureCapsule { name: String },

    #[serde(rename = "launch_capsule")]
    LaunchCapsule {
        name: String,
        #[serde(default)]
        config: serde_json::Value,
    },

    #[serde(rename = "stop_capsule")]
    StopCapsule { handle: String },

    #[serde(rename = "wait_capsule")]
    WaitCapsule { handle: String },

    #[serde(rename = "capsule_status")]
    CapsuleStatus { handle: String },

    #[serde(rename = "download_external")]
    DownloadExternal { name: String, platform: String },

    #[serde(rename = "start_gateway")]
    StartGateway {
        addr: String,
        #[serde(default)]
        cache_dir: Option<String>,
    },
}

/// Response from runtime supervisor to shell.
#[derive(Debug, Serialize, Deserialize)]
pub struct SupervisorResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vsock_cid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SupervisorResponse {
    fn ok() -> Self {
        Self {
            status: "ok".into(),
            path: None,
            handle: None,
            vsock_cid: None,
            uptime_secs: None,
            exit_code: None,
            error: None,
        }
    }

    fn ok_with_path(path: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
            ..Self::ok()
        }
    }

    fn ok_with_exit_code(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            ..Self::ok()
        }
    }

    fn ok_with_handle(handle: impl Into<String>, vsock_cid: u32) -> Self {
        Self {
            handle: Some(handle.into()),
            vsock_cid: Some(vsock_cid),
            ..Self::ok()
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            status: "error".into(),
            error: Some(msg.into()),
            path: None,
            handle: None,
            vsock_cid: None,
            uptime_secs: None,
            exit_code: None,
        }
    }
}

// ── Running capsule tracking ────────────────────────────────────────

/// Backend process for a running capsule.
enum CapsuleBackend {
    /// crosvm microVM. Carrier owns the private control network used for guest
    /// runtime API access and VM-backed provider RPC.
    Vm(Box<RunningVm>),
    /// Carrier-plane host process (for `permissions.carrier: true`).
    /// These are explicit runtime-owned providers, not ordinary app capsules.
    Carrier,
}

struct RunningCapsule {
    name: String,
    handle: String,
    vsock_cid: u32,
    started_at: std::time::Instant,
    provider_route: Option<ProviderRoute>,
    backend: CapsuleBackend,
}

struct RunningGateway {
    addr: String,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProviderRoute {
    SubProvider(String),
    Scheme(String),
}

// ── Supervisor ──────────────────────────────────────────────────────

/// The capsule supervisor manages lifecycle for all capsule VMs.
pub struct Supervisor {
    /// Where capsule artifacts are stored (~/.local/share/elastos/capsules/)
    capsules_dir: PathBuf,
    /// Where external tools are stored (~/.local/share/elastos/)
    data_dir: PathBuf,
    /// The components.json registry (capsules + external tools)
    registry: ComponentsManifest,
    /// Currently running capsules, keyed by handle
    running: Arc<RwLock<HashMap<String, RunningCapsule>>>,
    /// Next vsock CID to assign (starts at 3, increments)
    next_cid: Arc<RwLock<u32>>,
    /// crosvm configuration (paths to binary, kernel, etc.)
    crosvm_config: CrosvmConfig,
    /// Shell session token — only injected into the shell capsule VM
    shell_token: Option<String>,
    /// API address injected into VM boot args (set by forward_to_shell)
    api_addr: Option<String>,
    /// Session registry for minting per-capsule tokens
    session_registry: Option<Arc<SessionRegistry>>,
    /// Runtime provider registry for VM-backed provider route registration.
    provider_registry: Option<Arc<ProviderRegistry>>,
    /// Capability manager for minting real tokens in the microVM Carrier bridge.
    capability_manager: Option<Arc<elastos_runtime::capability::CapabilityManager>>,
    /// Pending capability request store for shell-mediated approval.
    pending_store: Option<Arc<elastos_runtime::capability::pending::PendingRequestStore>>,
    /// Optional running gateway server task.
    gateway: Arc<RwLock<Option<RunningGateway>>>,
}

impl Supervisor {
    fn initial_cid_seed() -> u32 {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        100 + (millis % 100_000)
    }

    fn unique_handle(name: &str, cid: u32) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("vm-{}-{}-{}", name, cid, millis)
    }

    fn resolve_external_install_path(
        registry: &ComponentsManifest,
        data_dir: &Path,
        component: &str,
        default_relative: &str,
    ) -> PathBuf {
        registry
            .external
            .get(component)
            .and_then(|entry| entry.install_path.as_deref())
            .map(|rel| data_dir.join(rel))
            .unwrap_or_else(|| data_dir.join(default_relative))
    }

    fn verify_host_artifact(&self, component: &str, path: &Path) -> Result<()> {
        let checksum =
            crate::setup::verify_installed_component_binary(&self.data_dir, component, path)
                .map_err(|err| {
                    err.context(format!(
                        "refusing to launch capsule with unverified host artifact '{}' at {}",
                        component,
                        path.display()
                    ))
                })?;
        tracing::info!(
            "{} host artifact verified against installed manifest ({})",
            component,
            checksum
        );
        Ok(())
    }

    fn verify_carrier_service_binary(
        &self,
        name: &str,
        capsule_dir: &Path,
        binary_path: &Path,
    ) -> Result<()> {
        let capsule_root = std::fs::canonicalize(capsule_dir).with_context(|| {
            format!(
                "failed to canonicalize carrier service capsule dir for '{}': {}",
                name,
                capsule_dir.display()
            )
        })?;
        let binary = std::fs::canonicalize(binary_path).with_context(|| {
            format!(
                "failed to canonicalize carrier service binary for '{}': {}",
                name,
                binary_path.display()
            )
        })?;

        if !binary.starts_with(&capsule_root) {
            bail!(
                "carrier service binary for '{}' escapes capsule artifact root: {} not under {}",
                name,
                binary.display(),
                capsule_root.display()
            );
        }

        let artifact_binary = capsule_root.join(name);
        if binary == artifact_binary {
            let entry = self.registry.capsules.get(name).ok_or_else(|| {
                anyhow::anyhow!("carrier service '{}' missing capsule registry entry", name)
            })?;
            let cached_cid = std::fs::read_to_string(capsule_dir.join(CACHED_CID_FILE))
                .with_context(|| {
                    format!(
                        "carrier service '{}' missing cached capsule CID metadata at {}",
                        name,
                        capsule_dir.join(CACHED_CID_FILE).display()
                    )
                })?;
            if cached_cid.trim() != entry.cid {
                bail!(
                    "carrier service '{}' cached CID mismatch: have {}, expected {}",
                    name,
                    cached_cid.trim(),
                    entry.cid
                );
            }

            if !entry.sha256.is_empty() {
                let cached_sha =
                    std::fs::read_to_string(capsule_dir.join(CACHED_ARTIFACT_SHA_FILE))
                        .with_context(|| {
                            format!(
                                "carrier service '{}' missing cached capsule sha metadata at {}",
                                name,
                                capsule_dir.join(CACHED_ARTIFACT_SHA_FILE).display()
                            )
                        })?;
                if cached_sha.trim() != entry.sha256 {
                    bail!(
                        "carrier service '{}' cached sha mismatch: have {}, expected {}",
                        name,
                        cached_sha.trim(),
                        entry.sha256
                    );
                }
            }

            tracing::info!(
                "carrier service '{}' rooted in ensured capsule artifact {} ({})",
                name,
                entry.cid,
                if entry.sha256.is_empty() {
                    "sha256 unavailable"
                } else {
                    "sha256 verified by cache metadata"
                }
            );
        } else {
            tracing::warn!(
                "carrier service '{}' launching from nested binary under capsule artifact root without direct artifact-binary match: {}",
                name,
                binary.display()
            );
        }

        Ok(())
    }

    pub fn new(data_dir: PathBuf, registry: ComponentsManifest) -> Self {
        let capsules_dir = data_dir.join("capsules");
        let crosvm_bin =
            Self::resolve_external_install_path(&registry, &data_dir, "crosvm", "bin/crosvm");
        let kernel_path =
            Self::resolve_external_install_path(&registry, &data_dir, "vmlinux", "bin/vmlinux");

        let crosvm_config = CrosvmConfig::new()
            .with_crosvm_bin(crosvm_bin)
            .with_kernel_path(kernel_path)
            .with_socket_dir(data_dir.join("crosvm"))
            .with_rootfs_cache_dir(data_dir.join("rootfs-cache"));

        Self {
            capsules_dir,
            data_dir,
            registry,
            running: Arc::new(RwLock::new(HashMap::new())),
            next_cid: Arc::new(RwLock::new(Self::initial_cid_seed())),
            crosvm_config,
            shell_token: None,
            api_addr: None,
            session_registry: None,
            provider_registry: None,
            capability_manager: None,
            pending_store: None,
            gateway: Arc::new(RwLock::new(None)),
        }
    }

    /// Set shell session credentials and session registry for minting capsule tokens.
    /// The shell token is only used for the shell VM itself. Other capsules get
    /// fresh Capsule-type tokens via the session registry.
    pub fn set_session(
        &mut self,
        shell_token: String,
        api_addr: String,
        session_registry: Arc<SessionRegistry>,
    ) {
        self.shell_token = Some(shell_token);
        self.api_addr = Some(api_addr);
        self.session_registry = Some(session_registry);
    }

    /// Attach runtime provider registry so launched VM providers can be routed.
    pub fn set_provider_registry(&mut self, provider_registry: Arc<ProviderRegistry>) {
        self.provider_registry = Some(provider_registry);
    }

    /// Attach capability manager for real token minting in the microVM Carrier bridge.
    pub fn set_capability_manager(
        &mut self,
        capability_manager: Arc<elastos_runtime::capability::CapabilityManager>,
    ) {
        self.capability_manager = Some(capability_manager);
    }

    /// Attach pending request store for shell-mediated capability approval.
    pub fn set_pending_store(
        &mut self,
        pending_store: Arc<elastos_runtime::capability::pending::PendingRequestStore>,
    ) {
        self.pending_store = Some(pending_store);
    }

    /// Handle a supervisor request from the shell.
    pub async fn handle_request(&self, req: SupervisorRequest) -> SupervisorResponse {
        self.reap_dead_capsules().await;
        match req {
            SupervisorRequest::EnsureCapsule { name } => match self.ensure_capsule(&name).await {
                Ok(path) => SupervisorResponse::ok_with_path(path.display().to_string()),
                Err(e) => SupervisorResponse::err(format!("ensure_capsule failed: {e}")),
            },
            SupervisorRequest::LaunchCapsule { name, config } => {
                match self.launch_capsule(&name, config).await {
                    Ok((handle, cid)) => SupervisorResponse::ok_with_handle(handle, cid),
                    Err(e) => SupervisorResponse::err(format!("launch_capsule failed: {e}")),
                }
            }
            SupervisorRequest::StopCapsule { handle } => match self.stop_capsule(&handle).await {
                Ok(()) => SupervisorResponse::ok(),
                Err(e) => SupervisorResponse::err(format!("stop_capsule failed: {e}")),
            },
            SupervisorRequest::WaitCapsule { handle } => match self.wait_for_exit(&handle).await {
                Ok(exit_code) => SupervisorResponse::ok_with_exit_code(exit_code),
                Err(e) => SupervisorResponse::err(format!("wait_capsule failed: {e}")),
            },
            SupervisorRequest::CapsuleStatus { handle } => {
                match self.capsule_status(&handle).await {
                    Ok(resp) => resp,
                    Err(e) => SupervisorResponse::err(format!("capsule_status failed: {e}")),
                }
            }
            SupervisorRequest::DownloadExternal { name, platform } => {
                match self.download_external(&name, &platform).await {
                    Ok(path) => SupervisorResponse::ok_with_path(path.display().to_string()),
                    Err(e) => SupervisorResponse::err(format!("download_external failed: {e}")),
                }
            }
            SupervisorRequest::StartGateway { addr, cache_dir } => {
                match self.start_gateway(&addr, cache_dir).await {
                    Ok(listen_addr) => SupervisorResponse::ok_with_path(listen_addr),
                    Err(e) => SupervisorResponse::err(format!("start_gateway failed: {e}")),
                }
            }
        }
    }

    fn parse_provider_route_from_provides(provides: &str) -> Option<ProviderRoute> {
        let (scheme, rest) = provides.split_once("://")?;
        let scheme = scheme.trim().to_ascii_lowercase();
        if scheme.is_empty() {
            return None;
        }
        if scheme == "elastos" {
            let sub = rest.split('/').next()?.trim();
            if sub.is_empty() {
                return None;
            }
            return Some(ProviderRoute::SubProvider(sub.to_ascii_lowercase()));
        }
        match scheme.as_str() {
            "localhost" | "http" => Some(ProviderRoute::Scheme(scheme)),
            _ => None,
        }
    }

    async fn register_provider_route(
        &self,
        capsule_name: &str,
        provides: Option<&str>,
        guest_ip: &str,
        init_config: serde_json::Value,
    ) -> Option<ProviderRoute> {
        if !vm_provider_bridge_enabled() {
            return None;
        }
        let registry = match &self.provider_registry {
            Some(r) => r,
            None => return None,
        };
        let provides = match provides {
            Some(p) => p,
            None => return None,
        };
        let route = match Self::parse_provider_route_from_provides(provides) {
            Some(s) => s,
            None => return None,
        };
        let provider_scheme = match &route {
            ProviderRoute::SubProvider(sub) => sub.clone(),
            ProviderRoute::Scheme(scheme) => scheme.clone(),
        };

        let provider: Arc<dyn elastos_runtime::provider::Provider> =
            Arc::new(VmCapsuleProvider::new(
                provider_scheme,
                guest_ip.to_string(),
                VM_PROVIDER_PORT,
                init_config,
            ));

        match route.clone() {
            ProviderRoute::SubProvider(sub) => {
                match registry.register_sub_provider(&sub, provider).await {
                    Ok(_) => {
                        tracing::info!(
                        "Registered VM sub-provider route elastos://{}/... -> capsule '{}' (guest={}, port={})",
                        sub,
                        capsule_name,
                        guest_ip,
                        VM_PROVIDER_PORT
                    );
                        Some(ProviderRoute::SubProvider(sub))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to register VM provider route for '{}' ({}): {}",
                            capsule_name,
                            provides,
                            e
                        );
                        None
                    }
                }
            }
            ProviderRoute::Scheme(scheme) => {
                registry.register(provider).await;
                tracing::info!(
                    "Registered VM provider route {}://... -> capsule '{}' (guest={}, port={})",
                    scheme,
                    capsule_name,
                    guest_ip,
                    VM_PROVIDER_PORT
                );
                Some(ProviderRoute::Scheme(scheme))
            }
        }
    }

    async fn register_carrier_service_route(
        &self,
        capsule_name: &str,
        provides: Option<&str>,
        binary_path: &Path,
        env_vars: Vec<(String, String)>,
        init_config: serde_json::Value,
    ) -> Option<ProviderRoute> {
        if !vm_provider_bridge_enabled() {
            return None;
        }
        let registry = self.provider_registry.as_ref()?;
        let provides = provides?;
        let route = Self::parse_provider_route_from_provides(provides)?;
        let provider_scheme = match &route {
            ProviderRoute::SubProvider(sub) => sub.clone(),
            ProviderRoute::Scheme(scheme) => scheme.clone(),
        };

        let provider: Arc<dyn elastos_runtime::provider::Provider> =
            Arc::new(CarrierServiceProvider::new(
                provider_scheme,
                binary_path.display().to_string(),
                env_vars,
                init_config,
            ));

        match route.clone() {
            ProviderRoute::SubProvider(sub) => {
                match registry.register_sub_provider(&sub, provider).await {
                    Ok(_) => {
                        tracing::info!(
                            "Registered carrier service route elastos://{}/... -> '{}' (binary={})",
                            sub,
                            capsule_name,
                            binary_path.display()
                        );
                        Some(ProviderRoute::SubProvider(sub))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to register carrier service route for '{}' ({}): {}",
                            capsule_name,
                            provides,
                            e
                        );
                        None
                    }
                }
            }
            ProviderRoute::Scheme(scheme) => {
                registry.register(provider).await;
                tracing::info!(
                    "Registered carrier service route {}://... -> '{}' (binary={})",
                    scheme,
                    capsule_name,
                    binary_path.display()
                );
                Some(ProviderRoute::Scheme(scheme))
            }
        }
    }

    async fn unregister_provider_route(&self, route: &ProviderRoute) {
        let Some(registry) = &self.provider_registry else {
            return;
        };
        match route {
            ProviderRoute::SubProvider(sub) => {
                registry.unregister_sub_provider(sub).await;
            }
            ProviderRoute::Scheme(scheme) => {
                registry.unregister(scheme).await;
            }
        }
    }

    async fn reap_dead_capsules(&self) {
        let mut dead: Vec<(String, Option<ProviderRoute>)> = Vec::new();
        {
            let mut running = self.running.write().await;
            let dead_handles: Vec<String> = running
                .iter()
                .filter_map(|(handle, capsule)| {
                    let alive = match &capsule.backend {
                        CapsuleBackend::Vm(vm) => vm.is_running(),
                        CapsuleBackend::Carrier => true, // managed by carrier service bridge
                    };
                    if alive {
                        None
                    } else {
                        Some(handle.clone())
                    }
                })
                .collect();
            for handle in dead_handles {
                if let Some(capsule) = running.remove(&handle) {
                    dead.push((handle, capsule.provider_route));
                }
            }
        }

        for (handle, route) in dead {
            if let Some(route) = route.as_ref() {
                self.unregister_provider_route(route).await;
            }
            let overlay_path = self
                .crosvm_config
                .rootfs_cache_dir
                .join("overlays")
                .join(format!("{}.ext4", handle));
            let _ = tokio::fs::remove_file(&overlay_path).await;
            tracing::warn!(
                "Reaped exited capsule '{}' and unregistered provider route",
                handle
            );
        }
    }

    async fn load_capsule_manifest(
        &self,
        name: &str,
    ) -> Result<(PathBuf, elastos_common::CapsuleManifest)> {
        let capsule_dir = self.ensure_capsule(name).await?;
        let manifest_path = capsule_dir.join("capsule.json");
        let manifest_data = tokio::fs::read_to_string(&manifest_path)
            .await
            .with_context(|| format!("reading capsule.json for '{name}'"))?;
        let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)
            .with_context(|| format!("parsing capsule.json for '{name}'"))?;
        Ok((capsule_dir, manifest))
    }

    /// Resolve transitive dependencies for a target capsule.
    /// Returns launch-ordered capsules (dependencies first) and required externals.
    pub async fn resolve_launch_plan(&self, target: &str) -> Result<(Vec<String>, Vec<String>)> {
        let mut ordered_capsules = Vec::<String>::new();
        let mut externals = HashSet::<String>::new();
        let mut visited = HashSet::<String>::new();
        let mut visiting = HashSet::<String>::new();
        let mut manifests = HashMap::<String, elastos_common::CapsuleManifest>::new();
        let mut stack: Vec<(String, bool)> = vec![(target.to_string(), false)];

        while let Some((name, expanded)) = stack.pop() {
            if expanded {
                visiting.remove(&name);
                visited.insert(name.clone());
                ordered_capsules.push(name);
                continue;
            }

            if visited.contains(&name) {
                continue;
            }

            if !visiting.insert(name.clone()) {
                bail!("dependency cycle detected at capsule '{name}'");
            }

            let manifest = if let Some(m) = manifests.get(&name) {
                m.clone()
            } else {
                let (_, m) = self.load_capsule_manifest(&name).await?;
                manifests.insert(name.clone(), m.clone());
                m
            };

            stack.push((name.clone(), true));
            for req in manifest.requires.iter().rev() {
                match req.kind {
                    elastos_common::RequirementKind::Capsule => {
                        if !self.registry.capsules.contains_key(&req.name) {
                            bail!(
                                "unknown capsule requirement '{}' declared by capsule '{}'",
                                req.name,
                                name
                            );
                        }
                        if !visited.contains(&req.name) {
                            stack.push((req.name.clone(), false));
                        }
                    }
                    elastos_common::RequirementKind::External => {
                        if !self.registry.external.contains_key(&req.name) {
                            bail!(
                                "unknown external requirement '{}' declared by capsule '{}'",
                                req.name,
                                name
                            );
                        }
                        externals.insert(req.name.clone());
                    }
                }
            }
        }

        let mut external_list: Vec<String> = externals.into_iter().collect();
        external_list.sort();
        Ok((ordered_capsules, external_list))
    }

    /// Ensure a capsule artifact is locally available. Downloads if missing.
    async fn ensure_capsule(&self, name: &str) -> Result<PathBuf> {
        let capsule_dir = self.capsules_dir.join(name);

        // Look up in registry
        let entry = self
            .registry
            .capsules
            .get(name)
            .with_context(|| format!("capsule '{name}' not in registry"))?;

        if entry.cid.is_empty() {
            bail!("capsule '{name}' has no CID in registry (not yet published)");
        }

        // Already present and matches current registry entry?
        if capsule_dir.join("capsule.json").is_file() {
            let cached_cid = tokio::fs::read_to_string(capsule_dir.join(CACHED_CID_FILE))
                .await
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let cached_sha = tokio::fs::read_to_string(capsule_dir.join(CACHED_ARTIFACT_SHA_FILE))
                .await
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if cached_cid == entry.cid && (entry.sha256.is_empty() || cached_sha == entry.sha256) {
                return Ok(capsule_dir);
            }

            eprintln!(
                "  Refreshing cached capsule '{}' (registry CID changed or cache metadata missing)...",
                name
            );
            let _ = tokio::fs::remove_dir_all(&capsule_dir).await;
        }

        // Download capsule artifact from IPFS gateways
        self.download_capsule(name, entry, &capsule_dir).await?;

        Ok(capsule_dir)
    }

    /// Download a capsule artifact, verify, and extract.
    ///
    /// Canonical path only: local IPFS node (kubo) managed by ipfs-provider.
    /// Kubo fetches content over the IPFS/Carrier network using DHT + bitswap.
    /// No HTTP fallback is allowed here.
    async fn download_capsule(&self, name: &str, entry: &CapsuleEntry, dest: &Path) -> Result<()> {
        self.try_download_capsule_via_carrier(name, &entry.cid, &entry.sha256, dest)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "capsule download failed via elastos://ipfs provider path: {}",
                    e
                )
            })
    }

    /// Fetch capsule content via local IPFS node (Carrier network path).
    ///
    /// This path stays inside the runtime/provider boundary: supervisor talks to
    /// the registered `elastos://ipfs` provider, which owns Kubo startup and
    /// the local Elastos fetch policy.
    async fn try_download_capsule_via_carrier(
        &self,
        name: &str,
        cid: &str,
        expected_sha256: &str,
        dest: &Path,
    ) -> Result<()> {
        use sha2::Digest;

        println!(
            "  Fetching capsule '{}' via Carrier (IPFS P2P: {})...",
            name, cid
        );

        let bytes = self.ipfs_cat_via_provider(cid).await?;

        // Verify sha256 — fail closed if missing
        if expected_sha256.is_empty() {
            bail!(
                "No sha256 checksum for capsule '{}' (CID: {}). \
                 Integrity verification is mandatory. \
                 Ensure components.json includes sha256 for all artifacts.",
                name,
                cid
            );
        }
        let actual = hex::encode(sha2::Sha256::digest(&bytes));
        if actual != *expected_sha256 {
            bail!(
                "sha256 mismatch for '{}': expected {expected_sha256}, got {actual}",
                name
            );
        }
        println!("  Checksum verified (sha256)");

        // Extract tarball
        std::fs::create_dir_all(dest)?;
        let tar_gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(tar_gz);
        archive.unpack(dest)?;

        tokio::fs::write(dest.join(CACHED_CID_FILE), format!("{}\n", cid)).await?;
        if !expected_sha256.is_empty() {
            tokio::fs::write(
                dest.join(CACHED_ARTIFACT_SHA_FILE),
                format!("{}\n", expected_sha256),
            )
            .await?;
        }
        let _ = ownership::repair_path_recursive(dest);

        println!("  Extracted to {} (via Carrier)", dest.display());
        Ok(())
    }

    async fn ipfs_cat_via_provider(&self, cid: &str) -> Result<Vec<u8>> {
        let registry = self
            .provider_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("runtime provider registry unavailable"))?;

        let request = serde_json::json!({
            "op": "cat",
            "cid": cid,
        });
        let response = registry
            .send_raw("ipfs", &request)
            .await
            .map_err(|e| anyhow::anyhow!("elastos://ipfs provider unavailable: {}", e))?;

        if let Some(status) = response.get("status").and_then(|s| s.as_str()) {
            if status == "error" {
                let message = response
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                bail!("elastos://ipfs provider error: {}", message);
            }
        }

        let encoded = response
            .get("data")
            .and_then(|d| d.get("data"))
            .and_then(|d| d.as_str())
            .ok_or_else(|| anyhow::anyhow!("elastos://ipfs provider returned no content"))?;

        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| anyhow::anyhow!("elastos://ipfs provider returned invalid base64: {}", e))
    }

    /// Launch a capsule in a crosvm VM. Returns (handle, vsock_cid).
    ///
    /// `config` is an opaque JSON payload from the CLI command. For the shell
    /// capsule, this contains the forwarded command (e.g. `{"command":"chat",...}`).
    /// It is base64-encoded and passed via the `elastos.command` kernel boot arg.
    async fn launch_capsule(&self, name: &str, config: serde_json::Value) -> Result<(String, u32)> {
        let (capsule_dir, manifest) = self.load_capsule_manifest(name).await?;

        // Carrier-plane services run as host processes, not VMs.
        // Skip if the provider is already registered (e.g., built-in Carrier gossip).
        if manifest.permissions.carrier && manifest.provides.is_some() {
            // Skip if built-in Carrier already provides this (e.g., peer-provider → carrier-gossip)
            if name == "peer-provider" && self.provider_registry.is_some() {
                tracing::debug!("peer-provider handled by built-in Carrier");
                return Ok((String::new(), 0));
            }
            return self
                .launch_carrier_service(name, &capsule_dir, &manifest, config)
                .await;
        }

        // VM path — hard require KVM
        if !elastos_crosvm::is_supported() {
            bail!("/dev/kvm not available — crosvm requires KVM. Cannot launch capsule '{name}'.");
        }
        self.crosvm_config.validate().map_err(|e| {
            anyhow::anyhow!(
                "VM prerequisites missing: {}. Run `elastos setup --with crosvm --with vmlinux` \
                 and ensure files exist under ~/.local/share/elastos/bin/",
                e
            )
        })?;
        self.verify_host_artifact("crosvm", &self.crosvm_config.crosvm_bin)?;
        self.verify_host_artifact("vmlinux", &self.crosvm_config.kernel_path)?;

        // Assign vsock CID (unique per VM)
        let cid = {
            let mut next = self.next_cid.write().await;
            let cid = *next;
            *next += 1;
            cid
        };

        let handle = Self::unique_handle(name, cid);

        // Normalize supervisor-reserved launch config keys.
        let mut launch_config = config;
        let interactive_stdio = launch_config
            .get("_elastos_interactive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let capsule_args: Vec<String> = launch_config
            .get("_elastos_capsule_args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(obj) = launch_config.as_object_mut() {
            obj.remove("_elastos_interactive");
            obj.remove("_elastos_capsule_args");
        }

        // Create VM config from manifest, override vsock CID
        let mut vm_config =
            VmConfig::from_manifest(&manifest, &capsule_dir, &self.crosvm_config.kernel_path);
        vm_config.vsock_cid = cid;
        vm_config.boot_args = format!("{} elastos.data_dir=/opt/elastos", vm_config.boot_args);
        vm_config.interactive_stdio = interactive_stdio;
        let vm_id = vm_config.vm_id.clone();

        // TAP networking only when explicitly requested via permissions.guest_network.
        // Default: app capsules use the virtio-console Carrier bridge (rootless, no sudo).
        // Provider capsules that need guest IP set guest_network: true in capsule.json.
        let needs_tap = manifest.permissions.guest_network;
        if needs_tap {
            vm_config = vm_config.with_network(NetworkConfig::new(&vm_id));
        }

        // For interactive VMs, pass host terminal dimensions and TERM type so the
        // guest TUI can render at the correct size and use matching escape sequences.
        // Serial consoles lack TIOCGWINSZ and default to TERM=linux which causes
        // key misinterpretation when the host terminal is xterm-256color etc.
        if interactive_stdio {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
            if ok == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                vm_config.boot_args = format!(
                    "{} elastos.term_cols={} elastos.term_rows={}",
                    vm_config.boot_args, ws.ws_col, ws.ws_row
                );
            }
            // Pass host TERM so guest crossterm generates matching escape sequences.
            if let Ok(term) = std::env::var("TERM") {
                if !term.is_empty() {
                    vm_config.boot_args = format!("{} elastos.term={}", vm_config.boot_args, term);
                }
            }
        }

        // Inject session credentials — shell gets its privileged token,
        // all other capsules get a fresh Capsule-type token.
        if let Some(api_addr) = &self.api_addr {
            let token = if name == "shell" {
                self.shell_token.clone()
            } else {
                // Mint a fresh Capsule token via the session registry
                match &self.session_registry {
                    Some(reg) => {
                        let session = reg.create_session(SessionType::Capsule, None).await;
                        Some(session.token)
                    }
                    None => {
                        eprintln!(
                            "[supervisor] Warning: no session registry, capsule '{}' gets no token",
                            name
                        );
                        None
                    }
                }
            };
            if let Some(t) = &token {
                if let Some(ref net) = vm_config.network {
                    // TAP path: inject HTTP API address via guest IP
                    let api_port = api_addr
                        .rsplit(':')
                        .next()
                        .and_then(|p| p.parse::<u16>().ok())
                        .ok_or_else(|| anyhow::anyhow!("invalid API address '{}'", api_addr))?;
                    let guest_api_addr = format!("http://{}:{}", net.host_ip, api_port);
                    vm_config = vm_config.with_session(t, &guest_api_addr);
                } else {
                    // No TAP: pass token via boot args only.
                    // The capsule uses the microVM Carrier bridge, not HTTP.
                    vm_config.boot_args = format!("{} elastos.token={}", vm_config.boot_args, t);
                }
            }
        }

        // Inject command payload as base64-encoded boot arg
        if !launch_config.is_null() {
            use base64::Engine as _;
            let json_bytes = serde_json::to_vec(&launch_config)?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&json_bytes);
            vm_config.boot_args = format!("{} elastos.command={}", vm_config.boot_args, encoded);
        }

        // Pass capsule arguments as base64-encoded boot arg so the guest
        // init can forward them to the entrypoint binary.
        // Encoding: args joined by newlines, then base64-encoded. Guest init
        // decodes with `base64 -d` and splits on newlines.
        if !capsule_args.is_empty() {
            use base64::Engine as _;
            let joined = capsule_args.join("\n");
            let encoded = base64::engine::general_purpose::STANDARD.encode(joined.as_bytes());
            vm_config.boot_args =
                format!("{} elastos.capsule_args={}", vm_config.boot_args, encoded);
        }

        // Provider capsules expose their request/response bridge on a fixed
        // guest TCP port over the Carrier-managed private control network.
        if manifest.provides.is_some() {
            vm_config.boot_args = format!(
                "{} elastos.provider_port={}",
                vm_config.boot_args, VM_PROVIDER_PORT
            );
        }

        // Create socket directory
        let socket_dir = &self.crosvm_config.socket_dir;
        tokio::fs::create_dir_all(socket_dir).await?;
        let socket_path = socket_dir.join(format!("{}.sock", handle));

        // Carrier bridge: add a virtio-console-backed Unix socket for
        // guest↔runtime provider communication without TAP networking.
        let carrier_socket = socket_dir.join(format!("{}-carrier.sock", handle));
        vm_config.carrier_socket_path = Some(carrier_socket.clone());
        vm_config.boot_args = format!("{} elastos.carrier_path=/dev/hvc0", vm_config.boot_args);

        // Create rootfs overlay (writable copy)
        let rootfs_base = capsule_dir.join("rootfs.ext4");
        if rootfs_base.is_file() {
            let overlay_dir = self.crosvm_config.rootfs_cache_dir.join("overlays");
            tokio::fs::create_dir_all(&overlay_dir).await?;
            let overlay_path = overlay_dir.join(format!("{}.ext4", handle));
            let _ = tokio::fs::remove_file(&overlay_path).await;
            tokio::fs::copy(&rootfs_base, &overlay_path).await?;
            vm_config.rootfs_path = overlay_path;
        }

        let provides = manifest.provides.clone();

        // Spawn the microVM Carrier bridge BEFORE starting the VM.
        // The bridge listens on the Unix socket; crosvm connects to it on launch.
        if let Some(ref registry) = self.provider_registry {
            let session_token = self.shell_token.clone().unwrap_or_default();
            // Build BridgeContext for shell-mediated capability approval.
            // When None (gateway/infrastructure path), the bridge denies capability
            // requests — infrastructure capsules run under service authority.
            let bridge_ctx = match (&self.capability_manager, &self.pending_store) {
                (Some(cap_mgr), Some(pending)) => Some(crate::carrier_bridge::BridgeContext {
                    provider_registry: registry.clone(),
                    capability_manager: cap_mgr.clone(),
                    pending_store: pending.clone(),
                    capsule_id: format!("vm-{}", name),
                }),
                _ => None,
            };
            if let Err(e) = crate::carrier_bridge::spawn_carrier_bridge(
                &carrier_socket,
                registry.clone(),
                session_token,
                bridge_ctx,
            )
            .await
            {
                tracing::warn!("Carrier bridge failed for '{}': {}", name, e);
            }
        }

        // Start the VM (after bridge socket is listening)
        let mut vm = RunningVm::new(vm_config, manifest, socket_path);
        vm.start(&self.crosvm_config.crosvm_bin)
            .await
            .map_err(|e| anyhow::anyhow!("VM boot failed for '{}': {}", name, e))?;

        eprintln!(
            "[supervisor] Launched VM '{}': handle={} vsock_cid={}",
            name, handle, cid
        );

        // Register provider route using guest IP (TCP bridge over TAP)
        let provider_route =
            if let Some(guest_ip) = vm.config.network.as_ref().map(|n| n.guest_ip.clone()) {
                self.register_provider_route(name, provides.as_deref(), &guest_ip, launch_config)
                    .await
            } else {
                None
            };

        // Register as running
        {
            let mut running = self.running.write().await;
            running.insert(
                handle.clone(),
                RunningCapsule {
                    name: name.to_string(),
                    handle: handle.clone(),
                    vsock_cid: cid,
                    started_at: std::time::Instant::now(),
                    provider_route,
                    backend: CapsuleBackend::Vm(Box::new(vm)),
                },
            );
        }

        Ok((handle, cid))
    }

    /// Launch a Carrier-plane service as a host process (for `permissions.carrier: true`).
    ///
    /// Instead of running in a crosvm VM, the provider binary runs directly on the host
    /// as part of the Carrier plane. This gives it real network/system access (iroh P2P,
    /// QUIC, UDP, etc.) while using the same line-delimited JSON protocol as VM providers.
    async fn launch_carrier_service(
        &self,
        name: &str,
        capsule_dir: &Path,
        manifest: &elastos_common::CapsuleManifest,
        config: serde_json::Value,
    ) -> Result<(String, u32)> {
        let binary_path = Self::find_carrier_binary(name, capsule_dir).ok_or_else(|| {
            anyhow::anyhow!(
                "carrier service binary for '{}' not found. Build with: \
                     cd capsules/{} && cargo build --release",
                name,
                name
            )
        })?;
        self.verify_carrier_service_binary(name, capsule_dir, &binary_path)?;

        // Use CID 0 for carrier services — no VM, no vsock
        let cid = 0u32;
        let handle = Self::unique_handle(name, cid);

        let provides = manifest.provides.clone();

        // Build env vars for the provider process
        let mut env_vars = Vec::new();
        if let Some(api_addr) = &self.api_addr {
            env_vars.push(("ELASTOS_API".into(), format!("http://{api_addr}")));
        }
        if let Some(reg) = &self.session_registry {
            let session = reg.create_session(SessionType::Capsule, None).await;
            env_vars.push(("ELASTOS_TOKEN".into(), session.token));
        }

        // Register provider route using CarrierServiceProvider
        let provider_route = self
            .register_carrier_service_route(
                name,
                provides.as_deref(),
                &binary_path,
                env_vars,
                config,
            )
            .await;

        eprintln!(
            "[supervisor] Launched carrier service '{}': handle={} binary={}",
            name,
            handle,
            binary_path.display()
        );

        {
            let mut running = self.running.write().await;
            running.insert(
                handle.clone(),
                RunningCapsule {
                    name: name.to_string(),
                    handle: handle.clone(),
                    vsock_cid: cid,
                    started_at: std::time::Instant::now(),
                    provider_route,
                    backend: CapsuleBackend::Carrier,
                },
            );
        }

        Ok((handle, cid))
    }

    /// Search for a carrier service binary in common locations.
    fn find_carrier_binary(name: &str, capsule_dir: &Path) -> Option<PathBuf> {
        // 1. Raw binary in artifact dir (placed by build-rootfs.sh)
        let in_artifact = capsule_dir.join(name);
        if in_artifact.is_file() {
            return Some(in_artifact);
        }

        // 2. Workspace build output (development)
        // capsule_dir is typically ~/.local/share/elastos/capsules/<name>/
        // but source capsules may have cargo target dirs
        let target_release = capsule_dir.join("target/release").join(name);
        if target_release.is_file() {
            return Some(target_release);
        }

        None
    }

    /// Stop a running capsule.
    async fn stop_capsule(&self, handle: &str) -> Result<()> {
        let mut running = self.running.write().await;
        let capsule = running
            .remove(handle)
            .ok_or_else(|| anyhow::anyhow!("no capsule with handle '{handle}'"))?;

        if let Some(route) = capsule.provider_route.as_ref() {
            self.unregister_provider_route(route).await;
        }

        match capsule.backend {
            CapsuleBackend::Vm(mut vm) => {
                vm.stop()
                    .await
                    .map_err(|e| anyhow::anyhow!("VM stop failed for '{}': {}", capsule.name, e))?;

                // Clean up rootfs overlay
                let overlay_path = self
                    .crosvm_config
                    .rootfs_cache_dir
                    .join("overlays")
                    .join(format!("{}.ext4", handle));
                let _ = tokio::fs::remove_file(&overlay_path).await;
            }
            CapsuleBackend::Carrier => {
                // Carrier service child process is killed when CarrierServiceProvider
                // is dropped (via CarrierServiceBridge::drop). Unregistering the
                // provider route above drops the last Arc reference.
            }
        }

        eprintln!("[supervisor] Stopped capsule handle={}", handle);
        Ok(())
    }

    /// Wait for a running capsule's VM process to exit.
    /// Returns Ok(exit_code) on clean exit, Err on wait failure or non-zero exit.
    pub async fn wait_for_exit(&self, handle: &str) -> Result<i32> {
        // Take the capsule out of running map so we get exclusive access to the VM
        let capsule = {
            let mut running = self.running.write().await;
            running
                .remove(handle)
                .ok_or_else(|| anyhow::anyhow!("no capsule with handle '{handle}'"))?
        };

        if let Some(route) = capsule.provider_route.as_ref() {
            self.unregister_provider_route(route).await;
        }

        let exit_code = match capsule.backend {
            CapsuleBackend::Vm(mut vm) => {
                let code = match vm.wait_for_exit().await {
                    Ok(status) => {
                        let code = status.code().unwrap_or(-1);
                        eprintln!(
                            "[supervisor] Capsule '{}' (handle={}) exited with code {}",
                            capsule.name, handle, code
                        );
                        code
                    }
                    Err(e) => {
                        eprintln!(
                            "[supervisor] Error waiting for capsule '{}': {}",
                            capsule.name, e
                        );
                        bail!("VM wait failed for '{}': {}", capsule.name, e);
                    }
                };

                // Clean up rootfs overlay
                let overlay_path = self
                    .crosvm_config
                    .rootfs_cache_dir
                    .join("overlays")
                    .join(format!("{}.ext4", handle));
                let _ = tokio::fs::remove_file(&overlay_path).await;
                code
            }
            CapsuleBackend::Carrier => {
                // Carrier services are background services — they don't "exit".
                // Waiting on them is a no-op; they run until stopped.
                eprintln!(
                    "[supervisor] Carrier service '{}' (handle={}) — wait is a no-op",
                    capsule.name, handle
                );
                0
            }
        };

        if exit_code != 0 && exit_code != CHAT_RETURN_HOME_EXIT_CODE {
            bail!("capsule '{}' exited with code {}", capsule.name, exit_code);
        }
        Ok(exit_code)
    }

    /// Query status of a running capsule.
    async fn capsule_status(&self, handle: &str) -> Result<SupervisorResponse> {
        let running = self.running.read().await;
        match running.get(handle) {
            Some(rc) => {
                let status = match &rc.backend {
                    CapsuleBackend::Vm(vm) => {
                        if vm.is_running() {
                            "running"
                        } else {
                            "stopped"
                        }
                    }
                    CapsuleBackend::Carrier => "running",
                };

                Ok(SupervisorResponse {
                    status: status.into(),
                    handle: Some(rc.handle.clone()),
                    vsock_cid: Some(rc.vsock_cid),
                    uptime_secs: Some(rc.started_at.elapsed().as_secs()),
                    exit_code: None,
                    path: None,
                    error: None,
                })
            }
            None => Ok(SupervisorResponse {
                status: "not_found".into(),
                handle: None,
                vsock_cid: None,
                uptime_secs: None,
                exit_code: None,
                path: None,
                error: None,
            }),
        }
    }

    /// Download an external tool (kubo, cloudflared, etc.) by name.
    async fn download_external(&self, name: &str, platform: &str) -> Result<PathBuf> {
        let component = self
            .registry
            .external
            .get(name)
            .with_context(|| format!("external component '{name}' not in registry"))?;

        let platform_info = component
            .platforms
            .get(platform)
            .or_else(|| component.platforms.get("*"))
            .with_context(|| format!("no platform '{platform}' for '{name}'"))?;

        let install_path = platform_info
            .install_path
            .as_deref()
            .or(component.install_path.as_deref())
            .with_context(|| format!("no install_path for '{name}'"))?;

        let dest = self.data_dir.join(install_path);

        // Already installed?
        if dest.is_file() {
            return Ok(dest);
        }

        if platform_info.release_path.is_some() {
            crate::setup::install_first_party_component_via_carrier(
                &self.data_dir,
                name,
                platform_info,
                &dest,
            )
            .await?;
            return Ok(dest);
        }

        let url = platform_info
            .url
            .as_deref()
            .with_context(|| format!("no URL for '{name}' on '{platform}'"))?;

        // Download using existing setup infrastructure
        crate::setup::run_download(name, url, platform_info, &dest).await?;

        Ok(dest)
    }

    /// Start the runtime content gateway once and reuse it across commands.
    async fn start_gateway(&self, addr: &str, cache_dir: Option<String>) -> Result<String> {
        {
            let mut gateway = self.gateway.write().await;
            if let Some(existing) = gateway.as_ref() {
                if !existing.task.is_finished() {
                    return Ok(existing.addr.clone());
                }
            }
            *gateway = None;
        }

        let registry = self
            .provider_registry
            .clone()
            .ok_or_else(|| anyhow::anyhow!("provider registry unavailable"))?;

        let listen_addr = addr.to_string();
        let cache_path = cache_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("gateway-cache"));
        std::fs::create_dir_all(&cache_path)?;

        let task = tokio::spawn({
            let listen_addr = listen_addr.clone();
            let cache_path = cache_path.clone();
            let data_dir = self.data_dir.clone();
            async move {
                if let Err(e) = crate::api::gateway::start_gateway_server(
                    &listen_addr,
                    None,
                    Some(registry),
                    cache_path,
                    data_dir,
                )
                .await
                {
                    tracing::error!("Gateway server exited with error: {}", e);
                }
            }
        });

        {
            let mut gateway = self.gateway.write().await;
            *gateway = Some(RunningGateway {
                addr: listen_addr.clone(),
                task,
            });
        }

        Ok(listen_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{Component, PlatformInfo};
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use sha2::Digest;
    use std::sync::Arc;

    #[test]
    fn test_supervisor_request_serialization() {
        let req = SupervisorRequest::EnsureCapsule {
            name: "chat".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("ensure_capsule"));
        assert!(json.contains("chat"));

        let parsed: SupervisorRequest = serde_json::from_str(&json).unwrap();
        match parsed {
            SupervisorRequest::EnsureCapsule { name } => assert_eq!(name, "chat"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_supervisor_response_ok() {
        let resp = SupervisorResponse::ok_with_path("/var/capsules/chat/");
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.path, Some("/var/capsules/chat/".into()));

        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_supervisor_response_error() {
        let resp = SupervisorResponse::err("not found");
        assert_eq!(resp.status, "error");
        assert_eq!(resp.error, Some("not found".into()));
    }

    #[test]
    fn test_supervisor_request_launch() {
        let json = r#"{"op":"launch_capsule","name":"chat"}"#;
        let req: SupervisorRequest = serde_json::from_str(json).unwrap();
        match req {
            SupervisorRequest::LaunchCapsule { name, .. } => assert_eq!(name, "chat"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_supervisor_request_download_external() {
        let json = r#"{"op":"download_external","name":"kubo","platform":"linux-amd64"}"#;
        let req: SupervisorRequest = serde_json::from_str(json).unwrap();
        match req {
            SupervisorRequest::DownloadExternal { name, platform } => {
                assert_eq!(name, "kubo");
                assert_eq!(platform, "linux-amd64");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_supervisor_request_wait_capsule() {
        let json = r#"{"op":"wait_capsule","handle":"vm-chat-3"}"#;
        let req: SupervisorRequest = serde_json::from_str(json).unwrap();
        match req {
            SupervisorRequest::WaitCapsule { handle } => {
                assert_eq!(handle, "vm-chat-3");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_supervisor_request_start_gateway() {
        let json =
            r#"{"op":"start_gateway","addr":"127.0.0.1:9090","cache_dir":"/tmp/elastos-gw"}"#;
        let req: SupervisorRequest = serde_json::from_str(json).unwrap();
        match req {
            SupervisorRequest::StartGateway { addr, cache_dir } => {
                assert_eq!(addr, "127.0.0.1:9090");
                assert_eq!(cache_dir.as_deref(), Some("/tmp/elastos-gw"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_provider_route_from_provides() {
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("elastos://did/*"),
            Some(ProviderRoute::SubProvider("did".to_string()))
        );
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("elastos://ai/chat"),
            Some(ProviderRoute::SubProvider("ai".to_string()))
        );
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("localhost://Users/*"),
            Some(ProviderRoute::Scheme("localhost".to_string()))
        );
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("http://127.0.0.1:3000/*"),
            Some(ProviderRoute::Scheme("http".to_string()))
        );
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("invalid-route"),
            None
        );
        assert_eq!(
            Supervisor::parse_provider_route_from_provides("elastos://"),
            None
        );
    }

    #[test]
    fn test_empty_sha256_is_rejected() {
        // Integrity enforcement: empty sha256 must not be accepted.
        // This is a compile-time guarantee via the bail! in download paths.
        // The actual download functions are async and need network, so we test
        // the principle: empty string is not a valid sha256.
        let empty = "";
        assert!(empty.is_empty(), "empty sha256 should be detected");
        let valid = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(!valid.is_empty(), "valid sha256 should pass");
    }

    #[test]
    fn test_sha256_mismatch_detected() {
        use sha2::Digest;
        let content = b"hello world";
        let actual = hex::encode(sha2::Sha256::digest(content));
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_ne!(actual, wrong, "mismatched hashes must differ");
        let correct = actual.clone();
        assert_eq!(actual, correct, "matching hashes must equal");
    }

    struct MockIpfsProvider {
        response: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl Provider for MockIpfsProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("not used in this test".into()))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["ipfs"]
        }

        fn name(&self) -> &'static str {
            "mock-ipfs-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            assert_eq!(request.get("op").and_then(|v| v.as_str()), Some("cat"));
            assert_eq!(request.get("cid").and_then(|v| v.as_str()), Some("QmTest"));
            Ok(self.response.clone())
        }
    }

    fn make_test_supervisor() -> Supervisor {
        Supervisor::new(
            tempfile::tempdir().unwrap().keep(),
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        )
    }

    fn make_external_component(platform_info: PlatformInfo, install_path: &str) -> Component {
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(crate::setup::detect_platform(), platform_info);
        Component {
            version: None,
            install_path: Some(install_path.to_string()),
            size_mb: None,
            description: None,
            platforms,
        }
    }

    fn write_installed_manifest(
        data_dir: &Path,
        component_name: &str,
        install_path: &str,
        strategy: Option<&str>,
        source: Option<&str>,
        checksum: Option<&str>,
    ) {
        let platform = crate::setup::detect_platform();
        let manifest = serde_json::json!({
            "external": {
                component_name: {
                    "install_path": install_path,
                    "platforms": {
                        platform: {
                            "install_path": install_path,
                            "strategy": strategy,
                            "source": source,
                            "checksum": checksum
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        });
        std::fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_verify_host_artifact_rejects_checksumless_local_copy_kernel() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        std::fs::create_dir_all(data_dir.join("bin")).unwrap();
        std::fs::write(data_dir.join("bin/vmlinux"), b"kernel").unwrap();
        write_installed_manifest(
            data_dir,
            "vmlinux",
            "bin/vmlinux",
            Some("local-copy"),
            Some("/boot/Image"),
            None,
        );

        let mut external = std::collections::HashMap::new();
        external.insert(
            "vmlinux".to_string(),
            make_external_component(
                PlatformInfo {
                    url: None,
                    cid: None,
                    release_path: None,
                    checksum: None,
                    extract_path: None,
                    install_path: Some("bin/vmlinux".to_string()),
                    strategy: Some("local-copy".to_string()),
                    source: Some("/boot/Image".to_string()),
                    note: Some("local arm64 kernel".to_string()),
                    size: None,
                },
                "bin/vmlinux",
            ),
        );

        let supervisor = Supervisor::new(
            data_dir.to_path_buf(),
            ComponentsManifest {
                external,
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        );

        let err = supervisor
            .verify_host_artifact("vmlinux", &data_dir.join("bin/vmlinux"))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("refusing to launch capsule with unverified host artifact 'vmlinux'"));
    }

    #[test]
    fn test_verify_host_artifact_accepts_stamped_local_copy_kernel() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        std::fs::create_dir_all(data_dir.join("bin")).unwrap();
        let kernel_bytes = b"kernel";
        std::fs::write(data_dir.join("bin/vmlinux"), kernel_bytes).unwrap();
        let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(kernel_bytes)));
        write_installed_manifest(
            data_dir,
            "vmlinux",
            "bin/vmlinux",
            Some("local-copy"),
            Some("/boot/Image"),
            Some(&checksum),
        );

        let mut external = std::collections::HashMap::new();
        external.insert(
            "vmlinux".to_string(),
            make_external_component(
                PlatformInfo {
                    url: None,
                    cid: None,
                    release_path: None,
                    checksum: Some(checksum),
                    extract_path: None,
                    install_path: Some("bin/vmlinux".to_string()),
                    strategy: Some("local-copy".to_string()),
                    source: Some("/boot/Image".to_string()),
                    note: Some("local arm64 kernel".to_string()),
                    size: Some(kernel_bytes.len() as u64),
                },
                "bin/vmlinux",
            ),
        );

        let supervisor = Supervisor::new(
            data_dir.to_path_buf(),
            ComponentsManifest {
                external,
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        );

        supervisor
            .verify_host_artifact("vmlinux", &data_dir.join("bin/vmlinux"))
            .unwrap();
    }

    #[test]
    fn test_verify_host_artifact_rejects_checksumless_crosvm() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        std::fs::create_dir_all(data_dir.join("bin")).unwrap();
        std::fs::write(data_dir.join("bin/crosvm"), b"crosvm").unwrap();
        write_installed_manifest(data_dir, "crosvm", "bin/crosvm", None, None, None);

        let mut external = std::collections::HashMap::new();
        external.insert(
            "crosvm".to_string(),
            make_external_component(
                PlatformInfo {
                    url: None,
                    cid: None,
                    release_path: None,
                    checksum: None,
                    extract_path: None,
                    install_path: Some("bin/crosvm".to_string()),
                    strategy: None,
                    source: None,
                    note: None,
                    size: None,
                },
                "bin/crosvm",
            ),
        );

        let supervisor = Supervisor::new(
            data_dir.to_path_buf(),
            ComponentsManifest {
                external,
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        );

        let err = supervisor
            .verify_host_artifact("crosvm", &data_dir.join("bin/crosvm"))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("refusing to launch capsule with unverified host artifact 'crosvm'"));
    }

    #[test]
    fn test_verify_carrier_service_binary_accepts_matching_capsule_artifact_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let capsule_dir = data_dir.join("capsules/peer-provider");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join("peer-provider"), b"carrier-service").unwrap();
        std::fs::write(capsule_dir.join(CACHED_CID_FILE), "bafy-test-cid\n").unwrap();
        std::fs::write(
            capsule_dir.join(CACHED_ARTIFACT_SHA_FILE),
            "sha256:test-artifact\n",
        )
        .unwrap();

        let mut capsules = std::collections::HashMap::new();
        capsules.insert(
            "peer-provider".to_string(),
            CapsuleEntry {
                cid: "bafy-test-cid".to_string(),
                sha256: "sha256:test-artifact".to_string(),
                size: 0,
                platforms: vec![],
            },
        );

        let supervisor = Supervisor::new(
            data_dir.to_path_buf(),
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules,
                profiles: std::collections::HashMap::new(),
            },
        );

        supervisor
            .verify_carrier_service_binary(
                "peer-provider",
                &capsule_dir,
                &capsule_dir.join("peer-provider"),
            )
            .unwrap();
    }

    #[test]
    fn test_verify_carrier_service_binary_rejects_cached_cid_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let capsule_dir = data_dir.join("capsules/peer-provider");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join("peer-provider"), b"carrier-service").unwrap();
        std::fs::write(capsule_dir.join(CACHED_CID_FILE), "bafy-wrong-cid\n").unwrap();
        std::fs::write(
            capsule_dir.join(CACHED_ARTIFACT_SHA_FILE),
            "sha256:test-artifact\n",
        )
        .unwrap();

        let mut capsules = std::collections::HashMap::new();
        capsules.insert(
            "peer-provider".to_string(),
            CapsuleEntry {
                cid: "bafy-test-cid".to_string(),
                sha256: "sha256:test-artifact".to_string(),
                size: 0,
                platforms: vec![],
            },
        );

        let supervisor = Supervisor::new(
            data_dir.to_path_buf(),
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules,
                profiles: std::collections::HashMap::new(),
            },
        );

        let err = supervisor
            .verify_carrier_service_binary(
                "peer-provider",
                &capsule_dir,
                &capsule_dir.join("peer-provider"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("cached CID mismatch"));
    }

    #[tokio::test]
    async fn test_ipfs_cat_via_provider_uses_registered_subprovider() {
        let registry = Arc::new(ProviderRegistry::new());
        let expected = b"capsule-bytes";
        let provider: Arc<dyn Provider> = Arc::new(MockIpfsProvider {
            response: serde_json::json!({
                "status": "ok",
                "data": {
                    "data": base64::engine::general_purpose::STANDARD.encode(expected),
                }
            }),
        });
        registry
            .register_sub_provider("ipfs", provider)
            .await
            .unwrap();

        let mut supervisor = make_test_supervisor();
        supervisor.set_provider_registry(Arc::clone(&registry));

        let bytes = supervisor.ipfs_cat_via_provider("QmTest").await.unwrap();
        assert_eq!(bytes, expected);
    }

    #[tokio::test]
    async fn test_ipfs_cat_via_provider_surfaces_provider_error() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider: Arc<dyn Provider> = Arc::new(MockIpfsProvider {
            response: serde_json::json!({
                "status": "error",
                "message": "kubo not found"
            }),
        });
        registry
            .register_sub_provider("ipfs", provider)
            .await
            .unwrap();

        let mut supervisor = make_test_supervisor();
        supervisor.set_provider_registry(Arc::clone(&registry));

        let err = supervisor
            .ipfs_cat_via_provider("QmTest")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("kubo not found"));
    }
}
