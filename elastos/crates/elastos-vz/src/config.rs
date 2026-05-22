//! Configuration types for the Vz backend.
//!
//! Mirrors the shapes of [`elastos_crosvm::CrosvmConfig`] and
//! [`elastos_crosvm::VmConfig`] so the supervisor's existing
//! manifest-to-VM-config translation maps cleanly. Phase 1 is
//! data-only — no Vz API calls. The
//! [`vz_boot_args`][`VmConfig::vz_boot_args`] helper applies the
//! Phase 0 finding that Vz exposes the kernel console only as
//! virtio-console (`/dev/hvc0`), so `console=ttyS0` from the
//! crosvm-style default becomes `console=hvc0` on Mac.

use std::fs;
use std::path::PathBuf;

use elastos_common::CapsuleManifest;

use crate::network::NetworkConfig;

/// Configuration for the Vz provider.
///
/// Field shape mirrors [`elastos_crosvm::CrosvmConfig`] minus the
/// `crosvm_bin` (Vz is a framework API, not a subprocess).
#[derive(Debug, Clone)]
pub struct VzConfig {
    /// Path to the default kernel image. Same artifact contract as
    /// `elastos-crosvm`: Linux Image (ARM64) or `vmlinux`. See
    /// `docs/vz-backend/PHASE_0_SCOPE.md` §C for the audit of how the
    /// existing kernel artifact maps onto Vz.
    pub kernel_path: PathBuf,

    /// Directory for per-capsule Vz state (machine identifiers, vsock
    /// sockets, console capture pipes). One subdir per VM.
    pub state_dir: PathBuf,

    /// Directory for rootfs overlays. Mirrors crosvm semantics.
    pub rootfs_cache_dir: PathBuf,
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

    /// Validate the configuration. Phase 1 only checks that the
    /// kernel file exists and looks like a Linux boot image; the full
    /// Vz `VZVirtualMachineConfiguration::validate()` probe is Phase 2.
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
    // shipped vmlinux passes both validators. Phase 6 may add a
    // `virtio_vsock` marker once the macOS kernel artifact decision
    // (`docs/vz-backend/PHASE_0_SCOPE.md` §C.3) is made.
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
    // x86_64 macOS hosts are out of scope per
    // `docs/vz-backend/PLAN.md` Phase 6 ("Apple Silicon first") so the
    // bzImage path is intentionally absent here.
    bytes.len() > 0x44 && &bytes[0x38..0x3c] == b"ARMd" && &bytes[0x40..0x44] == b"PE\0\0"
}

/// Default kernel console device name expected by the Vz guest path.
///
/// Phase 0 §D pitfall #3: Vz exposes the kernel console only as
/// virtio-console, not as a 16550 UART. A `console=ttyS0` boot arg
/// produces a silent boot on Vz; `console=hvc0` is the working
/// equivalent.
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
    /// expose a public CID API (`PHASE_0_SCOPE.md` §B / §D pitfall #5).
    /// Kept in the struct so the same `from_manifest` translation
    /// path the supervisor uses for crosvm works unchanged.
    pub vsock_cid: u32,

    /// Optional Carrier-managed private control link for
    /// guest→runtime API access.
    pub network: Option<NetworkConfig>,

    /// Attach the VM kernel console to host stdio for interactive
    /// capsules. The Vz translation wires this through a
    /// `VZFileHandleSerialPortAttachment` over a `socketpair` in
    /// Phase 2.
    pub interactive_stdio: bool,

    /// Unix-socket path for the microVM Carrier bridge.
    ///
    /// On macOS the guest sees the bridge at
    /// `/dev/hvc1` (see [`crate::CARRIER_GUEST_DEVICE_PATH`]) — Vz's
    /// kernel console occupies `/dev/hvc0`, so the Carrier port moves
    /// to the second virtio-console multi-port entry.
    pub carrier_socket_path: Option<PathBuf>,
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
        let default_boot_args = format!(
            "console={} reboot=k panic=1 init=/init",
            VZ_CONSOLE_DEVICE
        );
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
            interactive_stdio: false,
            carrier_socket_path: None,
        }
    }

    /// Return the boot args ready for [`VZLinuxBootLoader.commandLine`].
    ///
    /// Phase 1 returns the stored `boot_args` (already
    /// console-rewritten by [`from_manifest`][`Self::from_manifest`]).
    /// Phase 2 may extend with Vz-specific args once they are known.
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
/// Phase 1 only models kernel-related failures because that is the
/// only thing `VzConfig::validate()` checks. Phase 2 adds variants
/// for missing entitlements, unsupported macOS version, etc., at
/// which point the `Kernel*` prefix will share the namespace with
/// non-kernel variants.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("kernel image not found at: {0}")]
    KernelNotFound(PathBuf),

    #[error("failed to read kernel image at {0}: {1}")]
    KernelReadFailed(PathBuf, String),

    #[error("kernel image at {0} is incompatible with the Vz boot contract; missing markers: {1}. Install a guest kernel with ext4 + virtio_blk + virtio_pci built in (Apple Silicon: raw ARM64 Image format).")]
    KernelIncompatible(PathBuf, String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1};

    fn microvm_manifest(boot_args: &str) -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "phase1-test".into(),
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
                boot_args: boot_args.to_string(),
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
        let capsule_path = std::path::Path::new("/capsules/phase1-test");
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
        assert!(config.boot_args.contains("console=hvc0") || config.boot_args.contains("init=/init"));
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
        assert!(config.boot_args.contains("elastos.api=http://127.0.0.1:3000"));
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
    fn validate_guest_kernel_rejects_kernel_missing_virtio_blk() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"... ext4 ... virtio_pci ...").unwrap();
        let err = validate_guest_kernel(tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::KernelIncompatible(_, _)));
        assert!(err.to_string().contains("virtio_blk"));
    }
}
