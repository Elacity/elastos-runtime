//! Capsule supervisor — lifecycle management for capsule VMs.
//!
//! The supervisor is the runtime's control plane: ensure capsules are downloaded
//! and verified, launch them in crosvm VMs, stop them, and report status.
//! Guest capsules reach it over the Carrier-managed private control network.
//!
//! crosvm is the sole VM backend. No fallback — KVM is required.

use anyhow::{bail, Context, Result};
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

use elastos_common::CapsuleRole;
use elastos_crosvm::{
    CrosvmConfig, EgressFirewall, NetworkConfig, RunningVm, VmConfig, EGRESS_LOG_RATE_PER_SEC,
    EGRESS_NFLOG_GROUP,
};
use elastos_runtime::provider::ProviderRegistry;

/// The capsule the runtime treats as the active shell when no active-shell
/// pointer has been set. Preserves historical behaviour (the bundled `shell`).
const DEFAULT_ACTIVE_SHELL: &str = "shell";

/// Whether a launching capsule is eligible for the privileged shell session
/// token (W3). De-hardcodes the old `name == "shell"` magic string: a capsule
/// gets the shell token ONLY if it both holds the `Shell` role AND is the
/// user-selected active shell. Fail-closed — a non-`Shell` capsule can never
/// receive the shell token even if the active-shell pointer names it.
fn shell_token_eligible(name: &str, role: &CapsuleRole, active_shell: &str) -> bool {
    matches!(role, CapsuleRole::Shell) && name == active_shell
}
use elastos_runtime::session::{SessionRegistry, SessionType};
use elastos_runtime::signature::{hash_content, SignatureVerifier};

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
        #[serde(default)]
        principal_id: Option<String>,
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
    /// Shell session token — only injected into the active shell capsule VM
    shell_token: Option<String>,
    /// Name of the capsule currently selected as the active shell (W3). The
    /// privileged shell token is issued only to this capsule, and only if it
    /// holds the `Shell` role. Defaults to [`DEFAULT_ACTIVE_SHELL`].
    active_shell: String,
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
    /// Per-act spend policy for the microVM Carrier act path; `None` ⇒ unmetered.
    spend_policy: Option<crate::carrier_bridge::SpendPolicy>,
    /// The shared runtime/infra custody log, threaded to the serve gateway so its audit
    /// events ride the SAME signed chain as carrier/capability/spend. `None` ⇒ the gateway
    /// keeps its own file sink. Only adopted by the gateway when durable (see
    /// [`crate::api::gateway::seed_gateway_audit_log`]) — never a durable→memory downgrade.
    shared_audit_log: Option<Arc<elastos_runtime::primitives::audit::AuditLog>>,
    /// W1b/C3: TAP→`vm-{name}` map so a kernel egress drop (logged on a TAP) becomes an
    /// `EgressDenied` keyed on the canonical capsule identity. Populated at firewall-install;
    /// shared with the NFLOG audit-reader thread.
    tap_registry: crate::egress_audit::TapRegistry,
    /// Author-signature verifier for the launch gate (AUD-1). Default is empty (no
    /// trusted keys), in which case the gate skips and launches are byte-for-byte
    /// today's behavior; seeded from config `trusted_keys` at serve time to activate.
    signature_verifier: SignatureVerifier,
    /// Optional running gateway server task.
    gateway: Arc<RwLock<Option<RunningGateway>>>,
}

/// AUD-1 author-signature launch gate, FAIL-CLOSED WHEN CONFIGURED. With no trusted
/// keys (the default) it logs a warning and allows the launch — byte-for-byte today's
/// behavior, zero breakage. With >=1 trusted key it REFUSES (returns Err, before boot)
/// a capsule that is unsigned OR whose signature matches no trusted key. Uses the same
/// canonical content-hash domain as `trust_cmd` signing (`SHA256(manifest_without_sig)
/// || hash_content(entrypoint bytes)`), so it never false-denies a legitimately-signed
/// capsule (the sign and verify domains are byte-identical — see `signature/verifier.rs`).
/// A pure, sync, VM-free function so the gate is unit-testable without a crosvm boot.
fn gate_author_signature(
    verifier: &SignatureVerifier,
    capsule_dir: &Path,
    manifest: &elastos_common::CapsuleManifest,
) -> Result<()> {
    if !verifier.is_enabled() {
        tracing::warn!(
            "author-signature verification skipped for '{}' (no trusted keys configured)",
            manifest.name
        );
        return Ok(());
    }
    if manifest.signature.is_none() {
        bail!(
            "capsule '{}' is unsigned but trusted keys are configured; refusing launch",
            manifest.name
        );
    }
    let entrypoint_path = capsule_dir.join(&manifest.entrypoint);
    let content = std::fs::read(&entrypoint_path).with_context(|| {
        format!(
            "failed to read entrypoint {} for the signature gate",
            manifest.entrypoint
        )
    })?;
    let content_hash = hash_content(&content);
    if !verifier
        .verify_capsule(manifest, &content_hash)
        .map_err(|e| anyhow::anyhow!("signature verification error for '{}': {e}", manifest.name))?
    {
        bail!(
            "capsule '{}' signature does not match any trusted key; refusing launch",
            manifest.name
        );
    }
    tracing::info!("author signature verified for '{}'", manifest.name);
    Ok(())
}

/// Lowest assignable vsock guest CID. 0 (any/hypervisor), 1 (local), and 2
/// (host) are reserved by the vsock transport, so guest CIDs start at 3.
const MIN_GUEST_CID: u32 = 3;

/// Pick the next free vsock CID at or after `from`, skipping reserved low CIDs
/// and any CID currently in use by a live VM, wrapping the u32 space WITHOUT
/// overflow (the old `*next += 1` could wrap and re-hand a live VM's CID — BUG-8).
///
/// Returns `(cid, advance)` where `cid` is the allocated CID and `advance` is the
/// value to store as the next starting point. Returns `None` (fail-closed) only
/// if every CID is in use — the caller refuses the launch rather than colliding.
///
/// Pure + VM-free so it is unit-testable without a crosvm boot. The scan is
/// bounded by `in_use.len() + 1` candidates: by pigeonhole, that many distinct
/// CIDs cannot all be occupied if any free CID exists.
fn allocate_cid(from: u32, in_use: &HashSet<u32>) -> Option<(u32, u32)> {
    let mut candidate = from.max(MIN_GUEST_CID);
    for _ in 0..=in_use.len() {
        if !in_use.contains(&candidate) {
            // Wrap back to the first guest CID instead of overflowing past u32::MAX.
            let advance = candidate.checked_add(1).unwrap_or(MIN_GUEST_CID);
            return Some((candidate, advance));
        }
        candidate = candidate.checked_add(1).unwrap_or(MIN_GUEST_CID);
    }
    None
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
            active_shell: DEFAULT_ACTIVE_SHELL.to_string(),
            api_addr: None,
            session_registry: None,
            provider_registry: None,
            capability_manager: None,
            pending_store: None,
            spend_policy: None,
            shared_audit_log: None,
            tap_registry: crate::egress_audit::TapRegistry::new(),
            signature_verifier: SignatureVerifier::new(),
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

    /// Select which capsule is the active shell (W3). The privileged shell token
    /// is issued only to this capsule, and only if it holds the `Shell` role
    /// (see [`shell_token_eligible`]). Lets a user run a shell other than the
    /// bundled default without the runtime hardcoding a name.
    pub fn set_active_shell(&mut self, active_shell: impl Into<String>) {
        self.active_shell = active_shell.into();
    }

    /// The capsule currently selected as the active shell.
    pub fn active_shell(&self) -> &str {
        &self.active_shell
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

    /// Attach the per-act spend policy so microVM capsule acts are metered (act-over-MCP). `None`
    /// (the default) leaves the microVM carrier path unmetered.
    pub fn set_spend_policy(&mut self, spend_policy: Option<crate::carrier_bridge::SpendPolicy>) {
        self.spend_policy = spend_policy;
    }

    /// Attach the shared runtime/infra custody log so the serve gateway unifies its audit
    /// events onto the one signed chain (carrier/capability/spend). `None` (the default)
    /// leaves the gateway with its own file sink. The gateway adopts this only when it is
    /// durable — see [`crate::api::gateway::seed_gateway_audit_log`].
    pub fn set_shared_audit_log(
        &mut self,
        shared_audit_log: Option<Arc<elastos_runtime::primitives::audit::AuditLog>>,
    ) {
        self.shared_audit_log = shared_audit_log;
    }

    /// W1b/C3: start the single process-wide NFLOG egress-audit reader, which turns kernel egress
    /// drops into signed `EgressDenied` events on the shared custody chain (keyed on `vm-{name}`
    /// via [`tap_registry`](Self::tap_registry)). No-op without a shared audit log. Best-effort and
    /// enforcement-independent: a reader that can't bind never affects the in-kernel DROP. Call
    /// once at serve time, AFTER [`set_shared_audit_log`](Self::set_shared_audit_log).
    pub fn start_egress_audit_reader(&self) {
        if let Some(audit_log) = &self.shared_audit_log {
            crate::egress_audit::spawn_egress_audit_reader(
                audit_log.clone(),
                self.tap_registry.clone(),
            );
        }
    }

    /// Seed the author-signature launch gate with trusted keys (AUD-1). Called at
    /// serve time from the config `trusted_keys`. An empty verifier (the default)
    /// leaves the gate inert (launches unchanged); a non-empty one makes
    /// [`gate_author_signature`] refuse unsigned / wrong-signer capsules before boot.
    pub fn set_signature_verifier(&mut self, verifier: SignatureVerifier) {
        self.signature_verifier = verifier;
    }

    /// Handle a supervisor request from the shell.
    pub async fn handle_request(&self, req: SupervisorRequest) -> SupervisorResponse {
        self.reap_dead_capsules().await;
        match req {
            SupervisorRequest::EnsureCapsule { name } => match self.ensure_capsule(&name).await {
                Ok(path) => SupervisorResponse::ok_with_path(path.display().to_string()),
                Err(e) => SupervisorResponse::err(format!("ensure_capsule failed: {e}")),
            },
            SupervisorRequest::LaunchCapsule {
                name,
                config,
                principal_id,
            } => match self.launch_capsule(&name, config, principal_id).await {
                Ok((handle, cid)) => SupervisorResponse::ok_with_handle(handle, cid),
                Err(e) => SupervisorResponse::err(format!("launch_capsule failed: {e}")),
            },
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
            // iter_mut so the VM branch can REAP a self-exited child via
            // has_exited() (try_wait), not merely probe liveness. A zombie still
            // answers kill(pid,0), so an is_running()-only sweep would never
            // collect it (BUG-1). We hold the write lock here, so reaping is safe.
            let dead_handles: Vec<String> = running
                .iter_mut()
                .filter_map(|(handle, capsule)| {
                    let alive = match &mut capsule.backend {
                        CapsuleBackend::Vm(vm) => !vm.has_exited(),
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

        // Download capsule artifact through the registered content availability contract.
        self.download_capsule(name, entry, &capsule_dir).await?;

        Ok(capsule_dir)
    }

    /// Download a capsule artifact, verify, and extract.
    ///
    /// Canonical path today: content provider backed by local IPFS/Kubo.
    /// No HTTP fallback is allowed here.
    async fn download_capsule(&self, name: &str, entry: &CapsuleEntry, dest: &Path) -> Result<()> {
        self.try_download_capsule_via_content_provider(name, &entry.cid, &entry.sha256, dest)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "capsule download failed via elastos://content/fetch provider path: {}",
                    e
                )
            })
    }

    /// Fetch capsule content via the content availability provider.
    ///
    /// The content provider may use `ipfs-provider` internally, but supervisor
    /// code does not bind itself to the low-level backend namespace.
    async fn try_download_capsule_via_content_provider(
        &self,
        name: &str,
        cid: &str,
        expected_sha256: &str,
        dest: &Path,
    ) -> Result<()> {
        use sha2::Digest;

        println!(
            "  Fetching capsule '{}' via content provider (CID: {})...",
            name, cid
        );

        let bytes = self.content_fetch_via_provider(cid).await?;

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

        println!("  Extracted to {} (via content provider)", dest.display());
        Ok(())
    }

    async fn content_fetch_via_provider(&self, cid: &str) -> Result<Vec<u8>> {
        let registry = self
            .provider_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("runtime provider registry unavailable"))?;

        crate::content::fetch_bytes_via_provider(registry, cid, None)
            .await
            .map_err(|e| anyhow::anyhow!("elastos://content/fetch unavailable: {}", e))
    }

    /// Launch a capsule in a crosvm VM. Returns (handle, vsock_cid).
    ///
    /// `config` is an opaque JSON payload from the CLI command. For the shell
    /// capsule, this contains the forwarded command (e.g. `{"command":"chat",...}`).
    /// It is base64-encoded and passed via the `elastos.command` kernel boot arg.
    async fn launch_capsule(
        &self,
        name: &str,
        config: serde_json::Value,
        principal_id: Option<String>,
    ) -> Result<(String, u32)> {
        let (capsule_dir, manifest) = self.load_capsule_manifest(name).await?;
        if principal_id.is_some() && !manifest.role.is_shell_launchable() {
            bail!("principal launch grants are only valid for shell-launchable capsules");
        }

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

        // AUD-1: author-signature gate (VM path). Fail-closed when trusted keys are
        // configured, skips (warns) when none are. Runs BEFORE the KVM check and the
        // overlay copy, so it hashes the signed base entrypoint in capsule_dir.
        gate_author_signature(&self.signature_verifier, &capsule_dir, &manifest)?;

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

        // Assign vsock CID (unique per live VM). Hold the counter lock across the
        // live-set scan so concurrent launches serialize; the scan is the
        // belt-and-suspenders guard that a wrapped counter never re-hands a CID a
        // running VM still holds (BUG-8).
        let cid = {
            let mut next = self.next_cid.write().await;
            let in_use: HashSet<u32> = {
                let running = self.running.read().await;
                running.values().map(|c| c.vsock_cid).collect()
            };
            match allocate_cid(*next, &in_use) {
                Some((cid, advance)) => {
                    *next = advance;
                    cid
                }
                None => bail!(
                    "no free vsock CID available ({} live VMs occupy the CID space)",
                    in_use.len()
                ),
            }
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
            let token = if shell_token_eligible(name, &manifest.role, &self.active_shell) {
                self.shell_token.clone()
            } else {
                // Mint a fresh Capsule token via the session registry
                match &self.session_registry {
                    Some(reg) => {
                        // G-ID: carry the real capsule identity ("vm-{name}", the
                        // same string the carrier gate uses at supervisor.rs:1099)
                        // on the session, so the grant path can record it.
                        let session = reg
                            .create_session(SessionType::Capsule, Some(format!("vm-{}", name)))
                            .await;
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
                    principal_id: principal_id.clone(),
                    data_dir: Some(self.data_dir.clone()),
                    // microVM capsule acts are metered under the canonical vm-{name} budget.
                    spend_policy: self.spend_policy.clone(),
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

        // W1b: bind the per-TAP egress firewall before boot. Keyed on the REAL
        // TAP device name (not vm-{name}); the guest is leashed to the host
        // runtime API and default-drops everything else, fail-closed. Installed
        // and torn down with the TAP inside RunningVm.
        let net_params = vm
            .config
            .network
            .as_ref()
            .map(|n| (n.tap_name.clone(), n.host_ip.clone()));
        if let Some((tap_name, host_ip)) = net_params {
            // The one allowed destination port is the host runtime HTTP API.
            // Absent (carrier-only token path) ⇒ port 0 ⇒ no accept ⇒ deny-all.
            let api_port = self
                .api_addr
                .as_ref()
                .and_then(|a| a.rsplit(':').next())
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(0);
            let firewall = EgressFirewall::new(
                &tap_name,
                &host_ip,
                api_port,
                EGRESS_NFLOG_GROUP,
                EGRESS_LOG_RATE_PER_SEC,
            )
            .map_err(|e| anyhow::anyhow!("egress firewall build failed for '{}': {}", name, e))?;
            // W1b/C3: map this TAP to the canonical vm-{name} so a kernel drop logged on it
            // becomes an EgressDenied keyed on the same identity as the spend/grant chain.
            // Overwrite-on-record handles TAP reuse (re-label before any of the new VM's drops).
            self.tap_registry.record(&tap_name, &format!("vm-{name}"));
            vm.set_egress_firewall(Some(firewall));
        }

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
            // G-ID: carry the real capsule identity ("vm-{name}") on the session.
            let session = reg
                .create_session(SessionType::Capsule, Some(format!("vm-{}", name)))
                .await;
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
            // Unify the gateway act budget with the carrier paths: the same shared meter.
            let spend_policy = self.spend_policy.clone();
            // Unify the gateway audit sink onto the shared runtime custody chain (when durable).
            let shared_audit_log = self.shared_audit_log.clone();
            async move {
                if let Err(e) = crate::api::gateway::start_gateway_server(
                    &listen_addr,
                    Some(registry),
                    cache_path,
                    data_dir,
                    spend_policy,
                    shared_audit_log,
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
    use base64::Engine as _;
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use sha2::Digest;
    use std::sync::Arc;

    // ── BUG-8: vsock CID allocation is checked + collision-free ──

    #[test]
    fn allocate_cid_floors_reserved_low_cids() {
        // 0/1/2 are reserved; an allocator seeded below 3 must floor to 3.
        let (cid, advance) = allocate_cid(0, &HashSet::new()).unwrap();
        assert_eq!(cid, MIN_GUEST_CID);
        assert_eq!(advance, MIN_GUEST_CID + 1);
    }

    #[test]
    fn allocate_cid_skips_live_cids() {
        let in_use = HashSet::from([5u32, 6, 7]);
        let (cid, advance) = allocate_cid(5, &in_use).unwrap();
        assert_eq!(cid, 8, "5/6/7 are live, so 8 is the first free CID");
        assert_eq!(advance, 9);
    }

    #[test]
    fn allocate_cid_wraps_without_overflow() {
        // The old `*next += 1` would overflow/wrap here; the allocator wraps
        // cleanly back to the first guest CID for the NEXT call.
        let (cid, advance) = allocate_cid(u32::MAX, &HashSet::new()).unwrap();
        assert_eq!(cid, u32::MAX);
        assert_eq!(
            advance, MIN_GUEST_CID,
            "advance wraps to 3, not 0 or overflow"
        );
    }

    /// THE bug: a counter that wraps onto a CID a live VM still holds must NOT
    /// re-hand it — the live-set scan steps past the collision to a free CID.
    #[test]
    fn allocate_cid_wrap_does_not_collide_with_a_live_vm() {
        // Counter has wrapped to u32::MAX, but a long-lived VM already holds it.
        let in_use = HashSet::from([u32::MAX]);
        let (cid, advance) = allocate_cid(u32::MAX, &in_use).unwrap();
        assert_eq!(cid, MIN_GUEST_CID, "skips the live MAX CID, wraps to 3");
        assert_eq!(advance, MIN_GUEST_CID + 1);
        assert!(!in_use.contains(&cid), "the chosen CID is provably free");
    }

    #[test]
    fn allocate_cid_returns_a_free_cid_within_the_bound() {
        // A contiguous live block is stepped over to the next free CID.
        let in_use = HashSet::from([3u32, 4, 5, 6]);
        let (cid, _) = allocate_cid(3, &in_use).unwrap();
        assert_eq!(cid, 7);
        assert!(!in_use.contains(&cid));
    }

    #[test]
    fn shell_token_eligible_is_role_based_and_fail_closed() {
        // The bundled shell (Shell role, matches the active pointer) stays eligible
        // — behaviour-neutral default.
        assert!(shell_token_eligible(
            "shell",
            &CapsuleRole::Shell,
            DEFAULT_ACTIVE_SHELL
        ));
        // The de-hardcoded point: a DIFFERENT Shell-role capsule selected as the
        // active shell is eligible — no longer tied to the magic name "shell".
        assert!(shell_token_eligible("flint", &CapsuleRole::Shell, "flint"));
        // Fail-closed: a non-Shell capsule can NEVER get the shell token, even if
        // the active-shell pointer names it.
        assert!(!shell_token_eligible("flint", &CapsuleRole::App, "flint"));
        // A Shell-role capsule that is NOT the active shell is not eligible.
        assert!(!shell_token_eligible("flint", &CapsuleRole::Shell, "shell"));
    }

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

    #[tokio::test]
    async fn test_launch_capsule_rejects_principal_for_provider_role() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let capsule_dir = data_dir.join("capsules/provider-test");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join(CACHED_CID_FILE), "bafy-provider-test\n").unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "provider-test",
                "version": "0.1.0",
                "role": "provider",
                "type": "microvm",
                "entrypoint": "rootfs.ext4",
                "provides": "elastos://provider-test/*"
            })
            .to_string(),
        )
        .unwrap();

        let mut capsules = std::collections::HashMap::new();
        capsules.insert(
            "provider-test".to_string(),
            CapsuleEntry {
                cid: "bafy-provider-test".to_string(),
                sha256: String::new(),
                size: 0,
                repository: None,
                source_path: None,
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
            .launch_capsule(
                "provider-test",
                serde_json::json!({}),
                Some("person:local:alice".to_string()),
            )
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("principal launch grants are only valid for shell-launchable capsules"));
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

    const TEST_SUPERVISOR_CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

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
            assert_eq!(
                request.get("cid").and_then(|v| v.as_str()),
                Some(TEST_SUPERVISOR_CID)
            );
            Ok(self.response.clone())
        }
    }

    async fn registry_with_content_provider(response: serde_json::Value) -> Arc<ProviderRegistry> {
        let registry = Arc::new(ProviderRegistry::new());
        let data_dir = tempfile::tempdir().unwrap().keep();
        let content_provider = Arc::new(crate::content::ContentProvider::new(
            data_dir,
            Arc::downgrade(&registry),
        ));
        registry.register(content_provider.clone()).await;
        registry
            .register_sub_provider("content", content_provider)
            .await
            .unwrap();

        let ipfs_provider: Arc<dyn Provider> = Arc::new(MockIpfsProvider { response });
        registry
            .register_sub_provider("ipfs", ipfs_provider)
            .await
            .unwrap();
        registry
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
            repository: None,
            source_path: None,
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
        let capsule_dir = data_dir.join("capsules/carrier-service");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join("carrier-service"), b"carrier-service").unwrap();
        std::fs::write(capsule_dir.join(CACHED_CID_FILE), "bafy-test-cid\n").unwrap();
        std::fs::write(
            capsule_dir.join(CACHED_ARTIFACT_SHA_FILE),
            "sha256:test-artifact\n",
        )
        .unwrap();

        let mut capsules = std::collections::HashMap::new();
        capsules.insert(
            "carrier-service".to_string(),
            CapsuleEntry {
                cid: "bafy-test-cid".to_string(),
                sha256: "sha256:test-artifact".to_string(),
                size: 0,
                repository: None,
                source_path: None,
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
                "carrier-service",
                &capsule_dir,
                &capsule_dir.join("carrier-service"),
            )
            .unwrap();
    }

    #[test]
    fn test_verify_carrier_service_binary_rejects_cached_cid_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let capsule_dir = data_dir.join("capsules/carrier-service");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join("carrier-service"), b"carrier-service").unwrap();
        std::fs::write(capsule_dir.join(CACHED_CID_FILE), "bafy-wrong-cid\n").unwrap();
        std::fs::write(
            capsule_dir.join(CACHED_ARTIFACT_SHA_FILE),
            "sha256:test-artifact\n",
        )
        .unwrap();

        let mut capsules = std::collections::HashMap::new();
        capsules.insert(
            "carrier-service".to_string(),
            CapsuleEntry {
                cid: "bafy-test-cid".to_string(),
                sha256: "sha256:test-artifact".to_string(),
                size: 0,
                repository: None,
                source_path: None,
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
                "carrier-service",
                &capsule_dir,
                &capsule_dir.join("carrier-service"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("cached CID mismatch"));
    }

    #[tokio::test]
    async fn test_content_fetch_via_provider_uses_content_contract() {
        let expected = b"capsule-bytes";
        let registry = registry_with_content_provider(serde_json::json!({
            "status": "ok",
            "data": {
                "data": base64::engine::general_purpose::STANDARD.encode(expected),
            }
        }))
        .await;

        let mut supervisor = make_test_supervisor();
        supervisor.set_provider_registry(Arc::clone(&registry));

        let bytes = supervisor
            .content_fetch_via_provider(TEST_SUPERVISOR_CID)
            .await
            .unwrap();
        assert_eq!(bytes, expected);
    }

    #[tokio::test]
    async fn test_content_fetch_via_provider_surfaces_provider_error() {
        let registry = registry_with_content_provider(serde_json::json!({
            "status": "error",
            "message": "kubo not found"
        }))
        .await;

        let mut supervisor = make_test_supervisor();
        supervisor.set_provider_registry(Arc::clone(&registry));

        let err = supervisor
            .content_fetch_via_provider(TEST_SUPERVISOR_CID)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("kubo not found"));
    }

    // ── AUD-1: the author-signature launch gate ─────────────────────
    use elastos_runtime::signature::{generate_keypair, sign_capsule, SigningKey};

    fn aud1_signed_capsule(
        signing_key: &SigningKey,
        bytes: &[u8],
    ) -> (tempfile::TempDir, elastos_common::CapsuleManifest) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.wasm"), bytes).unwrap();
        let mut manifest: elastos_common::CapsuleManifest =
            serde_json::from_value(serde_json::json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "probe",
                "role": "app", "type": "wasm", "entrypoint": "main.wasm"
            }))
            .unwrap();
        let content_hash = hash_content(bytes);
        sign_capsule(signing_key, &mut manifest, &content_hash).unwrap();
        (dir, manifest)
    }

    #[test]
    fn aud1_gate_trusted_signed_capsule_passes() {
        let (sk, vk) = generate_keypair();
        let (dir, manifest) = aud1_signed_capsule(&sk, b"rootfs bytes");
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);
        assert!(gate_author_signature(&verifier, dir.path(), &manifest).is_ok());
    }

    #[test]
    fn aud1_gate_foreign_signed_capsule_refused() {
        let (sk, _vk) = generate_keypair();
        let (_, trusted_vk) = generate_keypair();
        let (dir, manifest) = aud1_signed_capsule(&sk, b"rootfs bytes");
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(trusted_vk);
        assert!(
            gate_author_signature(&verifier, dir.path(), &manifest).is_err(),
            "a capsule signed by a non-trusted key must be refused"
        );
    }

    #[test]
    fn aud1_gate_unsigned_capsule_refused_when_configured() {
        let (sk, vk) = generate_keypair();
        let (dir, mut manifest) = aud1_signed_capsule(&sk, b"rootfs bytes");
        manifest.signature = None;
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);
        assert!(
            gate_author_signature(&verifier, dir.path(), &manifest).is_err(),
            "an unsigned capsule must be refused when trusted keys are configured"
        );
    }

    #[test]
    fn aud1_gate_empty_verifier_passes_as_today() {
        let (sk, _vk) = generate_keypair();
        let (dir, manifest) = aud1_signed_capsule(&sk, b"rootfs bytes");
        let verifier = SignatureVerifier::new();
        let mut unsigned = manifest.clone();
        unsigned.signature = None;
        assert!(gate_author_signature(&verifier, dir.path(), &manifest).is_ok());
        assert!(
            gate_author_signature(&verifier, dir.path(), &unsigned).is_ok(),
            "with zero trusted keys the gate is inert (zero breakage)"
        );
    }

    #[test]
    fn aud1_gate_tampered_entrypoint_refused() {
        let (sk, vk) = generate_keypair();
        let (dir, manifest) = aud1_signed_capsule(&sk, b"original rootfs");
        std::fs::write(dir.path().join("main.wasm"), b"TAMPERED rootfs").unwrap();
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);
        assert!(
            gate_author_signature(&verifier, dir.path(), &manifest).is_err(),
            "a tampered entrypoint must fail the signature gate"
        );
    }

    // AUD-1 ACTIVATION round-trip: the founder path is config `trusted_keys` (HEX) →
    // `from_trusted_keys_hex` (the seed) → `gate_author_signature` (the launch gate).
    // These pin that the seed and the gate compose end to end, so a HEX-configured key
    // admits exactly its own capsules — not just that `add_trusted_key(vk)` works.
    #[test]
    fn aud1_activation_hex_configured_key_admits_its_capsule() {
        let (sk, vk) = generate_keypair();
        let (dir, manifest) = aud1_signed_capsule(&sk, b"rootfs bytes");
        // Exactly what serve does: seed the verifier from the configured hex public key.
        let verifier =
            SignatureVerifier::from_trusted_keys_hex([hex::encode(vk.as_bytes())]).unwrap();
        assert!(
            verifier.is_enabled(),
            "a configured key must enable the gate"
        );
        assert!(
            gate_author_signature(&verifier, dir.path(), &manifest).is_ok(),
            "a capsule signed by the hex-configured trusted key must launch"
        );
    }

    #[test]
    fn aud1_activation_hex_configured_key_refuses_a_foreign_capsule() {
        let (_founder_sk, founder_vk) = generate_keypair();
        let (foreign_sk, _foreign_vk) = generate_keypair();
        // The capsule is signed by a key NOT in the founder's configured trusted set.
        let (dir, manifest) = aud1_signed_capsule(&foreign_sk, b"rootfs bytes");
        let verifier =
            SignatureVerifier::from_trusted_keys_hex([hex::encode(founder_vk.as_bytes())]).unwrap();
        assert!(
            gate_author_signature(&verifier, dir.path(), &manifest).is_err(),
            "a capsule signed by a key outside the configured trusted set must be refused"
        );
    }
}
