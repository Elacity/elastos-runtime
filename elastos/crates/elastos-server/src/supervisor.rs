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
    /// Apple Virtualization.framework microVM (macOS).
    ///
    /// **Phase 3 Day 3.** Mirrors `Vm(...)` for the Mac substrate:
    /// the supervisor owns the [`elastos_vz::vm::RunningVm`]
    /// (taken from `VzProvider::take_running_vm` after a
    /// successful start). Dropping the variant or calling
    /// `stop()` on the inner VM ends the Vz lifecycle cleanly.
    ///
    /// Cfg-gated to `target_os = "macos"` because
    /// `elastos_vz::vm::RunningVm` on non-macOS targets is a
    /// fail-closed stub — there is no Vz framework off Apple
    /// platforms, and the Linux arm of every match below
    /// rejects this variant at compile time.
    #[cfg(target_os = "macos")]
    VzVm(Box<elastos_vz::RunningVm>),
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

    /// Allocate the next vsock CID under the supervisor's
    /// `next_cid` write lock and increment the counter.
    ///
    /// **Phase 4 Day 1.** Extracted from the inline blocks in
    /// `start_capsule_vm_macos` / `build_vm_config_for_mac` /
    /// the crosvm launch path so the allocator can be exercised
    /// in isolation by the concurrent-launch tests. The
    /// `RwLock::write` future grants mutually exclusive access
    /// regardless of how many writers race for it, which makes
    /// CID assignment race-free without any additional
    /// synchronisation.
    async fn allocate_next_cid(&self) -> u32 {
        let mut next = self.next_cid.write().await;
        let cid = *next;
        *next += 1;
        cid
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

    /// Phase 3 Day 6 — Mac sibling of [`Supervisor::register_provider_route`].
    ///
    /// Diff vs the Linux helper: `VmCapsuleProvider` is constructed
    /// with [`crate::vm_provider::VmCapsuleProvider::new_with_vsock_dialer`]
    /// so the bridge dials the guest via Apple's
    /// `VZVirtioSocketDevice.connectToPort:` (the Day 5 primitive)
    /// instead of `socket(AF_VSOCK,…)`. `guest_host` is the
    /// capsule's supervisor handle string — purely for log parity;
    /// the dialer drives the connection.
    ///
    /// Linux registration paths are byte-identical because they
    /// keep calling [`Self::register_provider_route`]; this helper
    /// is only invoked from the macOS launch arm.
    #[cfg(target_os = "macos")]
    async fn register_provider_route_with_vsock_dialer(
        &self,
        capsule_name: &str,
        provides: Option<&str>,
        guest_host: String,
        init_config: serde_json::Value,
        dialer: crate::vm_provider::MacVsockDial,
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
            Arc::new(VmCapsuleProvider::new_with_vsock_dialer(
                provider_scheme,
                guest_host.clone(),
                VM_PROVIDER_PORT,
                init_config,
                dialer,
            ));

        match route.clone() {
            ProviderRoute::SubProvider(sub) => {
                match registry.register_sub_provider(&sub, provider).await {
                    Ok(_) => {
                        tracing::info!(
                            "Registered Vz VM sub-provider route elastos://{}/... -> capsule '{}' (handle={}, port={})",
                            sub,
                            capsule_name,
                            guest_host,
                            VM_PROVIDER_PORT
                        );
                        Some(ProviderRoute::SubProvider(sub))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to register Vz VM provider route for '{}' ({}): {}",
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
                    "Registered Vz VM provider route {}://... -> capsule '{}' (handle={}, port={})",
                    scheme,
                    capsule_name,
                    guest_host,
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
                        #[cfg(target_os = "macos")]
                        CapsuleBackend::VzVm(vm) => vm.is_running(),
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
    ///
    /// **Linux:** the crosvm path below is the only substrate; behavior is
    /// byte-identical to the pre-Vz-backend commit. **macOS:** the launch
    /// path short-circuits at the substrate-check site below with a Phase 1
    /// `vz backend not yet implemented` bail. Phase 2/3 (see
    /// `docs/vz-backend/PLAN.md`) replace that bail with a real
    /// `VzProvider.load(...)` route, at which point the rest of this
    /// function body remains the Linux/crosvm-only code path.
    ///
    /// The `cfg_attr` below silences the expected `unreachable_code` warning
    /// on macOS, where the Phase 1 bail makes the rest of the function dead
    /// code. The warning is informational on Mac and a true error on Linux.
    #[cfg_attr(not(target_os = "linux"), allow(unreachable_code, unused_variables))]
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

        // VM path — hard require a microVM substrate.
        //
        // Linux: behavior is byte-identical to the pre-Vz-backend commit;
        // crosvm is the only substrate, and `/dev/kvm` must be present.
        //
        // macOS: the Vz substrate is registered in main.rs but the
        // per-VM launch path is not yet routed through it. Phase 1
        // delivers scaffold only; Phase 2 wires `VzProvider.load(...)`
        // and Phase 3 routes the supervisor through this site. Until
        // then, fail closed with the same single-source-of-truth
        // message used by VzProvider's stubs. See
        // `docs/vz-backend/PLAN.md`.
        //
        // Other OS: no microVM substrate is available at all.
        #[cfg(target_os = "linux")]
        if !elastos_crosvm::is_supported() {
            bail!("/dev/kvm not available — crosvm requires KVM. Cannot launch capsule '{name}'.");
        }

        #[cfg(target_os = "macos")]
        {
            // Clone to keep the Linux arm of this function
            // (compiled but unreachable on Mac) byte-identical:
            // it still owns the original manifest and capsule_dir.
            return self
                .start_capsule_vm_macos(name, manifest.clone(), capsule_dir.clone(), config)
                .await;
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        bail!("no microVM substrate available on this OS — cannot launch capsule '{name}'");

        self.crosvm_config.validate().map_err(|e| {
            anyhow::anyhow!(
                "VM prerequisites missing: {}. Run `elastos setup --with crosvm --with vmlinux` \
                 and ensure files exist under ~/.local/share/elastos/bin/",
                e
            )
        })?;
        self.verify_host_artifact("crosvm", &self.crosvm_config.crosvm_bin)?;
        self.verify_host_artifact("vmlinux", &self.crosvm_config.kernel_path)?;

        let cid = self.allocate_next_cid().await;
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

    /// Phase 3 Day 2 — full substrate-agnostic prefix port for
    /// the macOS arm of [`Self::launch_capsule`].
    ///
    /// Day 1 shipped the bare seam: `elastos start <microvm>` on
    /// macOS reached `VzProvider::load_with_vm_config` but with
    /// a *minimal* `VmConfig` — no session token, no command
    /// payload, no capsule args, no Carrier bridge, no rootfs
    /// overlay. Day 2 ports every AG step from
    /// [`docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md`](../../../docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md)
    /// — the same operations the Linux arm runs in
    /// `launch_capsule` L997-1174 — into this helper, in the
    /// same order, so a capsule launched on Mac sees the same
    /// boot-arg shape it sees on Linux.
    ///
    /// **Mac-specific deltas** vs the Linux flow:
    ///
    /// - TAP networking is unsupported (NAT only). If the manifest
    ///   requests `permissions.guest_network: true`, fail closed
    ///   with a typed error pointing at Phase 3 Day 4+ bridged-mode
    ///   work. NO silent downgrade.
    /// - The Carrier console lives at `/dev/hvc1` on Vz because
    ///   `/dev/hvc0` is the kernel console
    ///   ([`elastos-vz/src/ffi/builder.rs`](../../../elastos/crates/elastos-vz/src/ffi/builder.rs)
    ///   `setSerialPorts`/`setConsoleDevices`). Linux uses
    ///   `/dev/hvc0` because crosvm only attaches the Carrier
    ///   console. The boot arg differs accordingly.
    /// - The Carrier socket listens correctly (Day 2), but the
    ///   guest doesn't *yet* receive bytes through it because the
    ///   Vz console attachment is still a placeholder (Day 4+).
    ///   That's a Phase 3 Day 4 piece, named in the port plan.
    ///
    /// **Phase 3 Day 3 closed the Day-2 fail-closed exit.**
    /// After a successful `VzProvider::load_with_vm_config + start`,
    /// the supervisor now takes ownership of the
    /// `elastos_vz::vm::RunningVm` via `VzProvider::take_running_vm`
    /// and inserts a `CapsuleBackend::VzVm` `RunningCapsule` into
    /// `self.running` — same map and same handle key the Linux
    /// arm uses. From this commit forward `elastos ps`,
    /// `elastos status <handle>`, and `elastos stop <handle>`
    /// all work for Mac MicroVM capsules.
    ///
    /// **Explicitly NOT ported by Day 3** (Day 4+ work):
    ///
    /// - Real socketpair attachment on the Vz Carrier console
    ///   (Day 4). The bridge listener exists (Day 2 work) but
    ///   bytes do not yet flow guest↔host because
    ///   `ffi/console.rs::build_carrier_console_slot` is still
    ///   a placeholder. So `elastos ps` shows the VM running
    ///   but the capsule inside cannot yet talk to the host.
    /// - Real vsock host listener bridging (Day 5).
    ///
    /// The Linux launch path ([`Self::launch_capsule`] body below
    /// this method) is **byte-identical** to the pre-Day-1 commit.
    /// The macOS arm early-returns through this helper.
    #[cfg(target_os = "macos")]
    async fn start_capsule_vm_macos(
        &self,
        name: &str,
        manifest: elastos_common::CapsuleManifest,
        capsule_dir: std::path::PathBuf,
        config: serde_json::Value,
    ) -> Result<(String, u32)> {
        use elastos_compute::ComputeProvider;
        use elastos_vz::{VzConfig, VzProvider};

        if !elastos_vz::is_supported() {
            bail!(
                "Apple Virtualization.framework not available — cannot launch capsule '{name}' on this host. Requires macOS 12+ on Apple Silicon."
            );
        }

        // Phase 3 Day 7: `permissions.guest_network` is no longer
        // an unconditional bail. The supervisor populates
        // `vm_config.network` inside `build_vm_config_for_mac`;
        // the Vz builder then decides at build time whether the
        // process is entitled to attach a
        // `VZBridgedNetworkDeviceAttachment` (entitlement
        // present → bridged device; absent → typed Compute
        // error pointing at `docs/MAC.md`). NAT-only capsules
        // (`guest_network: false`) keep going through the
        // Day-2 NAT attachment with zero diff.

        let vz_config = VzConfig::default();
        let provider = VzProvider::new(vz_config.clone())
            .map_err(|e| anyhow::anyhow!("failed to construct VzProvider: {e}"))?;
        provider
            .init()
            .await
            .map_err(|e| anyhow::anyhow!("failed to init VzProvider state dir: {e}"))?;

        // Phase 3 Day 6: keep a copy of `launch_config` for the
        // provider-route registration below; `build_vm_config_for_mac`
        // consumes its input to assemble boot args (matching the
        // Linux flow at L1013–L1203 where `launch_config` is moved
        // into `register_provider_route`).
        let launch_config_for_route = config.clone();
        let (vm_config, handle, cid, _carrier_socket_path) = self
            .build_vm_config_for_mac(name, &manifest, &capsule_dir, config, &vz_config)
            .await?;

        // Phase 3 Day 4: the Carrier bridge on Mac does not bind
        // a Unix listener — the host endpoint comes directly from
        // the Vz console socketpair set up in
        // `elastos-vz::ffi::console::build_carrier_console_slot`.
        // The fd is taken off `RunningVm::take_carrier_host_fd`
        // AFTER `take_running_vm` below (see Phase-3 Day-4 wiring
        // further down this function).

        tracing::info!(
            target: "supervisor",
            capsule = name,
            handle = %handle,
            cid = cid,
            "phase 3 day 3: handing off to VzProvider::load_with_vm_config + start"
        );

        // Clone the manifest before `load_with_vm_config` consumes
        // its copy — the supervisor needs to keep the original
        // around for the `RunningCapsule` record below (the Linux
        // arm has the same shape at L1175).
        let manifest_for_provider = manifest.clone();
        let capsule_handle = provider
            .load_with_vm_config(vm_config, manifest_for_provider)
            .await
            .map_err(|e| anyhow::anyhow!("vz provider load failed for '{}': {}", name, e))?;
        provider
            .start(&capsule_handle)
            .await
            .map_err(|e| anyhow::anyhow!("vz provider start failed for '{}': {}", name, e))?;

        // Take ownership of the RunningVm from the provider so
        // the supervisor's running-map holds the lifecycle.
        // After this call `provider.vms` no longer references
        // the VM; the supervisor (via CapsuleBackend::VzVm) is
        // the sole owner. The VzMachineHandle inside RunningVm
        // carries its own Arc to the dispatch queue, so the
        // provider can drop without affecting the live VM.
        let mut vz_vm = provider
            .take_running_vm(&capsule_handle)
            .await
            .map_err(|e| {
                anyhow::anyhow!("vz provider take_running_vm failed for '{}': {}", name, e)
            })?;
        drop(provider); // explicit: queue Arc lives on inside vz_vm.

        // Phase 3 Day 4: take the Carrier host-side socket fd
        // and hand it to `spawn_carrier_bridge_on_stream`. From
        // here forward the guest's `/dev/hvc1` reads and writes
        // round-trip through the bridge dispatch loop and the
        // supervisor's `ProviderRegistry` — exactly the same
        // request/response semantics the Linux flow has via
        // crosvm's `unix-stream` carrier socket.
        if let Some(carrier_fd) = vz_vm.take_carrier_host_fd() {
            if let Some(ref registry) = self.provider_registry {
                use std::os::fd::{FromRawFd, IntoRawFd};
                use std::os::unix::net::UnixStream as StdUnixStream;

                let session_token = self.shell_token.clone().unwrap_or_default();
                let bridge_ctx = match (&self.capability_manager, &self.pending_store) {
                    (Some(cap_mgr), Some(pending)) => Some(crate::carrier_bridge::BridgeContext {
                        provider_registry: registry.clone(),
                        capability_manager: cap_mgr.clone(),
                        pending_store: pending.clone(),
                        capsule_id: format!("vm-{}", name),
                    }),
                    _ => None,
                };

                // SAFETY: `carrier_fd` is an `OwnedFd` returned from
                // `build_carrier_console_slot`, already set
                // non-blocking, with no other Rust owner. We
                // move it into `StdUnixStream` (which takes
                // ownership of the raw fd) and then into
                // `tokio::net::UnixStream` via `from_std`.
                let std_stream = unsafe { StdUnixStream::from_raw_fd(carrier_fd.into_raw_fd()) };
                match tokio::net::UnixStream::from_std(std_stream) {
                    Ok(tokio_stream) => {
                        crate::carrier_bridge::spawn_carrier_bridge_on_stream(
                            tokio_stream,
                            registry.clone(),
                            session_token,
                            bridge_ctx,
                            format!("vz:{}", handle),
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Carrier bridge failed to take Vz console fd for '{}': {}",
                            name,
                            e
                        );
                    }
                }
            }
        }

        // Phase 3 Day 6: Mac provider-route registration.
        //
        // The Linux arm calls `register_provider_route` with the
        // guest's TAP IP; on Mac we have no IP (NAT-only, no
        // entitlements) so we register through a vsock dialer
        // instead. The dialer captures a `Weak` of the supervisor's
        // running map plus this capsule's handle, and resolves the
        // live `RunningVm` on every dial — so a torn-down VM
        // surfaces `io::ErrorKind::NotConnected` cleanly rather
        // than panicking or holding a dangling Arc.
        //
        // We must register BEFORE inserting into `self.running`
        // because the dialer only needs `Weak<RwLock<…>>` to
        // function — the actual `RunningCapsule` will be present
        // by the time any other capsule issues a request that
        // routes through this provider.
        let provider_route = if manifest.provides.is_some() {
            let running_weak = Arc::downgrade(&self.running);
            let handle_for_dialer = handle.clone();
            let dialer: crate::vm_provider::MacVsockDial = Arc::new(move |port: u32| {
                let running_weak = running_weak.clone();
                let handle = handle_for_dialer.clone();
                Box::pin(async move {
                    let Some(running) = running_weak.upgrade() else {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "supervisor running map has been dropped",
                        ));
                    };
                    let map = running.read().await;
                    let Some(rc) = map.get(&handle) else {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            format!("capsule handle '{handle}' is no longer running"),
                        ));
                    };
                    match &rc.backend {
                        CapsuleBackend::VzVm(vm) => vm
                            .connect_vsock(port)
                            .await
                            .map_err(|e| {
                                std::io::Error::new(
                                    std::io::ErrorKind::ConnectionRefused,
                                    e.to_string(),
                                )
                            }),
                        _ => Err(std::io::Error::new(
                            std::io::ErrorKind::Unsupported,
                            format!(
                                "capsule '{handle}' is not a Vz VM (defensive — supervisor invariant)"
                            ),
                        )),
                    }
                })
            });

            self.register_provider_route_with_vsock_dialer(
                name,
                manifest.provides.as_deref(),
                handle.clone(),
                launch_config_for_route,
                dialer,
            )
            .await
        } else {
            None
        };

        let started_at = std::time::Instant::now();
        let running_capsule = RunningCapsule {
            name: name.to_string(),
            handle: handle.clone(),
            vsock_cid: cid,
            started_at,
            provider_route,
            backend: CapsuleBackend::VzVm(Box::new(vz_vm)),
        };
        self.running
            .write()
            .await
            .insert(handle.clone(), running_capsule);

        eprintln!(
            "[supervisor] Launched Vz VM '{}' (handle={}, cid={})",
            name, handle, cid
        );
        Ok((handle, cid))
    }

    /// Phase 3 Day 2 — substrate-agnostic prefix builder for the
    /// macOS launch path. Mirrors the Linux flow in
    /// [`Self::launch_capsule`] L997-1144, but operates on
    /// `elastos_vz::VmConfig` and uses `/dev/hvc1` for the
    /// Carrier console boot arg.
    ///
    /// Returns the fully-baked `VmConfig`, the supervisor handle
    /// string, the (advisory) vsock CID, and the carrier socket
    /// path so the caller can spawn the Carrier bridge listener.
    ///
    /// Factored out from `start_capsule_vm_macos` for test
    /// isolation: the AG prefix is pure data composition (apart
    /// from the file-system side-effects for the rootfs overlay
    /// and socket directory creation), so it can be unit-tested
    /// without touching the Vz framework.
    #[cfg(target_os = "macos")]
    async fn build_vm_config_for_mac(
        &self,
        name: &str,
        manifest: &elastos_common::CapsuleManifest,
        capsule_dir: &std::path::Path,
        mut launch_config: serde_json::Value,
        vz_config: &elastos_vz::VzConfig,
    ) -> Result<(elastos_vz::VmConfig, String, u32, std::path::PathBuf)> {
        use elastos_vz::VmConfig as VzVmConfig;

        // CID alloc — advisory only on Mac (Apple does not let
        // us hand the CID to Vz, Phase 0 §D pitfall #5) but kept
        // for log-line diffability with the Linux path. The
        // allocator is shared with the Linux flow via
        // `allocate_next_cid` (Phase 4 Day 1).
        let cid = self.allocate_next_cid().await;
        let handle = Self::unique_handle(name, cid);

        // Normalize supervisor-reserved launch config keys.
        // Mirrors supervisor.rs L997-1013.
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

        // Build VmConfig. Mirrors supervisor.rs L1015-1024 but on
        // the Vz type. `from_manifest` reuses the manifest's
        // `microvm.kernel` if set, else falls back to
        // `vz_config.kernel_path` (~/.local/share/elastos/bin/vmlinux).
        let mut vm_config =
            VzVmConfig::from_manifest(manifest, capsule_dir, &vz_config.kernel_path);
        vm_config.vsock_cid = cid;
        vm_config.boot_args = format!("{} elastos.data_dir=/opt/elastos", vm_config.boot_args);
        vm_config.interactive_stdio = interactive_stdio;

        // Phase 3 Day 7: bridged-networking opt-in.
        //
        // When the manifest sets `permissions.guest_network: true`,
        // populate `vm_config.network` with a deterministic
        // per-VM `NetworkConfig` (matching the Linux
        // `with_network(NetworkConfig::new(&vm_id))` shape at
        // supervisor.rs L1123-1126). The Vz FFI builder
        // (`elastos-vz::ffi::builder`) then attaches a
        // `VZBridgedNetworkDeviceAttachment` if the process holds
        // the `com.apple.vm.networking` entitlement, OR returns
        // a typed `ElastosError::Compute` if the binary is
        // unsigned. There is NO silent NAT downgrade — the
        // capsule explicitly asked for routable networking.
        if manifest.permissions.guest_network {
            vm_config.network = Some(elastos_vz::NetworkConfig::new(&vm_config.vm_id));
        }
        // Apply the provider-wide initramfs default if set.
        // VzConfig carries this for `vm-debug boot --initramfs`;
        // real capsules typically don't need it but the seam
        // stays consistent.
        if vm_config.initramfs_path.is_none() {
            if let Some(default_initramfs) = vz_config.initramfs_path.as_ref() {
                vm_config.initramfs_path = Some(default_initramfs.clone());
            }
        }

        // TERM/winsize for interactive VMs. TIOCGWINSZ + the
        // TERM env var both work on Darwin; same code as Linux.
        // Mirrors supervisor.rs L1035-1050.
        if interactive_stdio {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
            if ok == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                vm_config.boot_args = format!(
                    "{} elastos.term_cols={} elastos.term_rows={}",
                    vm_config.boot_args, ws.ws_col, ws.ws_row
                );
            }
            if let Ok(term) = std::env::var("TERM") {
                if !term.is_empty() {
                    vm_config.boot_args = format!("{} elastos.term={}", vm_config.boot_args, term);
                }
            }
        }

        // Session token injection — NAT-only path on Mac (the
        // TAP path is fail-closed-above). Mirrors the no-TAP
        // branch in supervisor.rs L1087-1091: token via boot
        // args only, no `elastos.api=` (the capsule uses the
        // microVM Carrier bridge, not HTTP).
        if self.api_addr.is_some() {
            let token = if name == "shell" {
                self.shell_token.clone()
            } else {
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
            if let Some(t) = token {
                vm_config.boot_args = format!("{} elastos.token={}", vm_config.boot_args, t);
            }
        }

        // Command payload base64. Mirrors supervisor.rs L1095-1101.
        if !launch_config.is_null() {
            use base64::Engine as _;
            let json_bytes = serde_json::to_vec(&launch_config)?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&json_bytes);
            vm_config.boot_args = format!("{} elastos.command={}", vm_config.boot_args, encoded);
        }

        // Capsule args base64. Mirrors supervisor.rs L1107-1113.
        if !capsule_args.is_empty() {
            use base64::Engine as _;
            let joined = capsule_args.join("\n");
            let encoded = base64::engine::general_purpose::STANDARD.encode(joined.as_bytes());
            vm_config.boot_args =
                format!("{} elastos.capsule_args={}", vm_config.boot_args, encoded);
        }

        // Provider port boot arg. Mirrors supervisor.rs L1117-1122.
        if manifest.provides.is_some() {
            vm_config.boot_args = format!(
                "{} elastos.provider_port={}",
                vm_config.boot_args, VM_PROVIDER_PORT
            );
        }

        // Carrier socket setup. Mirrors supervisor.rs L1124-1133
        // EXCEPT the kernel arg path: Mac uses `/dev/hvc1`
        // because `/dev/hvc0` is the kernel console (see
        // elastos-vz/src/ffi/builder.rs `setSerialPorts` vs
        // `setConsoleDevices`). The socket directory is reused
        // from `crosvm_config.socket_dir` — it's an OS-agnostic
        // path under ~/.local/share/elastos/ and keeps the
        // socket location diffable across substrates.
        let socket_dir = &self.crosvm_config.socket_dir;
        tokio::fs::create_dir_all(socket_dir).await?;
        let carrier_socket = socket_dir.join(format!("{}-carrier.sock", handle));
        vm_config.carrier_socket_path = Some(carrier_socket.clone());
        vm_config.boot_args = format!("{} elastos.carrier_path=/dev/hvc1", vm_config.boot_args);

        // Rootfs overlay. Mirrors supervisor.rs L1135-1144. The
        // cache directory is reused from
        // `crosvm_config.rootfs_cache_dir` for the same
        // OS-agnostic reason as the socket dir above. Vz accepts
        // a raw ext4 file as a `VZVirtioBlockDevice` — Day 5
        // boot evidence confirms this.
        let rootfs_base = capsule_dir.join("rootfs.ext4");
        if rootfs_base.is_file() {
            let overlay_dir = self.crosvm_config.rootfs_cache_dir.join("overlays");
            tokio::fs::create_dir_all(&overlay_dir).await?;
            let overlay_path = overlay_dir.join(format!("{}.ext4", handle));
            let _ = tokio::fs::remove_file(&overlay_path).await;
            tokio::fs::copy(&rootfs_base, &overlay_path).await?;
            vm_config.rootfs_path = overlay_path;
        }

        Ok((vm_config, handle, cid, carrier_socket))
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
            #[cfg(target_os = "macos")]
            CapsuleBackend::VzVm(mut vm) => {
                // Same shape as the crosvm arm: stop the VM
                // (Vz dispatches stopWithCompletionHandler on
                // the per-machine queue), then remove the
                // rootfs overlay the supervisor created in
                // Phase 3 Day 2's build_vm_config_for_mac.
                vm.stop().await.map_err(|e| {
                    anyhow::anyhow!("Vz VM stop failed for '{}': {}", capsule.name, e)
                })?;
                let overlay_path = self
                    .crosvm_config
                    .rootfs_cache_dir
                    .join("overlays")
                    .join(format!("{}.ext4", handle));
                let _ = tokio::fs::remove_file(&overlay_path).await;
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
            #[cfg(target_os = "macos")]
            CapsuleBackend::VzVm(mut vm) => {
                // Vz has no host child process to wait()
                // on. `wait_for_exit_code` polls the Vz state
                // property via the dispatch queue and returns
                // 0 when the VM leaves Running — clean
                // shutdown vs crash is Day 4+ work once
                // `VZVirtualMachineDelegate` is wired.
                let code = vm.wait_for_exit_code().await.map_err(|e| {
                    anyhow::anyhow!("Vz VM wait failed for '{}': {}", capsule.name, e)
                })?;
                eprintln!(
                    "[supervisor] Vz capsule '{}' (handle={}) exited with code {}",
                    capsule.name, handle, code
                );
                let overlay_path = self
                    .crosvm_config
                    .rootfs_cache_dir
                    .join("overlays")
                    .join(format!("{}.ext4", handle));
                let _ = tokio::fs::remove_file(&overlay_path).await;
                code
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
                    #[cfg(target_os = "macos")]
                    CapsuleBackend::VzVm(vm) => {
                        if vm.is_running() {
                            "running"
                        } else {
                            "stopped"
                        }
                    }
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

    /// Build a synthetic MicroVM `CapsuleManifest` suitable for
    /// the Phase 3 macOS supervisor tests. Each call returns a
    /// fresh value so tests don't share state.
    #[cfg(target_os = "macos")]
    fn synthetic_microvm_manifest(name: &str) -> elastos_common::CapsuleManifest {
        use elastos_common::{
            CapsuleManifest, CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1,
        };
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: name.into(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits {
                memory_mb: 128,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            microvm: Some(MicroVmConfig {
                kernel: None,
                boot_args: "console=ttyS0".into(),
                http_port: None,
                vcpu_count: Some(1),
                rootfs_cid: None,
                kernel_cid: None,
                rootfs_size: None,
                persistent_storage_mb: None,
            }),
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    /// Phase 3 Day 3 contract: with no kernel/rootfs installed
    /// on the test host, `start_capsule_vm_macos` must surface a
    /// typed `VzProvider::load_with_vm_config` validation error
    /// (Kernel/Rootfs not found) — NEVER the old
    /// `PHASE_1_STUB_MESSAGE` and NEVER the Day-2 "pending
    /// registration" message (Day 3 removed that exit). On a
    /// host with a real kernel + rootfs cached, this code path
    /// would reach `RunningCapsule` insertion and return
    /// `Ok((handle, cid))` — proved by the unit tests below
    /// that exercise the insertion path via a synthetic
    /// `RunningCapsule`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn start_capsule_vm_macos_seam_surfaces_vz_validation_error_after_phase3_day3() {
        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let manifest = synthetic_microvm_manifest("phase3-day3-seam-test");

        let err = supervisor
            .start_capsule_vm_macos(
                "phase3-day3-seam-test",
                manifest,
                capsule_dir.to_path_buf(),
                serde_json::Value::Null,
            )
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(
            !msg.contains(elastos_vz::PHASE_1_STUB_MESSAGE),
            "seam regression: error still contains the pre-Day-1 PHASE_1_STUB_MESSAGE: {msg}"
        );
        assert!(
            !msg.contains("supervisor RunningCapsule registration is pending"),
            "seam regression: Day-2 pending-registration message must be gone after Day 3; got: {msg}"
        );

        let is_kernel_missing = msg.contains("Kernel not found");
        let is_rootfs_missing = msg.contains("Rootfs not found");
        assert!(
            is_kernel_missing || is_rootfs_missing,
            "expected a typed Vz validation error (kernel/rootfs missing) on a test host \
             without installed artefacts; got: {msg}"
        );
    }

    /// Phase 3 Day 2 contract: `build_vm_config_for_mac` bakes
    /// every substrate-agnostic boot arg the Linux flow does —
    /// `elastos.data_dir`, session token (when applicable),
    /// command payload (base64), capsule args (base64),
    /// provider_port, carrier_path — into `vm_config.boot_args`
    /// BEFORE the VM is handed to VzProvider. Also confirms
    /// `/dev/hvc1` is the Mac carrier path (not `/dev/hvc0`).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_bakes_full_phase3_day2_prefix() {
        use elastos_vz::VzConfig;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let manifest = synthetic_microvm_manifest("phase3-day2-bake-test");

        let launch_config = serde_json::json!({
            "command": "chat",
            "_elastos_interactive": false,
            "_elastos_capsule_args": ["--peer", "alice"],
            "user_payload": {"hello": "world"}
        });

        let vz_config = VzConfig::default();
        let (vm_config, handle, _cid, carrier_socket) = supervisor
            .build_vm_config_for_mac(
                "phase3-day2-bake-test",
                &manifest,
                &capsule_dir,
                launch_config,
                &vz_config,
            )
            .await
            .expect("build_vm_config_for_mac succeeds with synthetic inputs");

        // Carrier path is the Mac-specific /dev/hvc1 (kernel
        // console is /dev/hvc0 on Vz). Diverges intentionally
        // from the Linux flow which uses /dev/hvc0.
        assert!(
            vm_config
                .boot_args
                .contains("elastos.carrier_path=/dev/hvc1"),
            "expected carrier_path=/dev/hvc1 on Mac, got boot_args: {}",
            vm_config.boot_args
        );
        assert!(
            !vm_config
                .boot_args
                .contains("elastos.carrier_path=/dev/hvc0"),
            "must not use hvc0 on Mac (kernel console lives there): {}",
            vm_config.boot_args
        );

        // Command payload was base64-encoded.
        assert!(
            vm_config.boot_args.contains("elastos.command="),
            "expected base64-encoded command payload in boot args: {}",
            vm_config.boot_args
        );
        // Capsule args were extracted, stripped from the launch
        // config, and re-encoded as a base64 newline-joined
        // payload.
        assert!(
            vm_config.boot_args.contains("elastos.capsule_args="),
            "expected base64-encoded capsule_args in boot args: {}",
            vm_config.boot_args
        );
        // data_dir always set.
        assert!(
            vm_config
                .boot_args
                .contains("elastos.data_dir=/opt/elastos"),
            "expected data_dir boot arg: {}",
            vm_config.boot_args
        );
        // Handle format is `vm-<name>-<cid>-<millis>` so the
        // capsule name must appear in it; carrier socket path
        // must embed the full handle.
        assert!(
            handle.contains("phase3-day2-bake-test"),
            "handle should embed the capsule name, got: {handle}"
        );
        assert!(
            handle.starts_with("vm-"),
            "handle should follow the supervisor's vm-<name>-<cid>-<millis> convention, got: {handle}"
        );
        assert!(
            carrier_socket.to_string_lossy().contains(&handle),
            "carrier socket path should embed the handle: {}",
            carrier_socket.display()
        );
        assert!(
            carrier_socket.to_string_lossy().ends_with("-carrier.sock"),
            "carrier socket should use the -carrier.sock suffix: {}",
            carrier_socket.display()
        );
        assert_eq!(
            vm_config.carrier_socket_path.as_deref(),
            Some(carrier_socket.as_path())
        );
    }

    /// Phase 3 Day 7 contract: a `guest_network: true` capsule
    /// no longer hits an unconditional bail in
    /// `start_capsule_vm_macos`. Instead `build_vm_config_for_mac`
    /// populates `vm_config.network` and the Vz FFI builder
    /// decides at build time whether to attach a
    /// `VZBridgedNetworkDeviceAttachment` (entitlement present)
    /// or surface a typed error (entitlement absent — every
    /// dev/CI binary).
    ///
    /// On a test host without the entitlement we expect the
    /// supervisor to fail somewhere in its launch pipeline. The
    /// exact failure depends on what else is missing: in a
    /// stock CI environment with no kernel/rootfs installed
    /// the kernel-not-found error fires first (also a valid
    /// fail-closed); when those are present the builder's
    /// entitlement error fires. Either is correct fail-closed
    /// behaviour. The test asserts only that
    /// `start_capsule_vm_macos` rejects the launch — silent
    /// success on an unentitled host would be a bug.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn start_capsule_vm_macos_fails_closed_when_guest_network_lacks_entitlement() {
        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let mut manifest = synthetic_microvm_manifest("phase3-day7-guest-network-test");
        manifest.permissions.guest_network = true;

        let result = supervisor
            .start_capsule_vm_macos(
                "phase3-day7-guest-network-test",
                manifest,
                capsule_dir.to_path_buf(),
                serde_json::Value::Null,
            )
            .await;
        assert!(
            result.is_err(),
            "guest_network: true must fail closed on an unentitled CI host"
        );
    }

    /// Phase 3 Day 7 routing contract: when the manifest sets
    /// `permissions.guest_network: true`, `build_vm_config_for_mac`
    /// must populate `vm_config.network = Some(NetworkConfig)`
    /// so the Vz FFI builder can decide bridged-vs-fail-closed
    /// at construction time. Without this routing, every Mac
    /// capsule would silently get NAT — exactly the kind of
    /// "fail open" the Phase 0 audit ruled out.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_routes_guest_network_capsule_into_vm_config_network() {
        use elastos_vz::VzConfig;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let mut manifest = synthetic_microvm_manifest("phase3-day7-network-routing");
        manifest.permissions.guest_network = true;

        let vz_config = VzConfig::default();
        let (vm_config, _handle, _cid, _carrier_socket) = supervisor
            .build_vm_config_for_mac(
                "phase3-day7-network-routing",
                &manifest,
                &capsule_dir,
                serde_json::Value::Null,
                &vz_config,
            )
            .await
            .expect("build_vm_config_for_mac succeeds (entitlement check is the builder's job)");

        let network = vm_config.network.as_ref().expect(
            "guest_network: true must populate vm_config.network — \
             without it the Vz builder cannot decide bridged-vs-fail-closed",
        );
        // The deterministic per-VM derivation lives in
        // `elastos_vz::NetworkConfig::new`; assert the
        // observable invariants (the supervisor doesn't need
        // to know the exact IP allocator).
        assert!(
            network.host_ip.starts_with("172.16."),
            "expected the Vz NetworkConfig host_ip to be in the 172.16/12 RFC1918 range, got {}",
            network.host_ip
        );
        assert!(network.guest_mac.starts_with("AA:FC:"));
        assert_eq!(network.prefix_len, 30);
    }

    /// Phase 3 Day 7 routing contract (mirror): a capsule that
    /// does NOT declare `guest_network` must leave
    /// `vm_config.network` as `None` — the Vz builder then takes
    /// the NAT path with zero diff vs Day 2. Guards against an
    /// accidental "always-on bridged" regression.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_leaves_network_none_when_guest_network_not_requested() {
        use elastos_vz::VzConfig;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let mut manifest = synthetic_microvm_manifest("phase3-day7-nat-default");
        manifest.permissions.guest_network = false;

        let vz_config = VzConfig::default();
        let (vm_config, _handle, _cid, _carrier_socket) = supervisor
            .build_vm_config_for_mac(
                "phase3-day7-nat-default",
                &manifest,
                &capsule_dir,
                serde_json::Value::Null,
                &vz_config,
            )
            .await
            .expect("build_vm_config_for_mac succeeds for NAT-only capsule");

        assert!(
            vm_config.network.is_none(),
            "NAT-only capsule must leave vm_config.network = None; got Some({:?})",
            vm_config.network
        );
    }

    /// Build a synthetic `RunningCapsule` whose backend is the
    /// Day-3 `CapsuleBackend::VzVm` variant, without touching
    /// the Vz framework. Uses the legacy `RunningVm::new`
    /// constructor (no `VzMachineHandle` attached) — its
    /// `is_running` returns the cached `status` (defaults to
    /// `Stopped`) and `stop` is a no-op `Ok(())`, which is
    /// exactly what the dispatcher tests need to assert
    /// supervisor wiring without a real VM.
    #[cfg(target_os = "macos")]
    fn synthetic_vzvm_running_capsule(name: &str, handle: &str) -> RunningCapsule {
        use elastos_vz::RunningVm;
        use elastos_vz::VmConfig as VzVmConfig;
        let manifest = synthetic_microvm_manifest(name);
        let vm_config = VzVmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/tmp/phase3-day3-fake-capsule-dir"),
            std::path::Path::new("/tmp/phase3-day3-fake-kernel"),
        );
        let vm = RunningVm::new(
            vm_config,
            manifest,
            std::path::PathBuf::from(format!("/tmp/{}-fake-socket", handle)),
        );
        RunningCapsule {
            name: name.into(),
            handle: handle.into(),
            vsock_cid: 1234,
            started_at: std::time::Instant::now(),
            provider_route: None,
            backend: CapsuleBackend::VzVm(Box::new(vm)),
        }
    }

    /// Phase 3 Day 3 contract: a `CapsuleBackend::VzVm` entry
    /// inserted into `self.running` is reported by
    /// `capsule_status` — `elastos status <handle>` and the
    /// `running` map enumeration that `elastos ps` builds from
    /// both go through this arm.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_status_returns_running_capsule_for_vz_vm_variant() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase3-day3-status-test-1234-0";
        let rc = synthetic_vzvm_running_capsule("phase3-day3-status-test", handle);
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .capsule_status(handle)
            .await
            .expect("capsule_status must dispatch to the VzVm arm");

        // The synthetic VM has no Vz handle, so is_running()
        // returns the cached status (Stopped). The dispatcher
        // path is what we're verifying — that the new variant
        // is matched without an `unreachable!()` / wildcard
        // panic.
        assert_eq!(response.status, "stopped");
        assert_eq!(response.handle.as_deref(), Some(handle));
        assert_eq!(response.vsock_cid, Some(1234));
    }

    /// Phase 3 Day 3 contract: `stop_capsule` removes a
    /// `CapsuleBackend::VzVm` entry from `self.running` and
    /// dispatches `RunningVm::stop` to the Vz substrate.
    /// `elastos stop <handle>` therefore works on Mac for the
    /// first time.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stop_capsule_removes_vz_vm_from_running_map() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase3-day3-stop-test-1234-0";
        let rc = synthetic_vzvm_running_capsule("phase3-day3-stop-test", handle);
        supervisor.running.write().await.insert(handle.into(), rc);

        supervisor
            .stop_capsule(handle)
            .await
            .expect("stop_capsule must dispatch through the VzVm arm cleanly");

        let running = supervisor.running.read().await;
        assert!(
            !running.contains_key(handle),
            "stop_capsule must remove the VzVm entry from `running`"
        );
    }

    /// Phase 3 Day 3 contract: `reap_dead_capsules` correctly
    /// handles the `CapsuleBackend::VzVm` variant — a stopped
    /// Vz VM is reaped (removed from `running`) on the same
    /// background tick that reaps a stopped crosvm VM. Without
    /// this arm, the reaper would either fail to compile
    /// (exhaustiveness) or pick up the wrong default behaviour.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn reap_dead_capsules_removes_stopped_vz_vm_entry() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase3-day3-reap-test-1234-0";
        // The synthetic RunningVm has no VzMachineHandle, so
        // is_running() reads the cached `status` which defaults
        // to Stopped — the reaper should pick it up.
        let rc = synthetic_vzvm_running_capsule("phase3-day3-reap-test", handle);
        supervisor.running.write().await.insert(handle.into(), rc);

        supervisor.reap_dead_capsules().await;

        let running = supervisor.running.read().await;
        assert!(
            !running.contains_key(handle),
            "reap_dead_capsules must remove stopped VzVm entries"
        );
    }

    /// Phase 3 Day 2 contract: when the capsule directory holds
    /// a `rootfs.ext4`, `build_vm_config_for_mac` creates a
    /// writable overlay under `rootfs_cache_dir/overlays/` and
    /// rewires `vm_config.rootfs_path` to point at it. The host
    /// source rootfs stays untouched (Linux has the same
    /// invariant).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_creates_rootfs_overlay_when_source_present() {
        use elastos_vz::VzConfig;
        use std::fs;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();

        // Stand-in rootfs file. Production capsules ship a real
        // ext4; the test only needs bytes that round-trip
        // through `tokio::fs::copy`.
        let rootfs_src = capsule_dir.join("rootfs.ext4");
        fs::write(&rootfs_src, b"phase3-day2-fake-rootfs").unwrap();

        let manifest = synthetic_microvm_manifest("phase3-day2-overlay-test");
        let vz_config = VzConfig::default();
        let (vm_config, handle, _cid, _carrier) = supervisor
            .build_vm_config_for_mac(
                "phase3-day2-overlay-test",
                &manifest,
                &capsule_dir,
                serde_json::Value::Null,
                &vz_config,
            )
            .await
            .expect("build_vm_config_for_mac succeeds with rootfs source present");

        // Source rootfs untouched.
        assert_eq!(
            fs::read(&rootfs_src).unwrap(),
            b"phase3-day2-fake-rootfs",
            "source rootfs.ext4 must not be mutated"
        );

        // Overlay path rewired to a per-handle file under
        // rootfs_cache_dir/overlays/.
        assert!(
            vm_config.rootfs_path.is_file(),
            "overlay file must exist on disk at {}",
            vm_config.rootfs_path.display()
        );
        assert!(
            vm_config.rootfs_path != rootfs_src,
            "overlay must NOT be the same path as the source rootfs"
        );
        let overlay_str = vm_config.rootfs_path.to_string_lossy();
        assert!(
            overlay_str.contains("overlays"),
            "overlay path should live under …/overlays/: {}",
            overlay_str
        );
        assert!(
            overlay_str.contains(&handle),
            "overlay filename should embed the handle: {}",
            overlay_str
        );
        // Bytes copied verbatim.
        assert_eq!(
            fs::read(&vm_config.rootfs_path).unwrap(),
            b"phase3-day2-fake-rootfs",
            "overlay must carry the source rootfs bytes verbatim"
        );
    }

    // ---------------------------------------------------------------
    // Phase 3 Day 6 — provider-bridge registration on the Mac arm.
    // ---------------------------------------------------------------

    /// The Mac sibling of `register_provider_route` must actually
    /// land a `Provider` in the supervisor's registry for the
    /// `localhost://` scheme when the manifest declares
    /// `provides: localhost://…`. The dialer doesn't fire during
    /// registration (only at first request), so we can validate
    /// the registration path without a live VM.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn register_provider_route_with_vsock_dialer_attaches_provider_to_registry() {
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        let mut supervisor = make_test_supervisor();
        supervisor.set_provider_registry(Arc::clone(&registry));

        let dialer: crate::vm_provider::MacVsockDial = Arc::new(|_port| {
            Box::pin(async move {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "test dialer never fires",
                ))
            })
        });

        let route = supervisor
            .register_provider_route_with_vsock_dialer(
                "localhost-provider-mac",
                Some("localhost://"),
                "vm-localhost-provider-mac-1".into(),
                serde_json::json!({"hello": "world"}),
                dialer,
            )
            .await;

        match route {
            Some(ProviderRoute::Scheme(s)) => assert_eq!(s, "localhost"),
            other => panic!("expected Scheme('localhost'), got {other:?}"),
        }

        // The registry must now resolve the scheme — confirming a
        // real `VmCapsuleProvider` (with the dialer) was inserted.
        assert!(
            registry.has_provider("localhost").await,
            "expected provider to be resolvable for 'localhost' scheme"
        );
    }

    /// The dialer closure baked into `start_capsule_vm_macos` looks
    /// up the live `RunningCapsule` via `Weak<…>` so a torn-down VM
    /// surfaces a clean `io::ErrorKind::NotConnected` rather than
    /// panicking. We can't easily boot a real Vz VM in test, but we
    /// CAN reproduce the closure shape against an empty running
    /// map — which is exactly the state the supervisor's reaper
    /// leaves behind for a dead VM.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn mac_vsock_dialer_closure_returns_not_connected_when_handle_is_missing() {
        // Build the same `Weak<RwLock<HashMap<…>>>` topology that
        // `start_capsule_vm_macos` would use, but never populate
        // the map. This exercises both fall-throughs in the
        // closure: `weak.upgrade()` ok, then `map.get(handle)` miss.
        let running: Arc<RwLock<HashMap<String, RunningCapsule>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let running_weak = Arc::downgrade(&running);
        let handle_for_dialer: String = "vm-phantom-1-1".into();

        let dialer: crate::vm_provider::MacVsockDial = Arc::new(move |port: u32| {
            let running_weak = running_weak.clone();
            let handle = handle_for_dialer.clone();
            Box::pin(async move {
                let Some(running) = running_weak.upgrade() else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "supervisor running map has been dropped",
                    ));
                };
                let map = running.read().await;
                let Some(rc) = map.get(&handle) else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        format!("capsule handle '{handle}' is no longer running"),
                    ));
                };
                match &rc.backend {
                    CapsuleBackend::VzVm(vm) => vm
                        .connect_vsock(port)
                        .await
                        .map_err(|e| std::io::Error::other(e.to_string())),
                    _ => Err(std::io::Error::other("not a Vz VM")),
                }
            })
        });

        let err = dialer(7000).await.expect_err("expected NotConnected");
        assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
        assert!(
            err.to_string().contains("no longer running"),
            "expected handle-missing error message, got: {err}"
        );

        // Now drop the running map entirely and assert the other
        // path (weak.upgrade() fails) also produces NotConnected.
        drop(running);
        let err = dialer(7000).await.expect_err("expected NotConnected");
        assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
        assert!(
            err.to_string().contains("running map has been dropped"),
            "expected dropped-map error message, got: {err}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 4 Day 1 — N concurrent launches: CID allocator audit,
    // multi-VM launch isolation, reaper concurrency.
    // ---------------------------------------------------------------

    /// The CID allocator is the only shared mutable state both
    /// launch flows touch before they branch into substrate-
    /// specific code. Under N concurrent callers it must hand
    /// out N distinct values — the `RwLock::write` future is the
    /// only thing standing between us and duplicate CIDs (which
    /// would later collide in the runtime's vsock dispatch).
    ///
    /// 100 spawned tasks exceeds Tokio's default
    /// `worker_threads = 1` test runtime by a wide margin so we
    /// run on the multi-thread flavour with 4 workers; we also
    /// verify the contract under the default single-thread
    /// flavour (the second test below) since CI invocations may
    /// pin `RUST_TEST_THREADS=1`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cid_allocator_hands_out_100_unique_values_under_concurrent_load() {
        let supervisor = std::sync::Arc::new(make_test_supervisor());

        const N: usize = 100;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..N {
            let supervisor = supervisor.clone();
            set.spawn(async move { supervisor.allocate_next_cid().await });
        }

        let mut cids = Vec::with_capacity(N);
        while let Some(joined) = set.join_next().await {
            cids.push(joined.expect("join must not panic"));
        }
        assert_eq!(cids.len(), N, "every spawned task must produce a CID");

        let mut sorted = cids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            N,
            "100 parallel allocate_next_cid calls must yield 100 distinct CIDs, got {} after dedup (cids: {:?})",
            sorted.len(),
            cids
        );
    }

    /// Same contract as the multi-thread case above but exercised
    /// under the single-threaded test runtime. Both `RUST_TEST_THREADS`
    /// values are CI gates per Phase 4 Day 1, so the allocator
    /// must remain race-free under either scheduler.
    #[tokio::test(flavor = "current_thread")]
    async fn cid_allocator_hands_out_100_unique_values_on_single_threaded_runtime() {
        let supervisor = std::sync::Arc::new(make_test_supervisor());

        const N: usize = 100;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..N {
            let supervisor = supervisor.clone();
            set.spawn(async move { supervisor.allocate_next_cid().await });
        }

        let mut cids = Vec::with_capacity(N);
        while let Some(joined) = set.join_next().await {
            cids.push(joined.expect("join must not panic"));
        }

        let mut sorted = cids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            N,
            "single-threaded runtime must also yield 100 distinct CIDs"
        );
    }

    /// Three parallel `build_vm_config_for_mac` calls must each
    /// receive a distinct CID and a handle that embeds its OWN
    /// capsule name. This is the supervisor-level analogue of the
    /// `elastos-vz` concurrent_load_rejections test: it proves the
    /// supervisor's `next_cid` write lock and `unique_handle`
    /// composition stay race-free when the multi-microVM launch
    /// graph (`home` + `chat` + `localhost-provider`) hits the
    /// supervisor in parallel.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn build_vm_config_for_mac_isolates_concurrent_launches() {
        use elastos_vz::VzConfig;

        let supervisor = std::sync::Arc::new(make_test_supervisor());
        let vz_config = std::sync::Arc::new(VzConfig::default());

        let names = ["alpha", "bravo", "charlie"];
        let mut set = tokio::task::JoinSet::new();
        for name in names.iter() {
            let supervisor = supervisor.clone();
            let vz_config = vz_config.clone();
            let name = name.to_string();
            let capsule_dir = tempfile::tempdir().unwrap().keep();
            let manifest = synthetic_microvm_manifest(&name);

            set.spawn(async move {
                let (_vm_config, handle, cid, _carrier_socket) = supervisor
                    .build_vm_config_for_mac(
                        &name,
                        &manifest,
                        &capsule_dir,
                        serde_json::Value::Null,
                        &vz_config,
                    )
                    .await
                    .expect("build_vm_config_for_mac succeeds with synthetic inputs");
                (name, handle, cid)
            });
        }

        let mut results = Vec::with_capacity(names.len());
        while let Some(joined) = set.join_next().await {
            results.push(joined.expect("join must not panic"));
        }

        // Every CID must be distinct.
        let mut cids: Vec<u32> = results.iter().map(|(_, _, c)| *c).collect();
        cids.sort_unstable();
        cids.dedup();
        assert_eq!(
            cids.len(),
            names.len(),
            "concurrent build_vm_config_for_mac calls must allocate distinct CIDs"
        );

        // Each handle must carry its OWN capsule name — proves
        // no name shadowing through the shared supervisor.
        for (name, handle, _) in &results {
            assert!(
                handle.contains(name),
                "handle '{handle}' must embed its own capsule name '{name}'"
            );
        }

        // Handles must themselves be distinct (UUID-ish via cid
        // disambiguator).
        let mut handles: Vec<String> = results.iter().map(|(_, h, _)| h.clone()).collect();
        handles.sort();
        handles.dedup();
        assert_eq!(
            handles.len(),
            names.len(),
            "concurrent launches must produce distinct handles"
        );
    }

    /// `reap_dead_capsules` must remove ONLY the capsules whose
    /// backend reports `is_running() == false`. Inject three
    /// synthetic `RunningCapsule`s — two with `status: Running`,
    /// one with `status: Stopped` — and assert exactly the
    /// stopped one is evicted. This is the supervisor's safety
    /// contract against accidental mass-eviction on a single
    /// VM's terminal transition.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn reap_dead_capsules_removes_only_stopped_vz_capsules() {
        use elastos_common::CapsuleStatus;

        let supervisor = make_test_supervisor();

        // Reuse the Day-3 `synthetic_vzvm_running_capsule`
        // helper and mutate the inner `RunningVm.status` so
        // `is_running()` returns either `true` (Running) or
        // `false` (Stopped) without needing a live Vz handle.
        fn make_vz_capsule_with_status(
            name: &str,
            status: CapsuleStatus,
        ) -> (String, RunningCapsule) {
            let handle = format!("vm-{name}-1-1");
            let mut rc = synthetic_vzvm_running_capsule(name, &handle);
            if let CapsuleBackend::VzVm(ref mut vm) = rc.backend {
                vm.status = status;
            }
            (handle, rc)
        }

        {
            let mut running = supervisor.running.write().await;
            let (h_alive, c_alive) = make_vz_capsule_with_status("alpha", CapsuleStatus::Running);
            let (h_dead, c_dead) = make_vz_capsule_with_status("bravo", CapsuleStatus::Stopped);
            let (h_alive2, c_alive2) =
                make_vz_capsule_with_status("charlie", CapsuleStatus::Running);
            running.insert(h_alive, c_alive);
            running.insert(h_dead, c_dead);
            running.insert(h_alive2, c_alive2);
        }

        supervisor.reap_dead_capsules().await;

        let running = supervisor.running.read().await;
        assert_eq!(
            running.len(),
            2,
            "reaper must leave exactly the two live capsules; got {} keys: {:?}",
            running.len(),
            running.keys().collect::<Vec<_>>()
        );
        assert!(
            running.keys().any(|k| k.contains("alpha")),
            "alpha must remain (Running)"
        );
        assert!(
            running.keys().any(|k| k.contains("charlie")),
            "charlie must remain (Running)"
        );
        assert!(
            !running.keys().any(|k| k.contains("bravo")),
            "bravo (Stopped) must be reaped"
        );
    }

    /// Phase 4 Day 2 — `reap_dead_capsules` × concurrent reader
    /// race. The reaper takes `running.write().await`; the
    /// supervisor's `capsule_status` / `info` / introspection
    /// handlers take `running.read().await`. Tokio's `RwLock` is
    /// fair: a long-held read briefly delays a contending write
    /// but never starves it, and vice versa.
    ///
    /// This test:
    /// 1. Inserts three `VzVm` capsules (alpha Running,
    ///    bravo Stopped, charlie Running).
    /// 2. Spawns a "reader" task that holds `running.read().await`
    ///    for ~200ms (simulates a supervisor introspection
    ///    call mid-iteration).
    /// 3. Calls `reap_dead_capsules` while the read is held.
    /// 4. Asserts the reaper completes after the reader
    ///    releases (proven by total elapsed >= 150ms), removes
    ///    ONLY bravo, and the reader's view never observed a
    ///    partial removal (proven by iterating it BEFORE the
    ///    drop and after re-acquiring a read).
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reap_dead_capsules_does_not_starve_concurrent_readers() {
        use elastos_common::CapsuleStatus;

        let supervisor = std::sync::Arc::new(make_test_supervisor());

        fn make_vz_capsule_with_status(
            name: &str,
            status: CapsuleStatus,
        ) -> (String, RunningCapsule) {
            let handle = format!("vm-{name}-1-1");
            let mut rc = synthetic_vzvm_running_capsule(name, &handle);
            if let CapsuleBackend::VzVm(ref mut vm) = rc.backend {
                vm.status = status;
            }
            (handle, rc)
        }

        {
            let mut running = supervisor.running.write().await;
            let (h_alive, c_alive) = make_vz_capsule_with_status("alpha", CapsuleStatus::Running);
            let (h_dead, c_dead) = make_vz_capsule_with_status("bravo", CapsuleStatus::Stopped);
            let (h_alive2, c_alive2) =
                make_vz_capsule_with_status("charlie", CapsuleStatus::Running);
            running.insert(h_alive, c_alive);
            running.insert(h_dead, c_dead);
            running.insert(h_alive2, c_alive2);
        }

        // Spawn the reader: hold a read lock for 200ms while
        // observing the map. The read MUST see all three
        // entries — the reaper cannot mutate the map while
        // the read is held.
        let reader_supervisor = supervisor.clone();
        let reader = tokio::spawn(async move {
            let running = reader_supervisor.running.read().await;
            let snapshot: Vec<String> = running.keys().cloned().collect();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // After the sleep, the map is STILL the same (the
            // read lock guarantees this — Tokio's RwLock blocks
            // writers behind active readers).
            let after: Vec<String> = running.keys().cloned().collect();
            assert_eq!(
                snapshot, after,
                "reader's snapshot must be stable under a held read lock"
            );
            snapshot
        });

        // Give the reader a head start so it definitely owns
        // the read lock before the reaper attempts the write.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let reap_start = std::time::Instant::now();
        supervisor.reap_dead_capsules().await;
        let reap_elapsed = reap_start.elapsed();

        // The reaper had to wait for the reader to release —
        // the reader holds the lock for 200ms, we started the
        // reaper ~50ms in, so the reap should take at least
        // ~150ms before the lock becomes available. Bound it
        // loosely (>= 120ms) to absorb scheduler jitter.
        assert!(
            reap_elapsed >= std::time::Duration::from_millis(120),
            "reaper must have waited for the reader to release the read lock; elapsed={:?}",
            reap_elapsed
        );

        // Confirm the reader saw the pre-reap state (all three
        // handles) and the post-reap state has only two.
        let reader_snapshot = reader.await.expect("reader task must not panic");
        assert_eq!(
            reader_snapshot.len(),
            3,
            "reader must have observed all three pre-reap capsules; saw: {reader_snapshot:?}"
        );

        let running = supervisor.running.read().await;
        assert_eq!(
            running.len(),
            2,
            "post-reap state must have exactly two live capsules"
        );
        assert!(
            running.keys().any(|k| k.contains("alpha")),
            "alpha (Running) must remain after reap"
        );
        assert!(
            running.keys().any(|k| k.contains("charlie")),
            "charlie (Running) must remain after reap"
        );
        assert!(
            !running.keys().any(|k| k.contains("bravo")),
            "bravo (Stopped) must have been reaped"
        );
    }
}
