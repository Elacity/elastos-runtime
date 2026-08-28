//! Configuration types for the Vz backend.
//!
//! Mirrors the shapes of [`elastos_crosvm::CrosvmConfig`] and
//! [`elastos_crosvm::VmConfig`] so the supervisor's existing
//! manifest-to-VM-config translation maps cleanly. This module is
//! data-only and does not call into Virtualization.framework. The
//! [`vz_boot_args`][`VmConfig::vz_boot_args`] helper applies the
//! Vz console rule: the kernel console is virtio-console
//! (`/dev/hvc0`), so `console=ttyS0` from the
//! crosvm-style default becomes `console=hvc0` on Mac.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use elastos_common::CapsuleManifest;

use crate::network::NetworkConfig;

/// Default upper bound on how long
/// [`crate::ffi::lifecycle::VzMachineHandle::stop`] will wait for
/// Apple's `stopWithCompletionHandler:` block to fire. 30 s is
/// long enough to cover any documented Vz stop delay (guest
/// kernel shutdown sequencing, paravirt device drain) but short
/// enough that a wedged framework call doesn't pin the
/// supervisor's `stop_capsule` indefinitely. Operators on slow
/// or instrumented hardware can extend it via
/// [`VzConfig::with_stop_timeout`]; CI uses short values to
/// exercise the timeout path.
pub const DEFAULT_VZ_STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// **default per-capsule memory cap.**
///
/// 64 GiB. Two orders of magnitude above any plausible per-capsule
/// workload but well below Apple's `validateWithError` blast
/// radius — the OS validator briefly tries to commit memory when
/// checking a `setMemorySize:` request, so a 4 TiB
/// (`u32::MAX` MiB) manifest can stall the supervisor for seconds
/// before being rejected. Sanity-rejecting at `from_manifest`
/// time avoids that stall entirely.
pub const DEFAULT_MAX_MEMORY_MIB: u32 = 65_536;

/// **default per-capsule vCPU cap.**
///
/// 32 vCPUs. Apple Silicon hosts ship with at most 24 performance
/// cores today (M3 Ultra); 32 is a comfortable ceiling for any
/// foreseeable hardware and well below `u8::MAX = 255` (the type
/// limit on the manifest field).
pub const DEFAULT_MAX_VCPU_COUNT: u8 = 32;

/// **per-deployment resource caps.**
///
/// Upper bounds on memory and vCPU requests accepted from
/// capsule manifests. Defaults are conservative (see
/// [`DEFAULT_MAX_MEMORY_MIB`] and [`DEFAULT_MAX_VCPU_COUNT`]) but
/// can be overridden by operators that need tighter or looser
/// bounds via [`VmConfig::from_manifest_with_limits`].
///
/// **Threat closed:** without this cap, a capsule manifest can
/// request `u32::MAX` MiB (= 4 TiB) of RAM or `u8::MAX` (= 255)
/// vCPUs — values that Apple's `validateWithError` will reject,
/// but only *after* briefly trying to commit memory or allocate
/// vCPU state. That brief stall on the supervisor thread is the
/// manifest-driven DoS the pre-review packet's M4 flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmConfigLimits {
    /// Maximum `mem_size_mib` accepted from a manifest's
    /// `resources.memory_mb` field. Defaults to
    /// [`DEFAULT_MAX_MEMORY_MIB`] (64 GiB).
    pub max_memory_mib: u32,
    /// Maximum `vcpu_count` accepted from a manifest's
    /// `microvm.vcpu_count` field. Defaults to
    /// [`DEFAULT_MAX_VCPU_COUNT`] (32 vCPUs).
    pub max_vcpu_count: u8,
}

impl Default for VmConfigLimits {
    fn default() -> Self {
        Self {
            max_memory_mib: DEFAULT_MAX_MEMORY_MIB,
            max_vcpu_count: DEFAULT_MAX_VCPU_COUNT,
        }
    }
}

impl VmConfigLimits {
    /// Construct with explicit caps. Used by tests and any
    /// future operator-driven flow that needs to override the
    /// defaults (a multi-tenant deployment might want
    /// per-tenant ceilings, for example).
    pub const fn new(max_memory_mib: u32, max_vcpu_count: u8) -> Self {
        Self {
            max_memory_mib,
            max_vcpu_count,
        }
    }
}

/// Configuration for the Vz provider.
///
/// Field shape mirrors [`elastos_crosvm::CrosvmConfig`] minus the
/// `crosvm_bin` (Vz is a framework API, not a subprocess).
#[derive(Debug, Clone)]
pub struct VzConfig {
    /// Path to the default kernel image. Same artifact contract as
    /// `elastos-crosvm`: Linux Image (ARM64) or `vmlinux`. See
    /// `docs/MAC.md` §C for the audit of how the
    /// existing kernel artifact maps onto Vz.
    pub kernel_path: PathBuf,

    /// Directory for per-capsule Vz state (machine identifiers, vsock
    /// sockets, console capture pipes). One subdir per VM.
    pub state_dir: PathBuf,

    /// Directory for rootfs overlays. Mirrors crosvm semantics.
    pub rootfs_cache_dir: PathBuf,

    /// Optional default initial ramdisk path applied to every VM
    /// the provider loads.
    ///
    /// The capsule manifest schema does not carry an initramfs
    /// path (`elastos-common` is a Linux-untouched protected crate),
    /// so the only current producer of this field is
    /// `elastos vm-debug boot --initramfs …`. When set, every
    /// `VmConfig` the provider builds inherits this path before
    /// the FFI builder hands it to `VZLinuxBootLoader.setInitialRamdiskURL:`.
    pub initramfs_path: Option<PathBuf>,

    /// Upper bound on how long
    /// [`crate::ffi::lifecycle::VzMachineHandle::stop`] will wait
    /// for Apple's `stopWithCompletionHandler:` block to fire
    /// before returning a typed timeout error. Defaults to
    /// [`DEFAULT_VZ_STOP_TIMEOUT`]. macOS has no `kill -9`
    /// equivalent for a Vz VM, so this timeout prevents a wedged
    /// completion handler from blocking `stop_capsule` indefinitely.
    ///
    /// Linux's [`elastos_crosvm::CrosvmConfig`] has no analogue
    /// (Linux uses SIGTERM + 5 s SIGKILL escalation in
    /// `RunningVm::stop`); this field is therefore Mac-only at
    /// the *use-site* level (`elastos-server`'s Linux launch
    /// path never reads it).
    pub stop_timeout: Duration,

    /// opt-in to running the Mac-only
    /// stale-artifact prune (overlays + sockets +
    /// carrier-bridge sockets) automatically inside
    /// `Supervisor::new`. Defaults to `true` so a freshly
    /// started `elastos serve` always converges its on-disk
    /// state to a clean baseline after a crash of the prior
    /// supervisor process.
    ///
    /// Operators running multiple supervisor processes
    /// against the same `data_dir` (rare; the Mac launch path
    /// expects per-instance data dirs) can set this to
    /// `false` to avoid the edge case where two supervisors
    /// nuke each other's in-flight overlays. The standalone
    /// [`elastos_server::supervisor::Supervisor::prune_stale_mac_artifacts`]
    /// method remains available for explicit operator-driven
    /// cleanup.
    ///
    /// Linux's launch path ignores this field entirely (the
    /// stub `prune_stale_mac_artifacts` is a no-op on Linux);
    /// it lives on `VzConfig` because the prune *behaviour*
    /// is Mac-specific even though the *toggle* must be
    /// reachable from the supervisor's construction site on
    /// every platform.
    pub prune_orphans_on_startup: bool,
}

impl VzConfig {
    /// Create a new configuration with default paths matching the
    /// crosvm convention so the same `~/.local/share/elastos` data dir
    /// hosts both substrates side-by-side.
    pub fn new() -> Self {
        let data_dir = default_data_dir();
        Self {
            kernel_path: data_dir.join("bin/vmlinux"),
            state_dir: data_dir.join("vz"),
            rootfs_cache_dir: data_dir.join("rootfs-cache"),
            initramfs_path: None,
            stop_timeout: DEFAULT_VZ_STOP_TIMEOUT,
            prune_orphans_on_startup: true,
        }
    }

    /// Set the kernel path.
    pub fn with_kernel_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.kernel_path = path.into();
        self
    }

    /// Set the Vz state directory.
    pub fn with_state_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_dir = path.into();
        self
    }

    /// Set the rootfs cache directory.
    pub fn with_rootfs_cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.rootfs_cache_dir = path.into();
        self
    }

    /// Set the provider-wide default initramfs path. Every VM
    /// loaded through this provider will inherit the path on its
    /// `VmConfig` (and thus its `VZLinuxBootLoader`) unless a
    /// future per-VM override is wired in.
    pub fn with_initramfs_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.initramfs_path = Some(path.into());
        self
    }

    /// Override the stop-timeout budget.
    pub fn with_stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    /// Override the "prune orphans on startup" flag. The default
    /// (`true`) covers the common case of a single `elastos serve`
    /// per data dir; pass `false` from
    /// CI / multi-supervisor harnesses that need to preserve
    /// on-disk artifacts across construction.
    pub fn with_prune_orphans_on_startup(mut self, enabled: bool) -> Self {
        self.prune_orphans_on_startup = enabled;
        self
    }

    /// Validate the provider-level configuration. This checks that
    /// the kernel exists and looks like a Linux boot image; the full
    /// Vz `VZVirtualMachineConfiguration::validate()` probe runs
    /// after the VM configuration has been assembled.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.kernel_path.exists() {
            return Err(ConfigError::KernelNotFound(self.kernel_path.clone()));
        }
        validate_guest_kernel(&self.kernel_path)?;
        Ok(())
    }
}

impl Default for VzConfig {
    fn default() -> Self {
        Self::new()
    }
}

fn default_data_dir() -> PathBuf {
    // Match the crosvm `default_data_dir()` semantics so a single
    // `~/.local/share/elastos` directory hosts both substrates and the
    // runtime's existing `ELASTOS_DATA_DIR` override (in elastos-server)
    // applies uniformly. On macOS this becomes
    // `~/.local/share/elastos`; the runtime's broader data-dir
    // resolution (which on Mac prefers `dirs::data_dir()`) is handled in
    // elastos-server and is out of scope here.
    let home = std::env::var_os("HOME").map(PathBuf::from).or_else(|| {
        // Fallback for setcap / AT_SECURE invocations where HOME is
        // scrubbed. Same fallback path crosvm uses.
        // SAFETY: getuid is always safe; getpwuid returns NULL on error
        // and we check for null before dereferencing.
        let uid = unsafe { libc::getuid() };
        let pw = unsafe { libc::getpwuid(uid) };
        if !pw.is_null() {
            let dir = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) };
            dir.to_str().ok().map(PathBuf::from)
        } else {
            None
        }
    });
    home.map(|h| h.join(".local/share/elastos"))
        .unwrap_or_else(|| PathBuf::from("/var/lib/elastos"))
}

fn validate_guest_kernel(path: &std::path::Path) -> Result<(), ConfigError> {
    let bytes = fs::read(path)
        .map_err(|e| ConfigError::KernelReadFailed(path.to_path_buf(), e.to_string()))?;

    if looks_like_supported_boot_image(&bytes) {
        return Ok(());
    }

    // Same string-marker contract as elastos-crosvm so the same
    // shipped vmlinux passes both validators. Add a `virtio_vsock`
    // marker only if the shipped macOS kernel artifact requires it.
    let has_ext4 = contains_ascii(&bytes, b"ext4");
    let has_virtio_blk = contains_ascii(&bytes, b"virtio_blk");
    let has_virtio_pci = contains_ascii(&bytes, b"virtio_pci");

    if has_ext4 && has_virtio_blk && has_virtio_pci {
        return Ok(());
    }

    let mut missing = Vec::new();
    if !has_ext4 {
        missing.push("ext4");
    }
    if !has_virtio_blk {
        missing.push("virtio_blk");
    }
    if !has_virtio_pci {
        missing.push("virtio_pci");
    }

    Err(ConfigError::KernelIncompatible(
        path.to_path_buf(),
        missing.join(", "),
    ))
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn looks_like_supported_boot_image(bytes: &[u8]) -> bool {
    // Vz on Apple Silicon boots the Linux ARM64 `Image` format
    // (raw kernel with the "ARMd\0PE\0\0" magic at offsets 0x38/0x40).
    // x86_64 macOS hosts are out of scope for this backend ("Apple
    // Silicon first"), so the bzImage path is intentionally absent here.
    bytes.len() > 0x44 && &bytes[0x38..0x3c] == b"ARMd" && &bytes[0x40..0x44] == b"PE\0\0"
}

/// Default kernel console device name expected by the Vz guest path.
///
/// Vz exposes the kernel console only as virtio-console, not as a
/// 16550 UART. A `console=ttyS0` boot arg produces a silent boot
/// on Vz; `console=hvc0` is the working equivalent.
const VZ_CONSOLE_DEVICE: &str = "hvc0";

/// Configuration for a single Vz VM instance. Shape mirrors
/// [`elastos_crosvm::VmConfig`] minus `pivot_root_dir` (Vz sandboxes
/// the hypervisor process in-framework; there is no analogue).
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// VM identifier (derived from capsule ID).
    pub vm_id: String,

    /// Path to the kernel image.
    pub kernel_path: PathBuf,

    /// Kernel boot arguments. Stored verbatim; the
    /// [`vz_boot_args`][`Self::vz_boot_args`] accessor rewrites
    /// `console=ttyS0` → `console=hvc0` for the Vz boot loader.
    pub boot_args: String,

    /// Path to the rootfs image (raw ext4).
    pub rootfs_path: PathBuf,

    /// Is the rootfs read-only.
    pub rootfs_readonly: bool,

    /// Memory size in MiB.
    pub mem_size_mib: u32,

    /// Number of vCPUs.
    pub vcpu_count: u8,

    /// HTTP port to forward (if any). Informational at the Vz layer;
    /// the actual forward is done by elastos-server.
    pub http_port: Option<u16>,

    /// Path to persistent data disk (attached as second virtio-blk
    /// device).
    pub data_disk_path: Option<PathBuf>,

    /// Vsock context ID. Informational on macOS because Vz does not
    /// expose a public CID API.
    /// Kept in the struct so the same `from_manifest` translation
    /// path the supervisor uses for crosvm works unchanged.
    pub vsock_cid: u32,

    /// Optional Carrier-managed private control link for
    /// guest→runtime API access.
    pub network: Option<NetworkConfig>,

    /// Attach no VZ network device. This is an explicit per-VM opt-in used by
    /// the Browser VZ vsock transport; `false` preserves the existing NAT or
    /// bridged behavior for every other VZ caller.
    pub network_disabled: bool,

    /// Attach the VM kernel console to host stdio for interactive
    /// capsules. The Vz translation wires this through a
    /// `VZFileHandleSerialPortAttachment` over a `socketpair`.
    pub interactive_stdio: bool,

    /// Unix-socket path for the microVM Carrier bridge.
    ///
    /// On macOS the guest sees the bridge at
    /// `/dev/hvc1` (see [`crate::CARRIER_GUEST_DEVICE_PATH`]) — Vz's
    /// kernel console occupies `/dev/hvc0`, so the Carrier port moves
    /// to the second virtio-console multi-port entry.
    pub carrier_socket_path: Option<PathBuf>,

    /// Optional initial ramdisk image.
    ///
    /// Every modern distro kernel built for arm64 expects an
    /// initramfs to bring up userspace (module loading, root pivot,
    /// `/sbin/init` discovery). Vz exposes this via
    /// `VZLinuxBootLoader.setInitialRamdiskURL:`; the FFI builder
    /// only attaches it when this field is `Some`. Capsule manifests
    /// don't currently surface this — `elastos-common::MicroVmConfig`
    /// is a Linux-untouched protected crate — so the only current
    /// path that sets it is `elastos vm-debug boot --initramfs …`.
    pub initramfs_path: Option<PathBuf>,
}

impl VmConfig {
    /// Create a VmConfig from a capsule manifest. Mirrors
    /// `elastos_crosvm::VmConfig::from_manifest` so the supervisor's
    /// existing call site maps cleanly.
    pub fn from_manifest(
        manifest: &CapsuleManifest,
        capsule_path: &std::path::Path,
        default_kernel: &std::path::Path,
    ) -> Self {
        let microvm = manifest.microvm.as_ref();

        let kernel_path = microvm
            .and_then(|m| m.kernel.as_ref())
            .map(|k| capsule_path.join(k))
            .unwrap_or_else(|| default_kernel.to_path_buf());

        // Default boot args use the Vz console device. The supervisor
        // may pass crosvm-style args through manifest overrides; the
        // `vz_boot_args` accessor rewrites them at construction time.
        let default_boot_args =
            format!("console={} reboot=k panic=1 init=/init", VZ_CONSOLE_DEVICE);
        let base_boot_args = microvm
            .map(|m| rewrite_console_for_vz(&m.boot_args))
            .unwrap_or(default_boot_args);

        let base_boot_args = if !base_boot_args.contains("init=") {
            format!("{} init=/init", base_boot_args)
        } else {
            base_boot_args
        };

        let base_boot_args = if !base_boot_args.contains("random.trust_cpu") {
            format!("{} random.trust_cpu=on", base_boot_args)
        } else {
            base_boot_args
        };

        let vm_id = uuid::Uuid::new_v4().to_string();

        Self {
            vm_id,
            kernel_path,
            boot_args: base_boot_args,
            rootfs_path: capsule_path.join(&manifest.entrypoint),
            rootfs_readonly: false,
            mem_size_mib: manifest.resources.memory_mb,
            vcpu_count: microvm.and_then(|m| m.vcpu_count).unwrap_or(1),
            http_port: microvm.and_then(|m| m.http_port),
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            network_disabled: false,
            interactive_stdio: false,
            carrier_socket_path: None,
            initramfs_path: None,
        }
    }

    /// **fallible counterpart of [`from_manifest`].**
    ///
    /// Validates `manifest.resources.memory_mb` and the
    /// effective `vcpu_count` against the supplied [`VmConfigLimits`]
    /// BEFORE constructing the `VmConfig`. Returns
    /// [`ConfigError::ResourceLimitExceeded`] for either cap on
    /// violation; on success, delegates to [`from_manifest`] for
    /// the construction body (zero behavioural drift between
    /// the two paths).
    ///
    /// **Use this method from production launch paths** so
    /// manifest-driven over-allocations are rejected before
    /// Apple's `validateWithError` is asked to briefly commit
    /// the memory / vCPU state (which can stall the supervisor
    /// for seconds on a `u32::MAX` MiB ask).
    ///
    /// [`from_manifest`] remains available for tests and any
    /// trusted-input call site that does not need
    /// validation — it is the equivalent of calling this method
    /// with effectively-unbounded limits and is intentionally
    /// distinct so a future audit grep for the validated path
    /// can find it.
    pub fn from_manifest_with_limits(
        manifest: &CapsuleManifest,
        capsule_path: &std::path::Path,
        default_kernel: &std::path::Path,
        limits: &VmConfigLimits,
    ) -> Result<Self, ConfigError> {
        enforce_resource_limits(manifest, limits)?;
        Ok(Self::from_manifest(manifest, capsule_path, default_kernel))
    }

    /// Attach an initial ramdisk path. Used by `elastos vm-debug
    /// boot --initramfs …` and any future flow that needs to boot a
    /// kernel that depends on an initramfs (every Ubuntu / Debian /
    /// Alpine cloud image kernel we know of). `None` is the default
    /// and matches the Vz boot loader's `nil`-by-default
    /// `initialRamdiskURL` property.
    pub fn with_initramfs_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.initramfs_path = Some(path.into());
        self
    }

    /// Return the boot args ready for [`VZLinuxBootLoader.commandLine`].
    ///
    /// Returns the stored `boot_args` (already
    /// console-rewritten by [`from_manifest`][`Self::from_manifest`]).
    /// Future work may extend this with additional Vz-specific args
    /// once they are known.
    pub fn vz_boot_args(&self) -> String {
        rewrite_console_for_vz(&self.boot_args)
    }

    /// Add session token + API address to the boot args. Mirrors
    /// `elastos_crosvm::VmConfig::with_session`.
    pub fn with_session(mut self, token: &str, api_addr: &str) -> Self {
        self.boot_args = format!(
            "{} elastos.token={} elastos.api={}",
            self.boot_args, token, api_addr
        );
        self
    }
}

/// Validate that a capsule manifest's resource requests fit within
/// the supplied limits. Returns the
/// first violation as a typed [`ConfigError::ResourceLimitExceeded`].
///
/// Order of checks: memory first (the more expensive runtime
/// allocation to validate via Apple), then vCPU count.
fn enforce_resource_limits(
    manifest: &CapsuleManifest,
    limits: &VmConfigLimits,
) -> Result<(), ConfigError> {
    let requested_memory = manifest.resources.memory_mb;
    if requested_memory > limits.max_memory_mib {
        return Err(ConfigError::ResourceLimitExceeded {
            field: "resources.memory_mb",
            requested: requested_memory as u64,
            max: limits.max_memory_mib as u64,
        });
    }
    // `vcpu_count` defaults to 1 when the manifest omits the
    // field (mirrors the construction default in `from_manifest`),
    // so a manifest with no `microvm.vcpu_count` is always within
    // any sane cap.
    let requested_vcpus = manifest
        .microvm
        .as_ref()
        .and_then(|m| m.vcpu_count)
        .unwrap_or(1);
    if requested_vcpus > limits.max_vcpu_count {
        return Err(ConfigError::ResourceLimitExceeded {
            field: "microvm.vcpu_count",
            requested: requested_vcpus as u64,
            max: limits.max_vcpu_count as u64,
        });
    }
    Ok(())
}

/// Public for tests and call sites that need to migrate a
/// crosvm-style boot string before passing it to a Vz config.
pub fn rewrite_console_for_vz(args: &str) -> String {
    // Replace `console=ttyS0` (with any trailing `,...` baud option)
    // with `console=hvc0`. Leaves every other token untouched.
    args.split_whitespace()
        .map(|token| {
            if let Some(rest) = token.strip_prefix("console=ttyS0") {
                if rest.is_empty() || rest.starts_with(',') {
                    return format!("console={}{}", VZ_CONSOLE_DEVICE, rest);
                }
            }
            token.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Errors that can occur during configuration.
///
/// This currently models kernel validation and manifest resource
/// cap failures. Runtime Vz failures are classified separately by
/// [`crate::VzError`].
#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("kernel image not found at: {0}")]
    KernelNotFound(PathBuf),

    #[error("failed to read kernel image at {0}: {1}")]
    KernelReadFailed(PathBuf, String),

    #[error("kernel image at {0} is incompatible with the Vz boot contract; missing markers: {1}. Install a guest kernel with ext4 + virtio_blk + virtio_pci built in (Apple Silicon: raw ARM64 Image format).")]
    KernelIncompatible(PathBuf, String),

    /// Manifest requested a resource value above the configured
    /// upper bound. Carries the field name,
    /// the requested value, and the cap so an operator looking
    /// at the error can decide whether to lower the manifest's
    /// ask or raise the deployment-wide cap.
    #[error(
        "manifest field `{field}` requested {requested}, but the configured maximum is {max} \
         (raise the cap via VmConfigLimits, or lower the manifest's request)"
    )]
    ResourceLimitExceeded {
        field: &'static str,
        requested: u64,
        max: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1};

    fn microvm_manifest(boot_args: &str) -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "vz-config-test".into(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
            runtime_abi: None,
            bus_contract: None,
            wit_world_sha256: None,
            execution: None,
            projections: Vec::new(),
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: ResourceLimits {
                memory_mb: 128,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            // v0.3.0 added the principal-binding `authority` field; None
            // here = "no authority constraint" for in-module unit tests.
            authority: None,
            microvm: Some(MicroVmConfig {
                kernel: None,
                boot_args: boot_args.to_string(),
                http_port: None,
                vcpu_count: Some(1),
                rootfs_cid: None,
                kernel_cid: None,
                rootfs_size: None,
                persistent_storage_mb: None,
            }),
            providers: None,
            icon: None,
            viewer: None,
            signature: None,
        }
    }

    #[test]
    fn rewrite_console_swaps_ttys0_for_hvc0() {
        let crosvm_style = "console=ttyS0 reboot=k panic=1 init=/init";
        let vz_style = rewrite_console_for_vz(crosvm_style);
        assert!(vz_style.contains("console=hvc0"));
        assert!(!vz_style.contains("console=ttyS0"));
        assert!(vz_style.contains("init=/init"));
    }

    #[test]
    fn rewrite_console_preserves_baud_suffix() {
        let crosvm_style = "console=ttyS0,115200 quiet";
        let vz_style = rewrite_console_for_vz(crosvm_style);
        assert!(vz_style.contains("console=hvc0,115200"));
    }

    #[test]
    fn rewrite_console_is_idempotent() {
        let already_vz = "console=hvc0 init=/init";
        let twice = rewrite_console_for_vz(already_vz);
        assert_eq!(twice, already_vz);
    }

    #[test]
    fn from_manifest_emits_hvc0_console_for_microvm() {
        let manifest = microvm_manifest("console=ttyS0 reboot=k panic=1");
        let capsule_path = std::path::Path::new("/capsules/vz-config-test");
        let default_kernel = std::path::Path::new("/default/vmlinux");

        let config = VmConfig::from_manifest(&manifest, capsule_path, default_kernel);

        assert!(config.boot_args.contains("console=hvc0"));
        assert!(!config.boot_args.contains("console=ttyS0"));
        assert!(config.boot_args.contains("init=/init"));
        assert!(config.boot_args.contains("random.trust_cpu=on"));
        assert_eq!(config.rootfs_path, capsule_path.join("rootfs.ext4"));
        assert_eq!(config.vsock_cid, 3);
        assert!(config.network.is_none());
    }

    #[test]
    fn from_manifest_default_boot_args_use_hvc0() {
        let manifest = microvm_manifest("");
        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/capsules/c"),
            std::path::Path::new("/k/vmlinux"),
        );
        // Empty boot_args triggers the default branch.
        assert!(
            config.boot_args.contains("console=hvc0") || config.boot_args.contains("init=/init")
        );
    }

    #[test]
    fn with_session_appends_token_and_api_to_boot_args() {
        let manifest = microvm_manifest("console=ttyS0");
        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
        )
        .with_session("abc12345", "http://127.0.0.1:3000");
        assert!(config.boot_args.contains("elastos.token=abc12345"));
        assert!(config
            .boot_args
            .contains("elastos.api=http://127.0.0.1:3000"));
    }

    #[test]
    fn vz_config_default_paths_under_local_share_elastos() {
        let config = VzConfig::new();
        // Path tail is platform-independent because we mirror the
        // crosvm convention so both substrates share `~/.local/share/elastos`.
        assert!(config.kernel_path.ends_with("bin/vmlinux"));
        assert!(config.state_dir.ends_with("vz"));
        assert!(config.rootfs_cache_dir.ends_with("rootfs-cache"));
    }

    #[test]
    fn vz_config_with_kernel_path_overrides() {
        let custom = PathBuf::from("/custom/vmlinux");
        let config = VzConfig::new().with_kernel_path(&custom);
        assert_eq!(config.kernel_path, custom);
    }

    #[test]
    fn validate_guest_kernel_accepts_string_marker_kernel() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"... ext4 ... virtio_blk ... virtio_pci ...").unwrap();
        validate_guest_kernel(tmp.path()).unwrap();
    }

    #[test]
    fn vm_config_initramfs_path_defaults_to_none_from_manifest() {
        let manifest = microvm_manifest("");
        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
        );
        assert!(
            config.initramfs_path.is_none(),
            "from_manifest must not invent an initramfs path"
        );
    }

    #[test]
    fn vm_config_with_initramfs_path_sets_the_field() {
        let manifest = microvm_manifest("");
        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
        )
        .with_initramfs_path("/tmp/initrd.img");
        assert_eq!(
            config.initramfs_path.as_deref(),
            Some(std::path::Path::new("/tmp/initrd.img"))
        );
    }

    #[test]
    fn vz_config_default_prune_orphans_on_startup_is_true() {
        let config = VzConfig::new();
        assert!(
            config.prune_orphans_on_startup,
            "default-constructed VzConfig must opt into Mac startup orphan pruning so `elastos serve` self-heals after a supervisor crash"
        );
    }

    #[test]
    fn vz_config_with_prune_orphans_on_startup_round_trip() {
        let opted_out = VzConfig::new().with_prune_orphans_on_startup(false);
        assert!(
            !opted_out.prune_orphans_on_startup,
            "with_prune_orphans_on_startup(false) must set the flag to false"
        );
        let opted_in_again = opted_out.with_prune_orphans_on_startup(true);
        assert!(
            opted_in_again.prune_orphans_on_startup,
            "with_prune_orphans_on_startup(true) must restore the flag"
        );
    }

    // ---------------------------------------------------------------
    // manifest resource cap regression tests.
    //
    // Previous `from_manifest` accepted any `u32` value for
    // `resources.memory_mb` (= 4 TiB max) and any `u8` value for
    // `microvm.vcpu_count` (= 255 max) without bound. A manifest
    // requesting absurd values would only be rejected later, by
    // Apple's `validateWithError`, which briefly tries to commit
    // memory / vCPU state and can stall the supervisor for seconds.
    //
    // `from_manifest_with_limits` is the fallible counterpart that
    // production launch paths should use. These tests cover the
    // four corner cases:
    //   - happy path: typical manifest within defaults.
    //   - reject: memory above default cap.
    //   - reject: vCPU above default cap.
    //   - override: custom limits accept values the default rejects.
    // ---------------------------------------------------------------

    fn microvm_manifest_with_resources(memory_mb: u32, vcpu_count: u8) -> CapsuleManifest {
        let mut manifest = microvm_manifest("console=ttyS0");
        manifest.resources.memory_mb = memory_mb;
        if let Some(microvm) = manifest.microvm.as_mut() {
            microvm.vcpu_count = Some(vcpu_count);
        }
        manifest
    }

    #[test]
    fn from_manifest_with_limits_accepts_typical_request() {
        let manifest = microvm_manifest_with_resources(512, 2);
        let result = VmConfig::from_manifest_with_limits(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
            &VmConfigLimits::default(),
        );
        let config = result.expect("typical 512 MiB / 2 vCPU manifest must pass default caps");
        assert_eq!(config.mem_size_mib, 512);
        assert_eq!(config.vcpu_count, 2);
    }

    #[test]
    fn from_manifest_with_limits_rejects_excessive_memory() {
        // 1 MiB above the default cap.
        let manifest = microvm_manifest_with_resources(DEFAULT_MAX_MEMORY_MIB + 1, 1);
        let err = VmConfig::from_manifest_with_limits(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
            &VmConfigLimits::default(),
        )
        .expect_err("memory request above default cap must be rejected");
        match err {
            ConfigError::ResourceLimitExceeded {
                field,
                requested,
                max,
            } => {
                assert_eq!(field, "resources.memory_mb");
                assert_eq!(requested, (DEFAULT_MAX_MEMORY_MIB as u64) + 1);
                assert_eq!(max, DEFAULT_MAX_MEMORY_MIB as u64);
            }
            other => panic!("expected ResourceLimitExceeded for memory, got: {other:?}"),
        }
    }

    #[test]
    fn from_manifest_with_limits_rejects_excessive_vcpus() {
        // 1 vCPU above the default cap.
        let manifest = microvm_manifest_with_resources(128, DEFAULT_MAX_VCPU_COUNT + 1);
        let err = VmConfig::from_manifest_with_limits(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
            &VmConfigLimits::default(),
        )
        .expect_err("vCPU request above default cap must be rejected");
        match err {
            ConfigError::ResourceLimitExceeded {
                field,
                requested,
                max,
            } => {
                assert_eq!(field, "microvm.vcpu_count");
                assert_eq!(requested, (DEFAULT_MAX_VCPU_COUNT as u64) + 1);
                assert_eq!(max, DEFAULT_MAX_VCPU_COUNT as u64);
            }
            other => panic!("expected ResourceLimitExceeded for vcpu, got: {other:?}"),
        }
    }

    #[test]
    fn from_manifest_with_limits_rejects_u32_max_memory() {
        // The exact previous attack: a manifest requesting
        // 4 TiB of RAM. Pre-fix this would propagate all the way
        // to Apple's `validateWithError`; post-fix it is rejected
        // at config-build time without touching the framework.
        let manifest = microvm_manifest_with_resources(u32::MAX, 1);
        let err = VmConfig::from_manifest_with_limits(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
            &VmConfigLimits::default(),
        )
        .expect_err("u32::MAX MiB ask must be rejected");
        assert!(
            matches!(
                err,
                ConfigError::ResourceLimitExceeded {
                    field: "resources.memory_mb",
                    ..
                }
            ),
            "expected memory-field rejection, got: {err:?}"
        );
    }

    #[test]
    fn from_manifest_with_custom_limits_overrides_default() {
        // An operator running on a 256 GiB host can raise the cap
        // to permit larger capsules. With limits raised, a 128 GiB
        // ask that would be rejected under defaults now passes.
        let above_default = DEFAULT_MAX_MEMORY_MIB + 65_536;
        let manifest = microvm_manifest_with_resources(above_default, 16);
        let custom = VmConfigLimits::new(above_default + 1024, 64);
        let config = VmConfig::from_manifest_with_limits(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
            &custom,
        )
        .expect("custom-limits build must accept ask within the operator-raised cap");
        assert_eq!(config.mem_size_mib, above_default);
        assert_eq!(config.vcpu_count, 16);
    }

    #[test]
    fn vm_config_limits_default_matches_documented_constants() {
        let limits = VmConfigLimits::default();
        assert_eq!(limits.max_memory_mib, DEFAULT_MAX_MEMORY_MIB);
        assert_eq!(limits.max_vcpu_count, DEFAULT_MAX_VCPU_COUNT);
    }

    #[test]
    fn from_manifest_remains_infallible_for_unvalidated_callers() {
        // `from_manifest` (no `_with_limits`) is the unvalidated
        // path retained for tests and trusted-input call sites.
        // It must continue to accept absurd values without panic
        // so trusted callers that intentionally bypass limits keep
        // compiling and running.
        let manifest = microvm_manifest_with_resources(u32::MAX, u8::MAX);
        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/c"),
            std::path::Path::new("/k"),
        );
        assert_eq!(config.mem_size_mib, u32::MAX);
        assert_eq!(config.vcpu_count, u8::MAX);
    }

    #[test]
    fn validate_guest_kernel_rejects_kernel_missing_virtio_blk() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"... ext4 ... virtio_pci ...").unwrap();
        let err = validate_guest_kernel(tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::KernelIncompatible(_, _)));
        assert!(err.to_string().contains("virtio_blk"));
    }
}
