//! ElastOS Apple Virtualization.framework Compute Provider.
//!
//! Sibling backend to [`elastos-crosvm`](../../elastos-crosvm). Runs
//! `type: microvm` capsules on macOS with the same hardware-enforced
//! isolation Linux gets via KVM + crosvm, the same capsule artifacts,
//! the same Carrier transport, and the same capability tokens.
//!
//! # Status
//!
//! This crate is the macOS microVM backend. On supported Apple Silicon hosts it
//! builds Virtualization.framework VM configurations and runs Browser VM helper
//! workloads behind the same Runtime contracts used by the Linux crosvm path.
//! On unsupported hosts it compiles as an unavailable backend and fails closed.
//!
//! # Cross-platform behaviour
//!
//! - **macOS (`cfg(target_os = "macos")`):** functional types compile against
//!   Virtualization.framework. `is_supported()` reports whether the current
//!   host is an eligible Apple Silicon target.
//! - **Linux / other:** the crate still compiles so the workspace
//!   builds remain green and CI is meaningful. The
//!   `network` module is replaced by [`network_stub`] which mirrors
//!   the public surface and fails closed on every microVM op.
//!   `is_supported()` returns `false`.
//!
//! # Principle anchors
//!
//! - [`PRINCIPLES.md` #10 *One Canonical Path*][p10]: Mac gets **one**
//!   substrate for `type: microvm` capsules — this crate. No parallel
//!   "host-binary on Mac" path.
//! - [`PRINCIPLES.md` #11 *Fail Closed, Then Explain*][p11]: unsupported
//!   hosts and incomplete VM records fail closed with a clear,
//!   single-source-of-truth error message.
//! - [Linux-untouched gate in `docs/MAC.md`][plan]: no
//!   modifications to `elastos-crosvm/`, `elastos-runtime/`,
//!   `elastos-common/`, or `elastos-compute/` unless the shared Runtime contract
//!   is intentionally changed.
//!
//! [p10]: ../../../PRINCIPLES.md
//! [p11]: ../../../PRINCIPLES.md
//!
//! # Example
//!
//! ```ignore
//! use elastos_vz::{VzConfig, VzProvider};
//!
//! if elastos_vz::is_supported() {
//!     let config = VzConfig::new();
//!     let provider = VzProvider::new(config)?;
//!     provider.init().await?;
//!     // provider.load(...) validates and prepares a macOS VM record.
//! }
//! ```

mod config;
mod error;
#[cfg(target_os = "macos")]
mod ffi;
pub mod logger;
#[cfg(target_os = "macos")]
mod network;
#[cfg(not(target_os = "macos"))]
#[path = "network_stub.rs"]
mod network;
mod provider;
mod vm;

pub use config::{
    ConfigError, VmConfig, VmConfigLimits, VzConfig, DEFAULT_MAX_MEMORY_MIB,
    DEFAULT_MAX_VCPU_COUNT, DEFAULT_VZ_STOP_TIMEOUT,
};
pub use error::{VzError, VzErrorReport, VzExitReason};
pub use network::NetworkConfig;
pub use provider::VzProvider;
pub use vm::RunningVm;

/// Shared fail-closed message for paths that cannot operate without a loaded
/// macOS Virtualization.framework VM handle.
pub const VZ_BACKEND_UNAVAILABLE_MESSAGE: &str =
    "vz backend unavailable on this host or VM handle was not loaded";

/// Carrier-bridge device path inside the guest **on macOS**.
///
/// Vz exposes the kernel console only as a virtio-console serial
/// port; the Carrier bridge therefore moves from `/dev/hvc0` (the
/// Linux/crosvm convention) to `/dev/hvc1` (the second virtio-console
/// multi-port). This is a one-line guest-environment difference
/// surfaced to the capsule via `ELASTOS_CARRIER_PATH`; the wire
/// protocol over the socket is unchanged.
pub const CARRIER_GUEST_DEVICE_PATH: &str = "/dev/hvc1";

/// Check whether the host supports the Vz backend.
///
/// Returns `false` everywhere outside macOS. On macOS, returns `true` on
/// Apple Silicon hosts where Virtualization.framework is the intended VM
/// substrate.
///
/// **Fail-closed contract:** if this returns `false`, the runtime
/// **must not** attempt to launch a `type: microvm` capsule on Mac.
/// The supervisor and `main.rs` registration site both honour this.
pub fn is_supported() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        true
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_supported_compiles_and_returns_bool() {
        let supported = is_supported();
        // Strictly typed sanity: `bool` not `Result<bool>`.
        let _: bool = supported;

        // Off-mac platforms must always return false.
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        assert!(
            !supported,
            "is_supported() must be false outside macOS arm64"
        );
    }

    #[test]
    fn carrier_guest_device_path_constant_is_hvc1() {
        assert_eq!(CARRIER_GUEST_DEVICE_PATH, "/dev/hvc1");
    }

    #[test]
    fn unavailable_message_is_single_source_of_truth() {
        // One source of truth for the fail-closed message; tests pin
        // the stable operator wording so future edits don't drift.
        assert!(VZ_BACKEND_UNAVAILABLE_MESSAGE.contains("vz backend unavailable"));
        assert!(VZ_BACKEND_UNAVAILABLE_MESSAGE.contains("VM handle was not loaded"));
    }
}
