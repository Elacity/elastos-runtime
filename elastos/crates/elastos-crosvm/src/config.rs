//! Configuration types for crosvm VMs

use std::fs;
use std::path::PathBuf;

use crate::network::NetworkConfig;
use elastos_common::CapsuleManifest;

/// Configuration for the crosvm provider
#[derive(Debug, Clone)]
pub struct CrosvmConfig {
    /// Path to the crosvm binary
    pub crosvm_bin: PathBuf,

    /// Path to the default kernel image
    pub kernel_path: PathBuf,

    /// Directory for VM control sockets
    pub socket_dir: PathBuf,

    /// Directory for rootfs overlays/snapshots
    pub rootfs_cache_dir: PathBuf,
}

impl CrosvmConfig {
    /// Create a new configuration with default paths
    pub fn new() -> Self {
        let data_dir = default_data_dir();
        Self {
            crosvm_bin: data_dir.join("bin/crosvm"),
            kernel_path: data_dir.join("bin/vmlinux"),
            socket_dir: data_dir.join("crosvm"),
            rootfs_cache_dir: data_dir.join("rootfs-cache"),
        }
    }

    /// Set the path to the crosvm binary
    pub fn with_crosvm_bin(mut self, path: impl Into<PathBuf>) -> Self {
        self.crosvm_bin = path.into();
        self
    }

    /// Set the path to the kernel image
    pub fn with_kernel_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.kernel_path = path.into();
        self
    }

    /// Set the directory for VM sockets
    pub fn with_socket_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.socket_dir = path.into();
        self
    }

    /// Set the directory for rootfs cache
    pub fn with_rootfs_cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.rootfs_cache_dir = path.into();
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.crosvm_bin.exists() {
            return Err(ConfigError::CrosvmNotFound(self.crosvm_bin.clone()));
        }

        if !self.kernel_path.exists() {
            return Err(ConfigError::KernelNotFound(self.kernel_path.clone()));
        }

        validate_guest_kernel(&self.kernel_path)?;

        Ok(())
    }
}

impl Default for CrosvmConfig {
    fn default() -> Self {
        Self::new()
    }
}

fn default_data_dir() -> PathBuf {
    // Prefer $HOME, fall back to passwd entry (survives setcap/AT_SECURE)
    let home = std::env::var_os("HOME").map(PathBuf::from).or_else(|| {
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
    #[cfg(target_arch = "x86_64")]
    if looks_like_x86_bzimage(bytes) {
        return true;
    }

    #[cfg(target_arch = "aarch64")]
    if looks_like_arm64_image(bytes) {
        return true;
    }

    false
}

fn default_console_device() -> &'static str {
    // crosvm uses 16550 UART (ttyS0) on both x86_64 and aarch64.
    // PL011 (ttyAMA0) is QEMU's virt machine, not crosvm.
    "ttyS0"
}

#[cfg(any(target_arch = "x86_64", test))]
fn looks_like_x86_bzimage(bytes: &[u8]) -> bool {
    bytes.len() > 0x206 && &bytes[0x202..0x206] == b"HdrS"
}

#[cfg(any(target_arch = "aarch64", test))]
fn looks_like_arm64_image(bytes: &[u8]) -> bool {
    bytes.len() > 0x44 && &bytes[0x38..0x3c] == b"ARMd" && &bytes[0x40..0x44] == b"PE\0\0"
}

/// Configuration for a single VM instance
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// VM identifier (derived from capsule ID)
    pub vm_id: String,

    /// Path to the kernel image
    pub kernel_path: PathBuf,

    /// Kernel boot arguments
    pub boot_args: String,

    /// Path to the rootfs image
    pub rootfs_path: PathBuf,

    /// Is the rootfs read-only
    pub rootfs_readonly: bool,

    /// Memory size in MiB
    pub mem_size_mib: u32,

    /// Number of vCPUs
    pub vcpu_count: u8,

    /// HTTP port to forward (if any)
    pub http_port: Option<u16>,

    /// Path to persistent data disk (attached as second drive)
    pub data_disk_path: Option<PathBuf>,

    /// Vsock context ID (CID) for this VM
    pub vsock_cid: u32,

    /// Optional Carrier-managed private control link for guest->runtime API access.
    pub network: Option<NetworkConfig>,

    /// Attach VM serial console to host stdio for interactive capsules.
    pub interactive_stdio: bool,

    /// Unix socket path for the microVM Carrier bridge (virtio-console).
    /// Guest writes to /dev/hvc0 (virtio-console), crosvm forwards to this socket, runtime reads it.
    pub carrier_socket_path: Option<PathBuf>,

    /// Empty directory used as crosvm sandbox pivot root.
    pub pivot_root_dir: Option<PathBuf>,
}

impl VmConfig {
    /// Create a VmConfig from a capsule manifest
    pub fn from_manifest(
        manifest: &CapsuleManifest,
        capsule_path: &std::path::Path,
        default_kernel: &std::path::Path,
    ) -> Self {
        let microvm = manifest.microvm.as_ref();

        // Determine kernel path
        let kernel_path = microvm
            .and_then(|m| m.kernel.as_ref())
            .map(|k| capsule_path.join(k))
            .unwrap_or_else(|| default_kernel.to_path_buf());

        // Get boot args from manifest (manifest default already includes init=/init)
        let default_boot_args = format!(
            "console={} reboot=k panic=1 init=/init",
            default_console_device()
        );
        let base_boot_args = microvm
            .map(|m| m.boot_args.clone())
            .unwrap_or(default_boot_args);

        // `pci=off` was a stale Firecracker-era boot arg. crosvm uses PCI-backed
        // virtio devices on x86, so disabling PCI prevents the guest from seeing
        // its root disk, NIC, and vsock devices.
        let base_boot_args = sanitize_crosvm_boot_args(&base_boot_args);

        // Ensure init=/init is present (critical for rootfs boot)
        let base_boot_args = if !base_boot_args.contains("init=") {
            format!("{} init=/init", base_boot_args)
        } else {
            base_boot_args
        };

        // Add random.trust_cpu=on for entropy (fixes Node.js hanging)
        let base_boot_args = if !base_boot_args.contains("random.trust_cpu") {
            format!("{} random.trust_cpu=on", base_boot_args)
        } else {
            base_boot_args
        };

        let vm_id = uuid::Uuid::new_v4().to_string();

        // Guest networking (TAP) is opt-in via permissions.guest_network.
        // Default: virtio-console Carrier bridge only (rootless, no CAP_NET_ADMIN).
        // Network is attached later by the supervisor if needed.

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
            vsock_cid: 3, // Default guest CID (2 is reserved for host)
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some(default_data_dir().join("crosvm-empty")),
        }
    }

    /// Build the crosvm command-line arguments for `crosvm run`.
    pub fn to_crosvm_args(&self) -> Vec<String> {
        let serial = if self.interactive_stdio {
            // crosvm doesn't support type=stdio. Use stdout+stdin for bidirectional serial.
            "type=stdout,hardware=serial,num=1,stdin"
        } else {
            "type=stdout,hardware=serial,num=1"
        };

        let mut args = vec![
            "--mem".into(),
            self.mem_size_mib.to_string(),
            "--cpus".into(),
            self.vcpu_count.to_string(),
            "--serial".into(),
            serial.into(),
        ];

        // Carrier bridge via virtio-console (bidirectional unix-stream).
        // Guest sees /dev/hvc0 (first virtio-console port).
        // 16550 UART serial was unreliable for host→guest delivery.
        if let Some(ref socket_path) = self.carrier_socket_path {
            args.push("--serial".into());
            args.push(format!(
                "type=unix-stream,path={},hardware=virtio-console,num=1,input-unix-stream",
                socket_path.to_string_lossy()
            ));
        }

        // Root disk (canonical crosvm syntax; avoids deprecated --rwroot/--root flags).
        args.push("--block".into());
        let mut root_block = format!("path={},root=true", self.rootfs_path.to_string_lossy());
        if self.rootfs_readonly {
            root_block.push_str(",ro=true");
        }
        args.push(root_block);

        // Additional data disk
        if let Some(ref data_path) = self.data_disk_path {
            args.push("--block".into());
            args.push(format!("path={}", data_path.to_string_lossy()));
        }

        // Optional guest-network TAP for compatibility capsules.
        // Ordinary app capsules use the virtio-console Carrier bridge instead.
        if let Some(ref network) = self.network {
            args.push("--net".into());
            args.push(format!(
                "tap-name={},mac={}",
                network.tap_name, network.guest_mac
            ));
        }

        if let Some(ref pivot_root) = self.pivot_root_dir {
            args.push("--pivot-root".into());
            args.push(pivot_root.to_string_lossy().into_owned());
        }

        // Kernel boot parameters
        if !self.boot_args.is_empty() {
            args.push("-p".into());
            args.push(self.boot_args.clone());
        }

        // Kernel image (positional, must be last)
        args.push(self.kernel_path.to_string_lossy().into_owned());

        args
    }

    /// Add session token and API address to boot args
    pub fn with_session(mut self, token: &str, api_addr: &str) -> Self {
        self.boot_args = format!(
            "{} elastos.token={} elastos.api={}",
            self.boot_args, token, api_addr
        );
        self
    }

    /// Attach explicit guest-network TAP.
    /// No iptables — the guest can reach the host runtime, not the internet.
    pub fn with_network(mut self, network: NetworkConfig) -> Self {
        let extra = format!(
            " elastos.guest_ip={} elastos.host_ip={} elastos.prefix_len={} elastos.net_iface=eth0",
            network.guest_ip, network.host_ip, network.prefix_len
        );
        self.boot_args = format!("{}{}", self.boot_args, extra);
        self.network = Some(network);
        self
    }
}

fn sanitize_crosvm_boot_args(args: &str) -> String {
    let sanitized: Vec<&str> = args
        .split_whitespace()
        .filter(|arg| *arg != "pci=off")
        .collect();
    sanitized.join(" ")
}

/// Errors that can occur during configuration
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("crosvm binary not found at: {0}")]
    CrosvmNotFound(PathBuf),

    #[error("Kernel image not found at: {0}")]
    KernelNotFound(PathBuf),

    #[error("failed to read kernel image at {0}: {1}")]
    KernelReadFailed(PathBuf, String),

    #[error("kernel image at {0} is incompatible with the current crosvm boot contract; missing markers: {1}. Install a guest kernel with ext4 + virtio_blk + virtio_pci built in")]
    KernelIncompatible(PathBuf, String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleType, MicroVmConfig, ResourceLimits};

    #[test]
    fn test_vm_config_from_manifest() {
        let manifest = CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "test-vm".into(),
            description: None,
            author: None,
            capsule_type: CapsuleType::MicroVM,
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits {
                memory_mb: 256,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            microvm: Some(MicroVmConfig {
                kernel: Some("kernel/vmlinux".into()),
                boot_args: "console=ttyS0".into(),
                http_port: Some(4100),
                vcpu_count: Some(2),
                rootfs_cid: None,
                kernel_cid: None,
                rootfs_size: None,
                persistent_storage_mb: None,
            }),
            providers: None,
            viewer: None,
            signature: None,
        };

        let capsule_path = std::path::Path::new("/capsules/test");
        let default_kernel = std::path::Path::new("/var/lib/elastos/vmlinux");

        let config = VmConfig::from_manifest(&manifest, capsule_path, default_kernel);

        assert_eq!(
            config.kernel_path,
            PathBuf::from("/capsules/test/kernel/vmlinux")
        );
        assert!(config
            .boot_args
            .starts_with(&format!("console={}", default_console_device())));
        assert!(!config.boot_args.contains("ip="));
        assert_eq!(
            config.rootfs_path,
            PathBuf::from("/capsules/test/rootfs.ext4")
        );
        assert_eq!(config.mem_size_mib, 256);
        assert_eq!(config.vcpu_count, 2);
        assert_eq!(config.http_port, Some(4100));
        assert!(config.network.is_none());
    }

    #[test]
    fn test_vm_config_strips_stale_pci_off() {
        let manifest = CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "test-vm".into(),
            description: None,
            author: None,
            capsule_type: CapsuleType::MicroVM,
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Default::default(),
            microvm: Some(MicroVmConfig {
                kernel: None,
                boot_args: "console=ttyS0 reboot=k panic=1 pci=off init=/init".into(),
                http_port: None,
                vcpu_count: None,
                rootfs_cid: None,
                kernel_cid: None,
                rootfs_size: None,
                persistent_storage_mb: None,
            }),
            providers: None,
            viewer: None,
            signature: None,
        };

        let config = VmConfig::from_manifest(
            &manifest,
            std::path::Path::new("/capsules/test"),
            std::path::Path::new("/default/vmlinux"),
        );

        assert!(!config.boot_args.contains("pci=off"));
        assert!(config.boot_args.contains("init=/init"));
    }

    #[test]
    fn test_crosvm_args_generation() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 512,
            vcpu_count: 2,
            http_port: Some(4100),
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let args = config.to_crosvm_args();

        assert!(args.contains(&"--mem".to_string()));
        assert!(args.contains(&"512".to_string()));
        assert!(args.contains(&"--cpus".to_string()));
        assert!(args.contains(&"2".to_string()));
        assert!(args.contains(&"--block".to_string()));
        assert!(args.contains(&"path=/path/to/rootfs.ext4,root=true".to_string()));
        assert!(args.contains(&"--pivot-root".to_string()));
        assert!(args.contains(&"/tmp/elastos/crosvm-empty".to_string()));
        // Kernel path is last
        assert_eq!(args.last().unwrap(), "/path/to/vmlinux");
    }

    #[test]
    fn test_crosvm_args_readonly_rootfs() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: true,
            mem_size_mib: 128,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 5,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let args = config.to_crosvm_args();
        assert!(args.contains(&"--block".to_string()));
        assert!(args.contains(&"path=/path/to/rootfs.ext4,root=true,ro=true".to_string()));
    }

    #[test]
    fn test_with_session() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 512,
            vcpu_count: 2,
            http_port: Some(4100),
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let config = config.with_session("test-token-123", "http://127.0.0.1:3000");

        assert!(config.boot_args.contains("elastos.token=test-token-123"));
        assert!(config
            .boot_args
            .contains("elastos.api=http://127.0.0.1:3000"));
        assert!(config
            .boot_args
            .contains(&format!("console={}", default_console_device())));
    }

    #[test]
    fn test_with_network() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 256,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 11,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let network = NetworkConfig::new("test-vm");
        let config = config.with_network(network.clone());
        let args = config.to_crosvm_args();

        assert!(config
            .boot_args
            .contains(&format!("elastos.guest_ip={}", network.guest_ip)));
        assert!(config
            .boot_args
            .contains(&format!("elastos.host_ip={}", network.host_ip)));
        assert!(args.contains(&"--net".to_string()));
        assert!(args.contains(&format!(
            "tap-name={},mac={}",
            network.tap_name, network.guest_mac
        )));
    }

    #[test]
    fn test_crosvm_args_with_data_disk() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 512,
            vcpu_count: 2,
            http_port: None,
            data_disk_path: Some("/path/to/data.ext4".into()),
            vsock_cid: 3,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let args = config.to_crosvm_args();
        assert!(args.contains(&"--block".to_string()));
        assert!(args.contains(&"path=/path/to/data.ext4".to_string()));
    }

    #[test]
    fn test_crosvm_args_interactive_serial_stdio() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 256,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 7,
            network: None,
            interactive_stdio: true,
            carrier_socket_path: None,
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let args = config.to_crosvm_args();
        assert!(args.contains(&"type=stdout,hardware=serial,num=1,stdin".to_string()));
    }

    #[test]
    fn test_crosvm_args_carrier_bridge_is_virtio_console() {
        let config = VmConfig {
            vm_id: "test-vm".into(),
            kernel_path: "/path/to/vmlinux".into(),
            boot_args: "console=ttyS0".into(),
            rootfs_path: "/path/to/rootfs.ext4".into(),
            rootfs_readonly: false,
            mem_size_mib: 256,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 7,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: Some("/tmp/carrier.sock".into()),
            pivot_root_dir: Some("/tmp/elastos/crosvm-empty".into()),
        };

        let args = config.to_crosvm_args();
        assert!(args.contains(
            &"type=unix-stream,path=/tmp/carrier.sock,hardware=virtio-console,num=1,input-unix-stream"
                .to_string()
        ));
    }

    #[test]
    fn test_validate_guest_kernel_accepts_required_markers() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"... ext4 ... virtio_blk ... virtio_pci ...").unwrap();

        validate_guest_kernel(tmp.path()).unwrap();
    }

    #[test]
    fn test_validate_guest_kernel_rejects_missing_virtio_pci() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"... ext4 ... virtio_blk ... virtio_mmio ...").unwrap();

        let err = validate_guest_kernel(tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::KernelIncompatible(_, _)));
        assert!(err.to_string().contains("virtio_pci"));
    }

    #[test]
    fn test_looks_like_x86_bzimage() {
        let mut bytes = vec![0u8; 0x220];
        bytes[0x202..0x206].copy_from_slice(b"HdrS");
        assert!(looks_like_x86_bzimage(&bytes));
    }

    #[test]
    fn test_looks_like_arm64_image() {
        let mut bytes = vec![0u8; 0x50];
        bytes[0x38..0x3c].copy_from_slice(b"ARMd");
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert!(looks_like_arm64_image(&bytes));
    }
}
