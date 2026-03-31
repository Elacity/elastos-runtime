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

/// Check if the system supports crosvm (has KVM).
/// If this returns false, capsule launch will fail hard.
pub fn is_supported() -> bool {
    std::path::Path::new("/dev/kvm").exists()
}
