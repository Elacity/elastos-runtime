//! ElastOS crosvm Compute Provider
//!
//! Runs capsules in crosvm VMs with hardware-level isolation on Linux/KVM.
//! crosvm is the sole microVM backend; capsule launch fails closed on hosts
//! without `/dev/kvm` rather than silently downgrading.
//!
//! On non-Linux hosts (macOS, Windows) the crate still compiles so the rest
//! of the runtime — browser-hosted Home, WASM capsules, data capsules — stays
//! in scope. The guest-network module is replaced by a stub that mirrors the
//! public surface and returns explicit errors if any microVM path is invoked.
//!
//! # Linux requirements
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
mod egress_audit;
mod egress_firewall;
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
#[cfg(target_os = "linux")]
pub use egress_audit::NflogReader;
pub use egress_audit::{parse_nflog_message, EgressDrop};
pub use egress_firewall::{
    EgressFirewall, EGRESS_LOG_RATE_PER_SEC, EGRESS_NFLOG_GROUP, EGRESS_TABLE,
};
pub use network::NetworkConfig;
pub use provider::CrosvmProvider;
pub use proxy::TcpProxy;
pub use vm::RunningVm;

/// Check if the system supports crosvm (has KVM).
/// If this returns false, capsule launch will fail hard.
pub fn is_supported() -> bool {
    std::path::Path::new("/dev/kvm").exists()
}
