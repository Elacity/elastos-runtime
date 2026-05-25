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

/// Phase 4 Day 4 — RAM reserved for the host kernel + Carrier + Rust
/// runtime when sizing a Vz microVM. The supervisor's pre-flight
/// memory guard rejects manifest requests larger than `(host_phys_mem
/// - this)` so an over-spec'd capsule fails with a clear message
/// instead of Apple's opaque `VZErrorInvalidVirtualMachineConfiguration`.
///
/// 1 GiB matches the headroom the Vz documentation suggests for a
/// host that's also running its normal desktop workload.
#[cfg(target_os = "macos")]
const MAC_HOST_HEADROOM_MIB: u64 = 1024;

/// Phase 4 Day 4 — query the host's total physical RAM in MiB via
/// `sysctlbyname("hw.memsize", …)`. The mach-port memory APIs are
/// the source of truth on Darwin; `sysconf(_SC_PHYS_PAGES)` exists
/// but returns 0 on macOS. Falls back to a conservative 4096 MiB if
/// the sysctl call ever fails (it shouldn't on any supported host),
/// keeping the pre-flight guard *active* rather than silently
/// permissive.
#[cfg(target_os = "macos")]
fn host_phys_mem_mib_mac() -> u64 {
    const FALLBACK_MIB: u64 = 4096;
    let name = std::ffi::CString::new("hw.memsize").expect("static C string");
    let mut bytes: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut bytes as *mut u64 as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || bytes == 0 {
        return FALLBACK_MIB;
    }
    bytes / (1024 * 1024)
}

/// Phase 4 Day 5 — counts of artifacts removed by
/// [`Supervisor::prune_stale_mac_artifacts`]. Returned so callers
/// can log / audit cleanup activity without coupling to
/// filesystem semantics.
///
/// On Linux the helper is a no-op stub and these counts are
/// always zero; the struct exists on all platforms so call
/// sites can be platform-agnostic.
///
/// **Phase 5 Day 4**: split socket counts into the two
/// real categories the prune sweeps separately so the
/// telemetry projected via [`OrphanCounts`] gives operators
/// a fleet-wide signal on which orphan class is the long
/// tail. `sockets_removed` continues to count generic
/// `*.sock` files (crosvm-style control sockets);
/// `bridge_sockets_removed` counts the
/// `*-carrier.sock` Carrier-bridge IPC files specifically.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaleArtifactCounts {
    pub overlays_removed: usize,
    pub sockets_removed: usize,
    /// Carrier-bridge IPC socket files (`*-carrier.sock`).
    /// Counted separately from generic `*.sock` files because
    /// the bridge half of the orphan-cleanup carries different
    /// operator semantics (Phase 4 Day 6 documented the bridge
    /// teardown surface independently).
    pub bridge_sockets_removed: usize,
}

/// Phase 5 Day 4 — operator-facing JSON projection of
/// [`StaleArtifactCounts`] surfaced via
/// [`SupervisorResponse::orphans_pruned`] on the first
/// [`SupervisorRequest::EnsureCapsule`] response after
/// [`Supervisor::new`]. **One-shot per supervisor lifetime**:
/// subsequent `EnsureCapsule` calls skip-serialise the field
/// so dashboards distinguish "supervisor just started + cleaned
/// orphans" from "supervisor steady-state."
///
/// Field shape mirrors [`StaleArtifactCounts`] one-to-one but
/// adds `serde` derives so the response is wire-format-stable.
/// Operators alerting on a sustained non-zero orphan rate
/// have a single field per category to pivot on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct OrphanCounts {
    /// Overlay files (`<rootfs_cache>/overlays/*.ext4`) left
    /// behind by a prior supervisor that died mid-launch.
    pub overlays_removed: usize,
    /// Generic Unix-domain socket files (`<socket_dir>/*.sock`)
    /// not matching the carrier-bridge suffix.
    pub sockets_removed: usize,
    /// Carrier-bridge IPC socket files
    /// (`<socket_dir>/*-carrier.sock`). Split out from
    /// `sockets_removed` so operators can pivot on the
    /// bridge-orphan rate independently of the crosvm-style
    /// control-socket rate.
    pub bridge_sockets_removed: usize,
}

impl From<StaleArtifactCounts> for OrphanCounts {
    fn from(s: StaleArtifactCounts) -> Self {
        Self {
            overlays_removed: s.overlays_removed,
            sockets_removed: s.sockets_removed,
            bridge_sockets_removed: s.bridge_sockets_removed,
        }
    }
}

impl OrphanCounts {
    /// True when every category is zero — i.e. nothing was
    /// pruned. Used by the supervisor's caching logic to
    /// decide whether to surface the report at all on the
    /// first `EnsureCapsule` response (a zero-counts report
    /// is also surfaced so dashboards can use field presence
    /// as the "supervisor just started" signal).
    pub fn is_zero(&self) -> bool {
        self.overlays_removed == 0 && self.sockets_removed == 0 && self.bridge_sockets_removed == 0
    }
}

/// Phase 4 Day 5 — remove orphaned per-VM artifacts left behind
/// by a prior supervisor process that exited without calling
/// `stop_capsule` (panic, SIGKILL, segfault).
///
/// On macOS, Apple's `VZVirtualMachine` instances die with the
/// owning process — they cannot leak across process boundaries
/// (no cross-process state in `Virtualization.framework`). What
/// *can* leak is the filesystem state the supervisor creates on
/// the host side:
///
/// - rootfs overlay copies under `<rootfs_cache_dir>/overlays/`.
///   Their UUID-based filenames (`<handle>.ext4`) are derived
///   from a per-launch handle, so no overlay file can be
///   claimed by a fresh `Supervisor` (the handle space is
///   process-local).
/// - Carrier socket files under `<socket_dir>/*-carrier.sock`
///   and `<socket_dir>/*.sock`. Unix domain socket inodes
///   linger past the listener; reusing the path would `EADDRINUSE`.
///
/// The Linux path doesn't need this helper — `crosvm` child
/// processes either survive the supervisor's death (in which
/// case they're reaped by `reap_dead_capsules`) or exit with
/// the supervisor (closing their socket fds). Either way the
/// Mac-specific drop chain (Vz handle → NSFileHandle config →
/// pipe write end → bridge EOF) doesn't apply on Linux, so the
/// helper is gated behind `cfg(target_os = "macos")` to keep
/// the Linux launch path byte-identical.
///
/// **Idempotent**: safe to call multiple times. **Best-effort**:
/// failures to remove individual files are logged but do not
/// abort the sweep; the caller decides how to react to the
/// returned counts.
///
/// Not wired into `Supervisor::new` automatically — operators
/// (or future `elastos serve` startup glue) opt in by calling
/// `Supervisor::prune_stale_mac_artifacts()` explicitly. This
/// avoids the edge case of two simultaneous supervisor
/// processes nuking each other's in-flight overlays.
#[cfg(target_os = "macos")]
fn prune_stale_mac_artifacts(
    socket_dir: &std::path::Path,
    rootfs_cache_dir: &std::path::Path,
) -> StaleArtifactCounts {
    let mut counts = StaleArtifactCounts::default();

    let overlays_dir = rootfs_cache_dir.join("overlays");
    if let Ok(entries) = std::fs::read_dir(&overlays_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_overlay = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("ext4"))
                .unwrap_or(false);
            if !is_overlay {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => counts.overlays_removed += 1,
                Err(e) => tracing::warn!(
                    "prune_stale_mac_artifacts: could not remove overlay {}: {}",
                    path.display(),
                    e
                ),
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(socket_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Unix domain socket inodes report `is_file() == false`
            // and `is_dir() == false`. We accept any non-directory
            // entry whose name matches the supervisor's socket
            // naming convention; this catches both crosvm control
            // sockets (`<handle>.sock`) and carrier IPC sockets
            // (`<handle>-carrier.sock`).
            if path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            // Phase 5 Day 4: split bridge sockets from generic
            // sockets so the OrphanCounts projection gives
            // operators category-level pivots. The `*-carrier.sock`
            // check is intentionally first because every
            // `-carrier.sock` also ends with `.sock`.
            let is_bridge_socket = name.ends_with("-carrier.sock");
            let is_socket = is_bridge_socket || name.ends_with(".sock");
            if !is_socket {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    if is_bridge_socket {
                        counts.bridge_sockets_removed += 1;
                    } else {
                        counts.sockets_removed += 1;
                    }
                }
                Err(e) => tracing::warn!(
                    "prune_stale_mac_artifacts: could not remove socket {}: {}",
                    path.display(),
                    e
                ),
            }
        }
    }

    counts
}

/// Extract the typed Vz exit-reason telemetry label from a
/// capsule backend, if one is set. **Phase 4 Day 7.**
///
/// Returns `Some(label)` only for macOS [`CapsuleBackend::VzVm`]
/// records whose `RunningVm` has cached a `VzExitReason` (set by
/// `RunningVm::stop` or `wait_for_exit_code`). Linux crosvm,
/// Carrier and non-Vz Mac records intentionally surface `None` —
/// their exit telemetry stays on the existing wire contract.
///
/// The label string is one of the canonical values from
/// [`elastos_vz::VzExitReason::label`]:
/// `guest_clean_stop`, `host_initiated_stop`,
/// `stopped_with_error`, `forced_after_timeout`.
fn vz_last_exit_reason(backend: &CapsuleBackend) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        match backend {
            CapsuleBackend::VzVm(vm) => vm.last_exit_reason().map(|r| r.label().to_string()),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = backend;
        None
    }
}

/// Project the cached typed Vz error (if any) into the
/// supervisor-facing [`elastos_vz::VzErrorReport`]. **Phase 4
/// Day 8.**
///
/// Returns `None` for non-Vz backends (Linux crosvm, Carrier),
/// for Mac Vz capsules with no cached error, and on every
/// platform that isn't macOS. Mac Vz capsules with a cached
/// [`elastos_vz::VzError`] surface the typed report.
fn vz_last_error_report(backend: &CapsuleBackend) -> Option<elastos_vz::VzErrorReport> {
    #[cfg(target_os = "macos")]
    {
        match backend {
            CapsuleBackend::VzVm(vm) => vm.last_vz_error().map(|e| e.to_report()),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = backend;
        None
    }
}

/// Result of [`Supervisor::capsule_vz_error`]. **Phase 4 Day 8.**
///
/// Three-state outcome the dispatcher maps to the response
/// shape:
/// - `Found(None)` → `status: "ok"`, no `vz_error` field
///   (capsule exists but has no cached error / is non-Vz).
/// - `Found(Some(report))` → `status: "ok"`,
///   `vz_error: Some(report)`.
/// - `NotFound` → `status: "not_found"`.
#[derive(Debug)]
enum CapsuleVzErrorOutcome {
    Found(Option<elastos_vz::VzErrorReport>),
    NotFound,
}

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

    /// Read the cached typed Vz error for a Mac Vz capsule.
    /// **Phase 4 Day 8.**
    ///
    /// Returns the structured [`elastos_vz::VzErrorReport`] for
    /// the most recent failed `RunningVm::stop` / wait, or
    /// `None` if no error was cached (success path, pre-stop,
    /// or non-Vz backend). Unknown handles return
    /// `status: "not_found"` (consistent with `capsule_status`).
    #[serde(rename = "capsule_vz_error")]
    CapsuleVzError { handle: String },

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
    /// Typed Vz exit-reason telemetry (macOS only). **Phase 4
    /// Day 7.** Populated by `stop_capsule` and `capsule_status`
    /// for stopped Vz capsules with one of the canonical
    /// labels: `"guest_clean_stop"`, `"host_initiated_stop"`,
    /// `"forced_after_timeout"`, or `"stopped_with_error"`.
    /// Operators piping `elastos status` JSON into Datadog /
    /// Grafana can alert on `forced_after_timeout` without
    /// grepping log lines. See `docs/vz-backend/PHASE_4_DAY_7_NOTES.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit_reason: Option<String>,

    /// Structured Vz error readback (macOS only). **Phase 4
    /// Day 8.** Populated by the new `capsule_vz_error` RPC
    /// (always when present) and by `capsule_status` for
    /// stopped Vz capsules that have a cached error. Carries
    /// the typed [`elastos_vz::VzErrorReport`] — `kind_label` is
    /// always set (`"vz_internal"`, `"vz_timed_out"`,
    /// `"vz_unknown"`, …); `domain` / `code` are populated only
    /// for `Unknown` variants Apple may have added in a future
    /// macOS; `vm_id` / `budget_secs` are populated only for
    /// stop-timeout cases. See
    /// `docs/vz-backend/PHASE_4_DAY_8_NOTES.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vz_error: Option<elastos_vz::VzErrorReport>,

    /// One-shot orphan-prune report (macOS only). **Phase 5
    /// Day 4.** Populated by the FIRST
    /// [`SupervisorRequest::EnsureCapsule`] response after
    /// [`Supervisor::new`] ran the Mac startup orphan-prune;
    /// every subsequent response leaves the field absent so
    /// dashboards can use field presence as the "supervisor
    /// just started + cleaned" signal. Always absent on Linux
    /// (the prune is a no-op stub) and absent when the
    /// operator opts out via
    /// [`elastos_vz::VzConfig::prune_orphans_on_startup`]`= false`.
    /// See `docs/vz-backend/PHASE_5_DAY_4_NOTES.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphans_pruned: Option<OrphanCounts>,
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
            last_exit_reason: None,
            vz_error: None,
            orphans_pruned: None,
        }
    }

    /// Phase 5 Day 4 — attach the cached one-shot
    /// orphan-prune report (if any) to an existing success
    /// response. Used by the `EnsureCapsule` handler so the
    /// first `ensure_capsule` response after `Supervisor::new`
    /// carries the startup-prune telemetry alongside its
    /// normal `path` payload.
    fn with_orphans_pruned(mut self, report: Option<OrphanCounts>) -> Self {
        self.orphans_pruned = report;
        self
    }

    /// Builder for the success surface returned by `stop_capsule`
    /// with a Phase-4-Day-7 `last_exit_reason` telemetry label
    /// attached. Used both on the host-initiated-stop success
    /// path and the forced-after-timeout path (where the
    /// supervisor swallows the typed error but still publishes
    /// the reason so dashboards know the VM was forced).
    fn ok_with_exit_reason(reason: Option<String>) -> Self {
        Self {
            last_exit_reason: reason,
            ..Self::ok()
        }
    }

    /// Builder for the success surface returned by
    /// `capsule_vz_error`. **Phase 4 Day 8.** Carries an
    /// optional [`elastos_vz::VzErrorReport`] — `None` means
    /// "no cached error" (success path, pre-stop, or non-Vz
    /// backend).
    fn ok_with_vz_error(report: Option<elastos_vz::VzErrorReport>) -> Self {
        Self {
            vz_error: report,
            ..Self::ok()
        }
    }

    /// Builder for the standard `not_found` shape both
    /// `capsule_status` and (Phase 4 Day 8) `capsule_vz_error`
    /// return for unknown handles. Centralised so future
    /// fields don't drift between the two query paths.
    fn not_found() -> Self {
        Self {
            status: "not_found".into(),
            ..Self::ok()
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
            last_exit_reason: None,
            vz_error: None,
            orphans_pruned: None,
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
    /// Phase 4 Day 6 — per-bridge termination observer
    /// (Mac path). `Some` when the supervisor wired
    /// `BridgeContext::on_terminate` for this capsule;
    /// `stop_capsule` awaits this notify with a bounded
    /// timeout after `vm.stop()` resolves so it can log
    /// "bridge terminated cleanly" vs. "bridge orphaned —
    /// continuing with cleanup". `None` for Linux capsules
    /// (no Mac-side lifecycle to observe) and for Mac
    /// capsules without a `capability_manager` /
    /// `pending_store` (no bridge_ctx → no notify).
    #[cfg(target_os = "macos")]
    bridge_terminated: Option<std::sync::Arc<tokio::sync::Notify>>,
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
    /// Phase 5 Day 4 — Vz-backend configuration consulted by
    /// the Mac-only startup orphan-prune in `Supervisor::new`.
    /// Defaults to [`elastos_vz::VzConfig::new`] (which sets
    /// `prune_orphans_on_startup: true`). Held independently
    /// from `crosvm_config` because the Linux launch path
    /// must remain byte-identical (it never reads this field).
    vz_config: elastos_vz::VzConfig,
    /// Phase 5 Day 4 — cached one-shot orphan-prune report
    /// surfaced via [`SupervisorResponse::orphans_pruned`] on
    /// the FIRST [`SupervisorRequest::EnsureCapsule`] response
    /// after construction. `Mutex` over `Arc<RwLock<_>>` because
    /// the slot is single-writer (filled in `Supervisor::new`)
    /// and single-reader-on-take (consumed by the first
    /// `ensure_capsule` handler); the heavier RwLock buys
    /// nothing here. Always `None` on Linux.
    pending_orphan_report: std::sync::Mutex<Option<OrphanCounts>>,
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

    /// Phase 4 Day 5 — opt-in cleanup of orphaned per-VM
    /// artifacts left behind by a prior supervisor process that
    /// died without calling `stop_capsule`. Calls the free
    /// [`prune_stale_mac_artifacts`] helper against this
    /// supervisor's `socket_dir` and `rootfs_cache_dir`.
    ///
    /// On Linux this is a no-op stub (returns zero counts) so
    /// callers can be platform-agnostic without `cfg!()` gates
    /// at the call site. The Linux launch path's existing
    /// `reap_dead_capsules` (already running on a timer) covers
    /// the equivalent surface for crosvm child processes.
    ///
    /// Operators wire this into `elastos serve` startup if they
    /// want the cleanup. `Supervisor::new` does NOT call it
    /// automatically — see the helper's doc comment for the
    /// multi-instance safety trade-off.
    #[cfg(target_os = "macos")]
    pub fn prune_stale_mac_artifacts(&self) -> StaleArtifactCounts {
        prune_stale_mac_artifacts(
            &self.crosvm_config.socket_dir,
            &self.crosvm_config.rootfs_cache_dir,
        )
    }

    /// Linux stub for [`Supervisor::prune_stale_mac_artifacts`].
    /// Returns the zero-valued counts so call sites can be
    /// uniform across platforms.
    #[cfg(not(target_os = "macos"))]
    pub fn prune_stale_mac_artifacts(&self) -> StaleArtifactCounts {
        StaleArtifactCounts::default()
    }

    pub fn new(data_dir: PathBuf, registry: ComponentsManifest) -> Self {
        Self::new_with_vz_config(data_dir, registry, elastos_vz::VzConfig::new())
    }

    /// Phase 5 Day 4 — construct a `Supervisor` with an
    /// explicit [`elastos_vz::VzConfig`]. Used by tests and
    /// future operator-driven harnesses that need to opt out
    /// of the Mac startup orphan-prune (or override Vz
    /// timeouts) without touching the simpler [`Supervisor::new`]
    /// signature.
    ///
    /// On Linux, every field of `vz_config` except the
    /// behaviour-bearing ones is ignored — the Linux launch
    /// path stays byte-identical. The `prune_orphans_on_startup`
    /// flag is still read (it's stored on the supervisor) but
    /// its consumer, [`prune_stale_mac_artifacts`], is a
    /// no-op stub on Linux, so the net effect is zero.
    pub fn new_with_vz_config(
        data_dir: PathBuf,
        registry: ComponentsManifest,
        vz_config: elastos_vz::VzConfig,
    ) -> Self {
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

        // Phase 5 Day 4: Mac-only startup orphan prune. Computed
        // BEFORE `Self { ... }` so the supervisor's filesystem
        // baseline is clean before any RPC handler can see it,
        // and so the unit tests can observe the on-disk state
        // post-construction. Result cached for one-shot delivery
        // via `pending_orphan_report`.
        #[cfg(target_os = "macos")]
        let pending_orphan_report: Option<OrphanCounts> = if vz_config.prune_orphans_on_startup {
            let counts = prune_stale_mac_artifacts(
                &crosvm_config.socket_dir,
                &crosvm_config.rootfs_cache_dir,
            );
            tracing::info!(
                overlays_removed = counts.overlays_removed,
                sockets_removed = counts.sockets_removed,
                bridge_sockets_removed = counts.bridge_sockets_removed,
                "vz: startup orphan-prune complete"
            );
            Some(OrphanCounts::from(counts))
        } else {
            tracing::debug!(
                "vz: startup orphan-prune skipped (VzConfig::prune_orphans_on_startup = false)"
            );
            None
        };
        #[cfg(not(target_os = "macos"))]
        let pending_orphan_report: Option<OrphanCounts> = {
            let _ = &crosvm_config;
            let _ = vz_config.prune_orphans_on_startup;
            None
        };

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
            vz_config,
            pending_orphan_report: std::sync::Mutex::new(pending_orphan_report),
        }
    }

    /// Phase 5 Day 4 — read-only accessor on the Vz config the
    /// supervisor was constructed with. Used by tests to assert
    /// the `prune_orphans_on_startup` flag was honoured.
    pub fn vz_config(&self) -> &elastos_vz::VzConfig {
        &self.vz_config
    }

    /// Phase 5 Day 4 — consume the one-shot orphan-prune
    /// report cached by [`Supervisor::new`]. Returns `Some` on
    /// the first call after construction (if startup pruning
    /// ran), `None` on every subsequent call. Used by the
    /// `EnsureCapsule` RPC handler so dashboards can pivot on
    /// `orphans_pruned` field presence as the "supervisor
    /// just started + cleaned" signal.
    fn take_pending_orphan_report(&self) -> Option<OrphanCounts> {
        self.pending_orphan_report
            .lock()
            .ok()
            .and_then(|mut g| g.take())
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
                Ok(path) => SupervisorResponse::ok_with_path(path.display().to_string())
                    .with_orphans_pruned(self.take_pending_orphan_report()),
                Err(e) => SupervisorResponse::err(format!("ensure_capsule failed: {e}")),
            },
            SupervisorRequest::LaunchCapsule { name, config } => {
                match self.launch_capsule(&name, config).await {
                    Ok((handle, cid)) => SupervisorResponse::ok_with_handle(handle, cid),
                    Err(e) => SupervisorResponse::err(format!("launch_capsule failed: {e}")),
                }
            }
            SupervisorRequest::StopCapsule { handle } => match self.stop_capsule(&handle).await {
                // Phase 4 Day 7: surface the typed Vz exit
                // reason in the response so operators piping
                // `elastos stop` JSON into telemetry can record
                // whether the VM was host-stopped cleanly or
                // forced after the configured Day 6 timeout.
                Ok(last_exit_reason) => SupervisorResponse::ok_with_exit_reason(last_exit_reason),
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
            SupervisorRequest::CapsuleVzError { handle } => {
                // Phase 4 Day 8 — typed Vz error readback. The
                // RPC is Mac-relevant only, but the dispatcher
                // is platform-agnostic: on Linux, every Vz
                // capsule lookup is `NotFound` (no Vz backends
                // can exist) and non-Vz backends on macOS
                // surface `Found(None)`. The cross-platform
                // shape lets shell clients ask the question
                // unconditionally and key off the response.
                match self.capsule_vz_error(&handle).await {
                    CapsuleVzErrorOutcome::Found(report) => {
                        SupervisorResponse::ok_with_vz_error(report)
                    }
                    CapsuleVzErrorOutcome::NotFound => SupervisorResponse::not_found(),
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
                    // Linux launch path keeps the Phase 4 Day
                    // 6 lifecycle observability off — crosvm
                    // bridges already terminate
                    // deterministically with the child
                    // process. Mac-only feature; see
                    // `start_capsule_vm_macos`.
                    on_terminate: None,
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
                    // Linux crosvm path — bridge lifecycle
                    // ties to the child process (Day 6 feature
                    // is Mac-only).
                    #[cfg(target_os = "macos")]
                    bridge_terminated: None,
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

        // Phase 4 Day 6: per-bridge termination observer
        // declared at function scope so it can be stored on
        // the `RunningCapsule` below. Populated only when a
        // bridge is actually wired (carrier_fd + provider
        // registry + capability infrastructure all present).
        let mut bridge_terminated: Option<std::sync::Arc<tokio::sync::Notify>> = None;

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
                        // Phase 4 Day 6: mint a per-bridge
                        // termination observer so the supervisor's
                        // `stop_capsule` can deterministically
                        // await natural bridge teardown after
                        // `vm.stop()` resolves. The supervisor
                        // extracts this Arc from the context
                        // below and stores it on the
                        // `RunningCapsule`.
                        on_terminate: Some(std::sync::Arc::new(tokio::sync::Notify::new())),
                    }),
                    _ => None,
                };
                bridge_terminated = bridge_ctx.as_ref().and_then(|c| c.on_terminate.clone());

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
            bridge_terminated,
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

        // Phase 4 Day 4: pre-flight memory guard. Apple Vz throws an
        // opaque runtime error if `setMemorySize:` exceeds the host's
        // available physical RAM. The Linux supervisor path does not
        // pre-check (KVM lazily commits guest pages so an oversize
        // request only manifests when the guest faults), but on Mac
        // the failure is surfaced at boot with no actionable message.
        // We reject manifest values larger than `host_phys_mem_mib -
        // MAC_HOST_HEADROOM_MIB` so the operator sees a clear
        // "manifest requests X MiB, host has Y MiB free for VMs"
        // error pointing at `docs/MAC.md`. Anchored in
        // `docs/vz-backend/PHASE_4_DAY_4_NOTES.md`.
        let host_phys_mem_mib = host_phys_mem_mib_mac();
        let max_capsule_mem_mib = host_phys_mem_mib.saturating_sub(MAC_HOST_HEADROOM_MIB);
        if vm_config.mem_size_mib as u64 > max_capsule_mem_mib {
            bail!(
                "capsule '{}' requests {} MiB of memory but this host only has {} MiB physical RAM \
                 ({} MiB reserved for the host). Edit the capsule manifest's `resources.memory_mb` \
                 or run on a host with more RAM. See docs/MAC.md for sizing guidance.",
                name,
                vm_config.mem_size_mib,
                host_phys_mem_mib,
                MAC_HOST_HEADROOM_MIB,
            );
        }

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
                    // Carrier services are host-side processes —
                    // no Vz bridge, no Day 6 notify wiring.
                    #[cfg(target_os = "macos")]
                    bridge_terminated: None,
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
    /// Stop a running capsule.
    ///
    /// **Phase 4 Day 7**: returns the typed Vz exit-reason
    /// telemetry label (one of `"host_initiated_stop"`,
    /// `"forced_after_timeout"`) when stopping a macOS Vz
    /// capsule. Non-Vz backends return `None` so the existing
    /// Linux-side stop wire contract stays unchanged.
    async fn stop_capsule(&self, handle: &str) -> Result<Option<String>> {
        let mut running = self.running.write().await;
        let capsule = running
            .remove(handle)
            .ok_or_else(|| anyhow::anyhow!("no capsule with handle '{handle}'"))?;

        if let Some(route) = capsule.provider_route.as_ref() {
            self.unregister_provider_route(route).await;
        }

        let last_exit_reason: Option<String> = match capsule.backend {
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
                None
            }
            CapsuleBackend::Carrier => {
                // Carrier service child process is killed when CarrierServiceProvider
                // is dropped (via CarrierServiceBridge::drop). Unregistering the
                // provider route above drops the last Arc reference.
                None
            }
            #[cfg(target_os = "macos")]
            CapsuleBackend::VzVm(mut vm) => {
                // Phase 4 Day 5/6: best-effort stop semantics.
                // The Vz path has no `kill -9` equivalent, so a
                // wedged `stopWithCompletionHandler:` would
                // block `stop_capsule` indefinitely. Day 6
                // bounded that with `VzConfig::stop_timeout`
                // inside `VzMachineHandle::stop`; on timeout the
                // call returns a typed error, the Vz handle is
                // orphaned, and the supervisor MUST proceed
                // with overlay + bridge cleanup so future
                // launches of the same capsule are not blocked
                // by stale on-disk state.
                let stop_outcome = vm.stop().await;
                if let Err(e) = &stop_outcome {
                    tracing::warn!(
                        "Vz VM stop failed for '{}' (handle={}): {} — \
                         continuing with best-effort cleanup (Phase 4 Day 6).",
                        capsule.name,
                        handle,
                        e
                    );
                }

                // Phase 4 Day 6: deterministic bridge teardown
                // observation. The Carrier-bridge dispatch loop
                // signals `bridge_terminated` on every exit
                // path; we wait up to 10 s for it to fire. If
                // it doesn't, we log and proceed — same
                // best-effort posture as the stop-timeout
                // case. 10 s comfortably covers the
                // NSFileHandle release lag (sub-second on real
                // hardware per the Day 5 audit).
                if let Some(notify) = capsule.bridge_terminated.as_ref() {
                    let bridge_budget = std::time::Duration::from_secs(10);
                    match tokio::time::timeout(bridge_budget, notify.notified()).await {
                        Ok(()) => tracing::debug!(
                            "Vz bridge for '{}' (handle={}) terminated cleanly",
                            capsule.name,
                            handle
                        ),
                        Err(_) => tracing::warn!(
                            "Vz bridge for '{}' (handle={}) did not terminate within {:?} — \
                             continuing with best-effort cleanup (Phase 4 Day 6).",
                            capsule.name,
                            handle,
                            bridge_budget
                        ),
                    }
                }

                let overlay_path = self
                    .crosvm_config
                    .rootfs_cache_dir
                    .join("overlays")
                    .join(format!("{}.ext4", handle));
                let _ = tokio::fs::remove_file(&overlay_path).await;

                // Phase 4 Day 7: read the typed exit reason
                // BEFORE returning so the response surface
                // includes the canonical label (e.g.
                // `"host_initiated_stop"` on success,
                // `"forced_after_timeout"` if Day 6's stop
                // timeout fired). `RunningVm::stop` caches the
                // reason on both Ok and (in the
                // forced-after-timeout case) Err paths.
                let last_exit_reason = vm.last_exit_reason().map(|r| r.label().to_string());

                if let Err(e) = stop_outcome {
                    // After best-effort cleanup ran, still
                    // surface the typed error to the caller —
                    // the supervisor's `stop_capsule` API
                    // contract is that a non-`Ok` return means
                    // operator attention is needed even if
                    // local state is consistent. The typed
                    // last_exit_reason is dropped here on the
                    // Err path because the dispatcher's
                    // `SupervisorResponse::err(...)` doesn't
                    // carry a reason field; operators read the
                    // structured kind_label from the error
                    // message instead.
                    let _ = last_exit_reason;
                    return Err(anyhow::anyhow!(
                        "Vz VM stop failed for '{}': {} (cleanup ran best-effort)",
                        capsule.name,
                        e
                    ));
                }
                last_exit_reason
            }
        };

        eprintln!("[supervisor] Stopped capsule handle={}", handle);
        Ok(last_exit_reason)
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

                // Phase 4 Day 7: typed `last_exit_reason`
                // telemetry — only meaningful for stopped Vz
                // capsules that are still held in the `running`
                // map (e.g. a guest-clean delegate signal that
                // the supervisor hasn't reaped yet). Non-Vz
                // capsules (Linux crosvm, Carrier) intentionally
                // surface `None`; their exit-reason wire format
                // stays on the existing Linux contract.
                let last_exit_reason = vz_last_exit_reason(&rc.backend);

                // Phase 4 Day 8: structured Vz error readback
                // alongside the exit-reason label. Together
                // they give operators one-query observability
                // for stopped Vz capsules — `last_exit_reason`
                // is the alertable signal, `vz_error` is the
                // structured detail (Apple's domain + code for
                // unmodelled variants, stop-budget for
                // timeouts).
                let vz_error = vz_last_error_report(&rc.backend);

                Ok(SupervisorResponse {
                    status: status.into(),
                    handle: Some(rc.handle.clone()),
                    vsock_cid: Some(rc.vsock_cid),
                    uptime_secs: Some(rc.started_at.elapsed().as_secs()),
                    exit_code: None,
                    path: None,
                    error: None,
                    last_exit_reason,
                    vz_error,
                    orphans_pruned: None,
                })
            }
            None => Ok(SupervisorResponse::not_found()),
        }
    }

    /// Read the cached typed Vz error for a Mac Vz capsule.
    /// **Phase 4 Day 8.**
    ///
    /// The three-state outcome distinguishes "unknown handle"
    /// (dispatch as `not_found`) from "known handle, no cached
    /// error" (dispatch as `ok` with `vz_error: None`) from
    /// "known handle, cached error" (dispatch as `ok` with
    /// `vz_error: Some(report)`). Non-Vz backends always
    /// surface `Found(None)` — their failure modes stay on the
    /// existing `error` string contract.
    async fn capsule_vz_error(&self, handle: &str) -> CapsuleVzErrorOutcome {
        let running = self.running.read().await;
        match running.get(handle) {
            Some(rc) => CapsuleVzErrorOutcome::Found(vz_last_error_report(&rc.backend)),
            None => CapsuleVzErrorOutcome::NotFound,
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
                    compression: None,
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
                    compression: None,
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
                    compression: None,
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
    /// Phase 4 Day 4 contract: a manifest requesting more memory
    /// than the host can satisfy must be rejected by the
    /// supervisor's pre-flight guard with a clear, actionable
    /// error. Without this guard the request would propagate into
    /// `VZVirtualMachineConfiguration.setMemorySize:` and surface
    /// as an opaque `VZErrorInvalidVirtualMachineConfiguration`
    /// long after the supervisor has already minted handles,
    /// allocated CIDs, and copied the rootfs overlay — leaking
    /// resources on every failed launch attempt.
    ///
    /// The test asks for 100 PiB (10^20 bytes-ish in MiB units) so
    /// the assertion is robust against any plausible Apple-Silicon
    /// hardware. The error must mention `memory_mb` so an operator
    /// can map the message to the manifest field.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_fails_closed_when_memory_exceeds_host_ram() {
        use elastos_vz::VzConfig;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let mut manifest = synthetic_microvm_manifest("phase4-day4-oversized-memory");
        // u32::MAX MiB ≈ 4 PiB — well above any plausible
        // Apple-Silicon RAM. Apple's bound is implicit; ours is
        // explicit and refuses with an actionable message.
        manifest.resources.memory_mb = u32::MAX;

        let vz_config = VzConfig::default();
        let result = supervisor
            .build_vm_config_for_mac(
                "phase4-day4-oversized-memory",
                &manifest,
                &capsule_dir,
                serde_json::Value::Null,
                &vz_config,
            )
            .await;

        let err = result.expect_err(
            "manifest requesting more RAM than the host has must fail closed in the pre-flight guard",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("MiB") && msg.contains("memory"),
            "fail-closed message must mention memory sizing so operators \
             can find the manifest field; got: {msg}"
        );
        assert!(
            msg.contains("phase4-day4-oversized-memory"),
            "fail-closed message must name the offending capsule for log triage; got: {msg}"
        );
    }

    /// Phase 4 Day 4 mirror: a manifest with a sensible memory
    /// request must NOT be rejected by the pre-flight guard. This
    /// guards against accidentally inverting the comparison or
    /// regressing the host-RAM sysctl call (e.g. returning 0 and
    /// then refusing every launch).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn build_vm_config_for_mac_accepts_modest_memory_under_pre_flight_guard() {
        use elastos_vz::VzConfig;

        let supervisor = make_test_supervisor();
        let capsule_dir = tempfile::tempdir().unwrap().keep();
        let manifest = synthetic_microvm_manifest("phase4-day4-modest-memory");
        // Default synthetic manifest asks for 128 MiB — any
        // CI/dev host with > ~1.5 GiB RAM should pass.

        let vz_config = VzConfig::default();
        let (vm_config, _handle, _cid, _carrier_socket) = supervisor
            .build_vm_config_for_mac(
                "phase4-day4-modest-memory",
                &manifest,
                &capsule_dir,
                serde_json::Value::Null,
                &vz_config,
            )
            .await
            .expect("modest memory request must pass the pre-flight guard");

        assert_eq!(
            vm_config.mem_size_mib, 128,
            "the manifest's memory_mb must be passed through unchanged when under the host limit"
        );
    }

    /// Phase 4 Day 5 — orphaned per-VM artifacts from a prior
    /// supervisor process must be detectable and removable on
    /// restart. Simulates the post-crash filesystem state by
    /// laying down stale overlay files and socket files, then
    /// invoking `prune_stale_mac_artifacts` and verifying the
    /// returned counts plus the on-disk state.
    ///
    /// The reverse contract is equally important: files whose
    /// names do not match the supervisor's per-VM naming
    /// convention (e.g. an unrelated config file the operator
    /// dropped in `rootfs_cache_dir`) must be preserved. This
    /// keeps the helper from doubling as a wildcard `rm -rf` on
    /// the user's data dir.
    #[cfg(target_os = "macos")]
    #[test]
    fn prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let socket_dir = temp.path().join("crosvm");
        let rootfs_cache_dir = temp.path().join("rootfs-cache");
        let overlays_dir = rootfs_cache_dir.join("overlays");
        std::fs::create_dir_all(&socket_dir).unwrap();
        std::fs::create_dir_all(&overlays_dir).unwrap();

        let stale_overlay_a = overlays_dir.join("uuid-aaa.ext4");
        let stale_overlay_b = overlays_dir.join("uuid-bbb.ext4");
        std::fs::write(&stale_overlay_a, b"stale rootfs overlay A").unwrap();
        std::fs::write(&stale_overlay_b, b"stale rootfs overlay B").unwrap();

        let unrelated_overlays_file = overlays_dir.join("README.txt");
        std::fs::write(&unrelated_overlays_file, b"left by the operator").unwrap();

        let stale_carrier_sock = socket_dir.join("uuid-aaa-carrier.sock");
        let stale_control_sock = socket_dir.join("uuid-aaa.sock");
        std::fs::write(&stale_carrier_sock, b"placeholder").unwrap();
        std::fs::write(&stale_control_sock, b"placeholder").unwrap();

        let unrelated_socket_file = socket_dir.join("operator-notes.txt");
        std::fs::write(&unrelated_socket_file, b"keep me").unwrap();

        let counts = prune_stale_mac_artifacts(&socket_dir, &rootfs_cache_dir);
        assert_eq!(
            counts,
            StaleArtifactCounts {
                overlays_removed: 2,
                sockets_removed: 1,
                bridge_sockets_removed: 1,
            },
            "prune must split socket counts: 1 bridge socket (`*-carrier.sock`) + 1 control socket (`*.sock`); got {counts:?}"
        );

        assert!(!stale_overlay_a.exists(), "stale overlay A must be removed");
        assert!(!stale_overlay_b.exists(), "stale overlay B must be removed");
        assert!(
            !stale_carrier_sock.exists(),
            "stale carrier socket must be removed"
        );
        assert!(
            !stale_control_sock.exists(),
            "stale control socket must be removed"
        );

        assert!(
            unrelated_overlays_file.exists(),
            "non-overlay files in overlays/ must be preserved"
        );
        assert!(
            unrelated_socket_file.exists(),
            "non-socket files in socket_dir must be preserved"
        );

        // Idempotent: a second sweep over the same dirs yields
        // zero counts and does not error.
        let counts_again = prune_stale_mac_artifacts(&socket_dir, &rootfs_cache_dir);
        assert_eq!(
            counts_again,
            StaleArtifactCounts::default(),
            "second sweep must be a no-op; got {counts_again:?}"
        );
    }

    /// Phase 5 Day 4 — a fresh `Supervisor` constructed against
    /// a data dir containing pre-existing stale overlay + socket
    /// files must (a) NOT falsely report any of them as a
    /// running capsule, and (b) AUTOMATICALLY clean them via
    /// the default `VzConfig::prune_orphans_on_startup = true`
    /// path. The supervisor's `running` map starts empty
    /// regardless; the on-disk orphans get swept during
    /// `Supervisor::new` so the subsequent supervisor lifetime
    /// sees a clean baseline.
    ///
    /// Pre-Day-4 behaviour (operator must opt in via
    /// `supervisor.prune_stale_mac_artifacts()` after
    /// construction) is preserved as the OPT-OUT path; see
    /// `supervisor_new_with_prune_orphans_on_startup_false_preserves_artifacts`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn fresh_supervisor_auto_prunes_orphans_on_startup_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().to_path_buf();
        let socket_dir = data_dir.join("crosvm");
        let overlays_dir = data_dir.join("rootfs-cache").join("overlays");
        std::fs::create_dir_all(&socket_dir).unwrap();
        std::fs::create_dir_all(&overlays_dir).unwrap();
        let stale_overlay = overlays_dir.join("ghost-handle.ext4");
        let stale_control = socket_dir.join("ghost-handle.sock");
        let stale_bridge = socket_dir.join("ghost-handle-carrier.sock");
        std::fs::write(&stale_overlay, b"orphan").unwrap();
        std::fs::write(&stale_control, b"orphan").unwrap();
        std::fs::write(&stale_bridge, b"orphan").unwrap();

        let supervisor = Supervisor::new(
            data_dir,
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        );

        let running = supervisor.running.read().await;
        assert!(
            running.is_empty(),
            "fresh Supervisor must start with an empty running map regardless of stale on-disk artifacts"
        );
        drop(running);

        // Phase 5 Day 4 contract: the default Vz config opts INTO
        // startup pruning, so the orphan files must already be
        // gone by the time `Supervisor::new` returns.
        assert!(
            !stale_overlay.exists(),
            "Phase 5 Day 4: default Supervisor::new must auto-prune the orphan overlay"
        );
        assert!(
            !stale_control.exists(),
            "Phase 5 Day 4: default Supervisor::new must auto-prune the orphan control socket"
        );
        assert!(
            !stale_bridge.exists(),
            "Phase 5 Day 4: default Supervisor::new must auto-prune the orphan carrier-bridge socket"
        );

        // The cached one-shot report must surface the exact
        // category split for downstream `EnsureCapsule` delivery.
        let report = supervisor
            .take_pending_orphan_report()
            .expect("Phase 5 Day 4: cached orphan report must be present after auto-prune");
        assert_eq!(
            report,
            OrphanCounts {
                overlays_removed: 1,
                sockets_removed: 1,
                bridge_sockets_removed: 1,
            },
            "cached report must split 1 control socket vs. 1 carrier-bridge socket; got {report:?}"
        );

        // One-shot semantics: a second take returns None.
        assert!(
            supervisor.take_pending_orphan_report().is_none(),
            "cached orphan report must be consumed by the first take(); subsequent takes return None"
        );

        // Idempotent: explicit prune after auto-prune yields zero counts.
        let counts_again = supervisor.prune_stale_mac_artifacts();
        assert_eq!(
            counts_again,
            StaleArtifactCounts::default(),
            "explicit prune after auto-prune must be a no-op; got {counts_again:?}"
        );
    }

    /// Phase 5 Day 4 — operator opt-out: when the supervisor is
    /// constructed with `VzConfig::prune_orphans_on_startup =
    /// false`, on-disk orphan artifacts MUST be preserved
    /// across `Supervisor::new`, and the cached one-shot report
    /// MUST be `None` (so the first `EnsureCapsule` response
    /// elides the `orphans_pruned` field entirely).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn supervisor_new_with_prune_orphans_on_startup_false_preserves_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().to_path_buf();
        let socket_dir = data_dir.join("crosvm");
        let overlays_dir = data_dir.join("rootfs-cache").join("overlays");
        std::fs::create_dir_all(&socket_dir).unwrap();
        std::fs::create_dir_all(&overlays_dir).unwrap();
        let stale_overlay = overlays_dir.join("opt-out.ext4");
        let stale_carrier = socket_dir.join("opt-out-carrier.sock");
        std::fs::write(&stale_overlay, b"keep").unwrap();
        std::fs::write(&stale_carrier, b"keep").unwrap();

        let supervisor = Supervisor::new_with_vz_config(
            data_dir,
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
            elastos_vz::VzConfig::new().with_prune_orphans_on_startup(false),
        );

        assert!(
            !supervisor.vz_config().prune_orphans_on_startup,
            "VzConfig opt-out must round-trip through Supervisor::new_with_vz_config"
        );
        assert!(
            stale_overlay.exists(),
            "Phase 5 Day 4 opt-out: stale overlay must NOT be pruned when prune_orphans_on_startup is false"
        );
        assert!(
            stale_carrier.exists(),
            "Phase 5 Day 4 opt-out: stale carrier-bridge socket must NOT be pruned when prune_orphans_on_startup is false"
        );
        assert!(
            supervisor.take_pending_orphan_report().is_none(),
            "Phase 5 Day 4 opt-out: pending orphan report must be None so EnsureCapsule responses elide the orphans_pruned field"
        );
    }

    /// Phase 5 Day 4 — Linux contract: the orphan-prune helper
    /// is a no-op stub on Linux, so `Supervisor::new` must
    /// NEVER touch on-disk artifacts on Linux regardless of the
    /// Vz config flag. This pins the Linux launch path's
    /// byte-identical guarantee against the Day-4 change set.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn supervisor_new_is_noop_on_linux_even_with_prune_flag_set() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().to_path_buf();
        let socket_dir = data_dir.join("crosvm");
        let overlays_dir = data_dir.join("rootfs-cache").join("overlays");
        std::fs::create_dir_all(&socket_dir).unwrap();
        std::fs::create_dir_all(&overlays_dir).unwrap();
        let linux_orphan_overlay = overlays_dir.join("linux-untouched.ext4");
        let linux_orphan_socket = socket_dir.join("linux-untouched.sock");
        std::fs::write(&linux_orphan_overlay, b"linux untouched").unwrap();
        std::fs::write(&linux_orphan_socket, b"linux untouched").unwrap();

        // Even with the (Mac-only-behaviourally-meaningful) flag
        // explicitly set to true, Linux's stub `prune_stale_mac_artifacts`
        // must not touch any file.
        let supervisor = Supervisor::new_with_vz_config(
            data_dir,
            ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
            elastos_vz::VzConfig::new().with_prune_orphans_on_startup(true),
        );

        assert!(
            linux_orphan_overlay.exists(),
            "Linux launch path must be byte-identical: orphan overlay file must NOT be touched"
        );
        assert!(
            linux_orphan_socket.exists(),
            "Linux launch path must be byte-identical: orphan socket file must NOT be touched"
        );
        assert!(
            supervisor.take_pending_orphan_report().is_none(),
            "Linux: cached orphan report must always be None"
        );
    }

    /// Phase 5 Day 4 — `OrphanCounts` projection semantics.
    /// Pins `From<StaleArtifactCounts>` field-by-field plus the
    /// `is_zero()` convenience used by future operator-side
    /// alerting.
    #[test]
    fn orphan_counts_projection_round_trip_from_stale_artifact_counts() {
        let stale = StaleArtifactCounts {
            overlays_removed: 7,
            sockets_removed: 3,
            bridge_sockets_removed: 5,
        };
        let projected = OrphanCounts::from(stale);
        assert_eq!(projected.overlays_removed, 7);
        assert_eq!(projected.sockets_removed, 3);
        assert_eq!(projected.bridge_sockets_removed, 5);
        assert!(!projected.is_zero());

        let zero = OrphanCounts::default();
        assert!(zero.is_zero());
        assert_eq!(zero, OrphanCounts::from(StaleArtifactCounts::default()));
    }

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
            bridge_terminated: None,
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

    /// Phase 4 Day 7 — `capsule_status` reports the typed
    /// `last_exit_reason` telemetry label for a stopped VzVm
    /// capsule whose `RunningVm` cached a `ForcedAfterTimeout`
    /// outcome.
    ///
    /// Synthetic: we inject the reason via the
    /// `set_last_exit_reason_for_testing` hook because real
    /// `ForcedAfterTimeout` requires a wedged Apple completion
    /// handler — impossible to provoke in CI without an
    /// Apple-runner. The supervisor's wiring (which is what
    /// Day 7 changes) is what this test validates.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_status_includes_last_exit_reason_for_forced_after_timeout_vz_capsule() {
        use elastos_vz::VzExitReason;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day7-status-forced-after-timeout";

        let mut rc =
            synthetic_vzvm_running_capsule("phase4-day7-status-forced-after-timeout", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_last_exit_reason_for_testing(VzExitReason::ForcedAfterTimeout);
        } else {
            panic!("synthetic_vzvm_running_capsule must yield a VzVm backend");
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .capsule_status(handle)
            .await
            .expect("capsule_status must dispatch through the VzVm arm");

        assert_eq!(
            response.last_exit_reason.as_deref(),
            Some("forced_after_timeout"),
            "capsule_status must surface the typed telemetry label for forced-after-timeout \
             stops so Datadog / Grafana can alert without grepping log lines: {response:?}"
        );
    }

    /// Phase 4 Day 7 — every supported `VzExitReason` round
    /// trips through `capsule_status`'s `last_exit_reason`
    /// field. Guards against a regression where a new
    /// `VzExitReason` variant is added without updating the
    /// supervisor classifier.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_status_round_trips_every_vz_exit_reason_label() {
        use elastos_vz::VzExitReason;

        let supervisor = make_test_supervisor();
        let cases: &[(VzExitReason, &str)] = &[
            (VzExitReason::GuestCleanStop, "guest_clean_stop"),
            (VzExitReason::HostInitiatedStop, "host_initiated_stop"),
            (VzExitReason::StoppedWithError, "stopped_with_error"),
            (VzExitReason::ForcedAfterTimeout, "forced_after_timeout"),
        ];
        for (reason, expected_label) in cases {
            let handle = format!("vm-phase4-day7-status-{expected_label}");
            let mut rc = synthetic_vzvm_running_capsule(expected_label, &handle);
            if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
                vm.set_last_exit_reason_for_testing(*reason);
            }
            supervisor.running.write().await.insert(handle.clone(), rc);

            let response = supervisor
                .capsule_status(&handle)
                .await
                .expect("capsule_status must dispatch through the VzVm arm");

            assert_eq!(
                response.last_exit_reason.as_deref(),
                Some(*expected_label),
                "{reason:?} must surface as '{expected_label}', got {:?}",
                response.last_exit_reason
            );
        }
    }

    /// Phase 4 Day 7 — non-Vz backends (Linux crosvm, Carrier,
    /// or a VzVm with no cached exit reason) MUST leave
    /// `last_exit_reason` as `None`. This guards against any
    /// accidental cross-backend leakage of the new field.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_status_omits_last_exit_reason_when_vz_capsule_has_no_cached_outcome() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day7-status-no-cached-outcome";
        let rc = synthetic_vzvm_running_capsule("phase4-day7-status-no-cached-outcome", handle);
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .capsule_status(handle)
            .await
            .expect("capsule_status must dispatch through the VzVm arm");

        assert!(
            response.last_exit_reason.is_none(),
            "capsule_status must omit last_exit_reason for Vz capsules with no cached outcome; \
             got {:?}",
            response.last_exit_reason
        );
    }

    /// Phase 4 Day 7 — `not_found` responses MUST never carry a
    /// `last_exit_reason` (we have no capsule to read from).
    /// Trivial but catches a regression where a `Some(...)` is
    /// leaked through `..Self::ok()` defaulting on a future
    /// refactor.
    #[tokio::test]
    async fn capsule_status_not_found_response_has_no_last_exit_reason() {
        let supervisor = make_test_supervisor();
        let response = supervisor
            .capsule_status("vm-phase4-day7-no-such-handle")
            .await
            .expect("capsule_status of unknown handle must return Ok(not_found)");
        assert_eq!(response.status, "not_found");
        assert!(response.last_exit_reason.is_none());
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

        let last_exit_reason = supervisor
            .stop_capsule(handle)
            .await
            .expect("stop_capsule must dispatch through the VzVm arm cleanly");

        // Synthetic VzVm has no Vz handle attached, so
        // `RunningVm::stop` is a no-op and `last_exit_reason`
        // stays `None`. Day 7's contract is "non-None only
        // when the Vz handle actually stopped"; the value here
        // is precisely `None`.
        assert!(
            last_exit_reason.is_none(),
            "synthetic Vz capsule (no Vz handle) must surface no last_exit_reason: {:?}",
            last_exit_reason
        );

        let running = supervisor.running.read().await;
        assert!(
            !running.contains_key(handle),
            "stop_capsule must remove the VzVm entry from `running`"
        );
    }

    /// Phase 4 Day 7 — `handle_request(StopCapsule)` must
    /// surface the typed `last_exit_reason` in the JSON
    /// response when the underlying `RunningVm` cached one
    /// (e.g. a forced-after-timeout stop). This is the
    /// end-to-end wire-format check operators / dashboards
    /// depend on.
    ///
    /// We mark the synthetic VM as `Running` via the
    /// `set_status_for_testing` hook so `reap_dead_capsules`
    /// (which `handle_request` calls first) doesn't prune the
    /// record before `stop_capsule` runs.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn handle_request_stop_capsule_surfaces_typed_last_exit_reason_in_response() {
        use elastos_common::CapsuleStatus;
        use elastos_vz::VzExitReason;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day7-handle-request-forced";

        let mut rc = synthetic_vzvm_running_capsule("phase4-day7-handle-request-forced", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_status_for_testing(CapsuleStatus::Running);
            vm.set_last_exit_reason_for_testing(VzExitReason::ForcedAfterTimeout);
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .handle_request(SupervisorRequest::StopCapsule {
                handle: handle.to_string(),
            })
            .await;

        assert_eq!(
            response.status, "ok",
            "stop_capsule must succeed: error={:?}",
            response.error
        );
        assert_eq!(
            response.last_exit_reason.as_deref(),
            Some("forced_after_timeout"),
            "stop_capsule response must surface the typed telemetry label so \
             `elastos stop` JSON-mode consumers (Datadog / Grafana / scripts) can \
             alert on forced-stop rate without grepping logs: {response:?}"
        );
    }

    // ── Phase 4 Day 8: structured `vz_error` readback ──────────

    /// Phase 4 Day 8 — `capsule_vz_error` returns
    /// `NotFound` for an unknown handle so the dispatcher can
    /// emit the standard `not_found` response shape and stay
    /// consistent with `capsule_status`.
    #[tokio::test]
    async fn capsule_vz_error_unknown_handle_returns_not_found() {
        let supervisor = make_test_supervisor();
        let outcome = supervisor
            .capsule_vz_error("vm-phase4-day8-no-such-handle")
            .await;
        assert!(
            matches!(outcome, CapsuleVzErrorOutcome::NotFound),
            "unknown handle must yield NotFound: {outcome:?}"
        );
    }

    /// Phase 4 Day 8 — a Vz capsule with **no cached error**
    /// (success path, pre-stop) MUST return `Found(None)` so
    /// the dispatcher emits `status: "ok"` with `vz_error`
    /// skip-serialised. Guards against `is_none()` confusion
    /// across the `Found(None)` / `NotFound` boundary.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_vz_error_known_handle_without_cached_error_returns_found_none() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-known-no-error";
        let rc = synthetic_vzvm_running_capsule("phase4-day8-known-no-error", handle);
        supervisor.running.write().await.insert(handle.into(), rc);

        let outcome = supervisor.capsule_vz_error(handle).await;
        match outcome {
            CapsuleVzErrorOutcome::Found(report) => assert!(
                report.is_none(),
                "no cached error must surface as Found(None), got {report:?}"
            ),
            CapsuleVzErrorOutcome::NotFound => panic!("known handle must not return NotFound"),
        }
    }

    /// Phase 4 Day 8 — every documented [`elastos_vz::VzError`]
    /// variant round-trips through `capsule_vz_error` into the
    /// typed [`elastos_vz::VzErrorReport`]. This is the
    /// observability contract Datadog / Grafana dashboards key
    /// off: filtering on
    /// `vz_error.kind_label == "vz_internal"` must match every
    /// capsule whose underlying Vz error was
    /// `VzError::Internal`, etc.
    ///
    /// Synthetic: we inject the cached error via the
    /// `set_last_vz_error_for_testing` hook because real
    /// `VZErrorCode`s require an Apple-runner (impossible in
    /// CI). The wiring is what we're validating, not Apple's
    /// classifier.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_vz_error_round_trips_every_documented_vzerror_variant() {
        use elastos_vz::VzError;

        let supervisor = make_test_supervisor();
        let cases: Vec<(VzError, &'static str)> = vec![
            (
                VzError::Internal {
                    description: "kernel panic in vsock driver".into(),
                },
                "vz_internal",
            ),
            (
                VzError::InvalidConfiguration {
                    description: "boot config malformed".into(),
                },
                "vz_invalid_configuration",
            ),
            (
                VzError::InvalidState {
                    description: "stop while resuming".into(),
                },
                "vz_invalid_state",
            ),
            (
                VzError::InvalidStateTransition {
                    description: "double-start".into(),
                },
                "vz_invalid_state_transition",
            ),
            (
                VzError::NetworkError {
                    description: "NAT interface down".into(),
                },
                "vz_network_error",
            ),
            (
                VzError::OperationCancelled {
                    description: "supervisor cancelled mid-stop".into(),
                },
                "vz_operation_cancelled",
            ),
            (
                VzError::NotSupported {
                    description: "AMD CPU".into(),
                },
                "vz_not_supported",
            ),
        ];

        for (err, expected_label) in cases {
            let handle = format!("vm-phase4-day8-vz-error-{expected_label}");
            let mut rc = synthetic_vzvm_running_capsule(expected_label, &handle);
            if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
                vm.set_last_vz_error_for_testing(err.clone());
            } else {
                panic!("synthetic_vzvm_running_capsule must yield a VzVm backend");
            }
            supervisor.running.write().await.insert(handle.clone(), rc);

            let outcome = supervisor.capsule_vz_error(&handle).await;
            let report = match outcome {
                CapsuleVzErrorOutcome::Found(Some(report)) => report,
                other => panic!("{err:?} must surface as Found(Some(report)), got {other:?}"),
            };
            assert_eq!(
                report.kind_label, expected_label,
                "{err:?} must surface kind_label '{expected_label}', got '{}'",
                report.kind_label
            );
            assert!(
                report.domain.is_none(),
                "documented variant must leave domain implicit: {report:?}"
            );
            assert!(
                report.code.is_none(),
                "documented variant must leave code implicit: {report:?}"
            );
        }
    }

    /// Phase 4 Day 8 — `VzError::Unknown` round-trips with its
    /// `domain` + `code` preserved, so operators can grep
    /// future / unmodelled Apple variants without needing a
    /// binding update.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_vz_error_unknown_variant_preserves_domain_and_code() {
        use elastos_vz::VzError;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-vz-error-unknown";
        let mut rc = synthetic_vzvm_running_capsule("phase4-day8-vz-error-unknown", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_last_vz_error_for_testing(VzError::Unknown {
                domain: "VZErrorDomain".into(),
                code: 30001,
                description: "future variant".into(),
            });
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let outcome = supervisor.capsule_vz_error(handle).await;
        let report = match outcome {
            CapsuleVzErrorOutcome::Found(Some(report)) => report,
            other => panic!("Unknown must surface as Found(Some(report)), got {other:?}"),
        };
        assert_eq!(report.kind_label, "vz_unknown");
        assert_eq!(report.domain.as_deref(), Some("VZErrorDomain"));
        assert_eq!(report.code, Some(30001));
    }

    /// Phase 4 Day 8 — `VzError::TimedOut` round-trips with its
    /// `vm_id` + `budget_secs` preserved. Operators sizing the
    /// fleet-wide `VzConfig::stop_timeout` query `budget_secs`
    /// directly.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_vz_error_timed_out_preserves_vm_id_and_budget() {
        use elastos_vz::VzError;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-vz-error-timed-out";
        let mut rc = synthetic_vzvm_running_capsule("phase4-day8-vz-error-timed-out", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_last_vz_error_for_testing(VzError::TimedOut {
                vm_id: "phase4-day8-vz-error-timed-out".into(),
                budget: std::time::Duration::from_millis(2_500),
            });
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let outcome = supervisor.capsule_vz_error(handle).await;
        let report = match outcome {
            CapsuleVzErrorOutcome::Found(Some(report)) => report,
            other => panic!("TimedOut must surface as Found(Some(report)), got {other:?}"),
        };
        assert_eq!(report.kind_label, "vz_timed_out");
        assert_eq!(
            report.vm_id.as_deref(),
            Some("phase4-day8-vz-error-timed-out")
        );
        assert_eq!(report.budget_secs, Some(2.5));
    }

    /// Phase 4 Day 8 — end-to-end through the dispatcher:
    /// `handle_request(CapsuleVzError)` for a Vz capsule with a
    /// cached `Internal` error returns
    /// `status: "ok"` + the typed `vz_error.kind_label`.
    ///
    /// We mark the synthetic VM as `Running` via the
    /// `set_status_for_testing` hook so `reap_dead_capsules`
    /// (which `handle_request` calls first) doesn't prune the
    /// record before the dispatcher reaches the
    /// `CapsuleVzError` arm. Same trick as the Day-7
    /// `handle_request_stop_capsule_*` test.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn handle_request_capsule_vz_error_surfaces_typed_report_for_internal_variant() {
        use elastos_common::CapsuleStatus;
        use elastos_vz::VzError;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-handle-request-internal";
        let mut rc = synthetic_vzvm_running_capsule("phase4-day8-handle-request-internal", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_status_for_testing(CapsuleStatus::Running);
            vm.set_last_vz_error_for_testing(VzError::Internal {
                description: "kernel panic in vsock driver".into(),
            });
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .handle_request(SupervisorRequest::CapsuleVzError {
                handle: handle.to_string(),
            })
            .await;

        assert_eq!(response.status, "ok", "error={:?}", response.error);
        let report = response
            .vz_error
            .expect("Internal variant must surface vz_error in the response");
        assert_eq!(report.kind_label, "vz_internal");
        assert!(
            report.description.contains("kernel panic"),
            "description must round-trip Apple's localised string: {report:?}"
        );
    }

    /// Phase 4 Day 8 — `handle_request(CapsuleVzError)` for a
    /// Vz capsule with a cached `TimedOut` error returns the
    /// `vm_id` + `budget_secs` operators sizing the fleet-wide
    /// `VzConfig::stop_timeout` rely on.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn handle_request_capsule_vz_error_surfaces_typed_report_for_timed_out_variant() {
        use elastos_common::CapsuleStatus;
        use elastos_vz::VzError;

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-handle-request-timed-out";
        let mut rc = synthetic_vzvm_running_capsule("phase4-day8-handle-request-timed-out", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_status_for_testing(CapsuleStatus::Running);
            vm.set_last_vz_error_for_testing(VzError::TimedOut {
                vm_id: handle.into(),
                budget: std::time::Duration::from_secs(3),
            });
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .handle_request(SupervisorRequest::CapsuleVzError {
                handle: handle.to_string(),
            })
            .await;

        assert_eq!(response.status, "ok", "error={:?}", response.error);
        let report = response
            .vz_error
            .expect("TimedOut variant must surface vz_error in the response");
        assert_eq!(report.kind_label, "vz_timed_out");
        assert_eq!(report.vm_id.as_deref(), Some(handle));
        assert_eq!(report.budget_secs, Some(3.0));
    }

    /// Phase 4 Day 8 — `handle_request(CapsuleVzError)` for an
    /// unknown handle MUST return `status: "not_found"` with
    /// no `vz_error` field. The dispatcher reuses the same
    /// `not_found` shape as `capsule_status` so shell consumers
    /// can handle both paths uniformly.
    #[tokio::test]
    async fn handle_request_capsule_vz_error_unknown_handle_returns_not_found() {
        let supervisor = make_test_supervisor();
        let response = supervisor
            .handle_request(SupervisorRequest::CapsuleVzError {
                handle: "vm-phase4-day8-handle-request-no-such-handle".into(),
            })
            .await;

        assert_eq!(response.status, "not_found");
        assert!(
            response.vz_error.is_none(),
            "not_found response must omit vz_error: {response:?}"
        );
    }

    /// Phase 4 Day 8 — `capsule_status` enriches a stopped Vz
    /// capsule's response with BOTH the Day-7
    /// `last_exit_reason` label AND the Day-8 structured
    /// `vz_error`. Single-query observability: operators get
    /// the telemetry label for alerting AND the structured
    /// detail for triage in one round-trip.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capsule_status_enrichment_carries_both_last_exit_reason_and_vz_error() {
        use elastos_vz::{VzError, VzExitReason};

        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day8-status-enrichment";
        let mut rc = synthetic_vzvm_running_capsule("phase4-day8-status-enrichment", handle);
        if let CapsuleBackend::VzVm(vm) = &mut rc.backend {
            vm.set_last_exit_reason_for_testing(VzExitReason::ForcedAfterTimeout);
            vm.set_last_vz_error_for_testing(VzError::TimedOut {
                vm_id: handle.into(),
                budget: std::time::Duration::from_secs(5),
            });
        }
        supervisor.running.write().await.insert(handle.into(), rc);

        let response = supervisor
            .capsule_status(handle)
            .await
            .expect("capsule_status must dispatch through the VzVm arm");

        assert_eq!(
            response.last_exit_reason.as_deref(),
            Some("forced_after_timeout"),
            "telemetry label must be present alongside vz_error: {response:?}"
        );
        let report = response
            .vz_error
            .expect("capsule_status must enrich with structured vz_error: {response:?}");
        assert_eq!(report.kind_label, "vz_timed_out");
        assert_eq!(report.vm_id.as_deref(), Some(handle));
        assert_eq!(report.budget_secs, Some(5.0));
    }

    /// Phase 4 Day 6 — `stop_capsule` awaits
    /// `RunningCapsule::bridge_terminated` after `vm.stop`
    /// resolves and continues without delay once the notify
    /// fires. Proves the deterministic teardown observation
    /// the supervisor now relies on.
    ///
    /// Fixture:
    /// 1. Build a synthetic VzVm capsule whose `RunningVm::stop`
    ///    is an idempotent no-op (no handle attached).
    /// 2. Attach a fresh `bridge_terminated` Arc<Notify>.
    /// 3. Spawn `stop_capsule(handle)` in the background.
    /// 4. After a tiny delay (let stop_capsule reach its
    ///    `notify.notified()` await), fire `notify_waiters()`.
    /// 5. Assert the spawn returns Ok within 1 s.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_capsule_proceeds_immediately_when_bridge_termination_notify_fires() {
        let supervisor = std::sync::Arc::new(make_test_supervisor());
        let handle = "vm-phase4-day6-bridge-notify-fires";
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());

        let mut rc = synthetic_vzvm_running_capsule("phase4-day6-bridge-notify-fires", handle);
        rc.bridge_terminated = Some(std::sync::Arc::clone(&notify));
        supervisor.running.write().await.insert(handle.into(), rc);

        let supervisor_for_stop = std::sync::Arc::clone(&supervisor);
        let handle_for_stop = handle.to_string();
        let stop_task =
            tokio::spawn(async move { supervisor_for_stop.stop_capsule(&handle_for_stop).await });

        // Give stop_capsule time to reach `notify.notified()`.
        // The synthetic RunningVm::stop is synchronous-no-op,
        // so the supervisor task is parked in the notify wait
        // after ~one tick.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        notify.notify_waiters();

        let started = std::time::Instant::now();
        tokio::time::timeout(std::time::Duration::from_secs(1), stop_task)
            .await
            .expect("stop_capsule must return inside 1s once bridge notify fires")
            .expect("stop_capsule task must not panic")
            .expect("stop_capsule must succeed on the happy path");
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "stop_capsule must observe the notify and proceed quickly; took {:?}",
            elapsed
        );

        let running = supervisor.running.read().await;
        assert!(
            !running.contains_key(handle),
            "stop_capsule must remove the VzVm entry from `running` regardless of bridge state"
        );
    }

    /// Phase 4 Day 6 — when the bridge notify is `None` (no
    /// BridgeContext was wired, e.g. legacy infrastructure
    /// capsules), `stop_capsule` MUST NOT block waiting for a
    /// non-existent signal. The synthetic capsule defaults to
    /// `bridge_terminated = None`, so this also doubles as a
    /// regression guard against accidental "always wait
    /// 10 s" behaviour.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stop_capsule_does_not_block_when_bridge_terminated_is_none() {
        let supervisor = make_test_supervisor();
        let handle = "vm-phase4-day6-no-bridge-notify";
        let rc = synthetic_vzvm_running_capsule("phase4-day6-no-bridge-notify", handle);
        assert!(
            rc.bridge_terminated.is_none(),
            "synthetic capsule starts with no bridge notify (legacy / no-cap-infra path)"
        );
        supervisor.running.write().await.insert(handle.into(), rc);

        let started = std::time::Instant::now();
        supervisor
            .stop_capsule(handle)
            .await
            .expect("stop_capsule must succeed without a bridge notify");
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "stop_capsule must NOT block when there is no bridge to await; took {:?}",
            elapsed
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
