//! ElastOS crosvm Compute Provider
//!
//! Runs capsules in crosvm VMs with hardware-level isolation.
//! crosvm is the sole VM backend — no fallback, no feature gating.
//!
//! # Requirements
//!
//! - Linux with KVM support (`/dev/kvm`)
//! - crosvm binary
//! - Linux kernel image (vmlinux, 5.10+)
//!
//! # Example
//!
//! ```ignore
//! use elastos_crosvm::{CrosvmProvider, CrosvmConfig};
//!
//! let config = CrosvmConfig::new()
//!     .with_crosvm_bin("/home/alice/.local/share/elastos/bin/crosvm")
//!     .with_kernel_path("/home/alice/.local/share/elastos/bin/vmlinux");
//!
//! let provider = CrosvmProvider::new(config)?;
//! ```

mod config;
#[cfg(target_os = "linux")]
mod network;
#[cfg(not(target_os = "linux"))]
#[path = "network_stub.rs"]
mod network;
mod provider;
mod proxy;
mod rootfs;
mod vm;

pub use config::{CrosvmConfig, VmConfig};
pub use network::NetworkConfig;
pub use provider::CrosvmProvider;
pub use proxy::TcpProxy;
pub use vm::RunningVm;

/// Check process access to the supported KVM API before admitting a local VM.
/// Binary, image, capacity and VM-specific capabilities are checked at launch.
pub fn is_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        kvm_api_available(std::path::Path::new("/dev/kvm"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn kvm_api_available(path: &std::path::Path) -> bool {
    use std::os::fd::AsRawFd;

    let Ok(device) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    // Linux UAPI: _IO(KVMIO, 0x00), where KVMIO is 0xAE. This read-only ioctl
    // takes no third argument and returns the API version; it creates no VM.
    // Version 12 is the stable API required by KVM userspace implementations.
    unsafe { libc::ioctl(device.as_raw_fd(), 0xAE00 as libc::c_ulong) == 12 }
}

#[cfg(test)]
mod host_support_tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn a_path_without_the_kvm_api_does_not_admit_a_vm() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!super::kvm_api_available(&dir.path().join("absent")));
        let file = dir.path().join("ordinary-file");
        std::fs::write(&file, b"not a KVM device").unwrap();
        assert!(!super::kvm_api_available(&file));
        assert_eq!(std::fs::read(&file).unwrap(), b"not a KVM device");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_hosts_do_not_advertise_kvm() {
        assert!(!super::is_supported());
    }
}
