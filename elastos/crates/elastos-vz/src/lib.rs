//! ElastOS Apple Virtualization.framework Compute Provider.
//!
//! Sibling backend to [`elastos-crosvm`](../../elastos-crosvm). Runs
//! `type: microvm` capsules on macOS with the same hardware-enforced
//! isolation Linux gets via KVM + crosvm, the same capsule artifacts,
//! the same Carrier transport, and the same capability tokens.
//!
//! # Status
//!
//! **Phase 1 — Scaffold only.** This crate compiles and exposes the
//! public surface (`VzProvider`, `VzConfig`, `VmConfig`,
//! `NetworkConfig`, `RunningVm`, `is_supported`) but does **not** make
//! any Apple `Virtualization.framework` API calls yet. Every entry
//! point that would launch a VM returns a deliberate
//! `"vz backend not yet implemented (Phase 2)"` error. This matches
//! the [Phase 1 deliverable in `docs/vz-backend/PLAN.md`][plan].
//!
//! Phases beyond Phase 1 are tracked in [`docs/vz-backend/PLAN.md`][plan]
//! and gated by [`docs/vz-backend/PHASE_0_SCOPE.md`][scope].
//!
//! # Cross-platform behaviour
//!
//! - **macOS (`cfg(target_os = "macos")`):** functional types compile
//!   against the future Vz wiring. `is_supported()` reports whether
//!   the host meets the macOS-version + Apple-Silicon prerequisites.
//! - **Linux / other:** the crate still compiles so the workspace
//!   builds remain green and CI is meaningful. The
//!   `network` module is replaced by [`network_stub`] which mirrors
//!   the public surface and fails closed on every microVM op.
//!   `is_supported()` returns `false`.
//!
//! # Principle anchors
//!
//! - [`PRINCIPLES.md` #10 *One Canonical Path*][p10]: Mac gets **one**
//!   substrate for `type: microvm` capsules — this crate, when Vz is
//!   actually wired up. No parallel "host-binary on Mac" path.
//! - [`PRINCIPLES.md` #11 *Fail Closed, Then Explain*][p11]: until
//!   Phase 2 lands, every microVM op on Mac fails closed with a
//!   clear, single-source-of-truth error message.
//! - [Linux-untouched gate in `docs/vz-backend/PLAN.md`][plan]: no
//!   modifications to `elastos-crosvm/`, `elastos-runtime/`,
//!   `elastos-common/`, `elastos-compute/`. CI enforces this via
//!   `scripts/check-linux-untouched.sh`.
//!
//! [plan]: ../../../docs/vz-backend/PLAN.md
//! [scope]: ../../../docs/vz-backend/PHASE_0_SCOPE.md
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
//!     // Phase 1: provider.load(...) returns "vz backend not yet
//!     // implemented (Phase 2)" until Phase 2 lands.
//! }
//! ```

mod config;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod network;
#[cfg(not(target_os = "macos"))]
#[path = "network_stub.rs"]
mod network;
mod provider;
mod vm;

pub use config::{VmConfig, VzConfig};
pub use network::NetworkConfig;
pub use provider::VzProvider;
pub use vm::RunningVm;

/// Vz backend marker used by Phase 1 fail-closed error messages so
/// every call site reads from one source of truth.
pub const PHASE_1_STUB_MESSAGE: &str =
    "vz backend not yet implemented (Phase 2). See docs/vz-backend/PLAN.md.";

/// Carrier-bridge device path inside the guest **on macOS**.
///
/// Vz exposes the kernel console only as a virtio-console serial
/// port; the Carrier bridge therefore moves from `/dev/hvc0` (the
/// Linux/crosvm convention) to `/dev/hvc1` (the second virtio-console
/// multi-port). This is a one-line guest-environment difference
/// surfaced to the capsule via `ELASTOS_CARRIER_PATH`; the wire
/// protocol over the socket is unchanged. See
/// `docs/vz-backend/PHASE_0_SCOPE.md` §D pitfalls #3 and #4.
pub const CARRIER_GUEST_DEVICE_PATH: &str = "/dev/hvc1";

/// Check whether the host supports the Vz backend.
///
/// Returns `false` everywhere outside macOS. On macOS, Phase 1 returns
/// `true` if the host is Apple Silicon (`aarch64-apple-darwin`); the
/// actual Vz capability probe (macOS version, hypervisor entitlement,
/// `VZVirtualMachineConfiguration::validate`) lands in Phase 2.
///
/// **Fail-closed contract:** if this returns `false`, the runtime
/// **must not** attempt to launch a `type: microvm` capsule on Mac.
/// The supervisor and `main.rs` registration site both honour this.
pub fn is_supported() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        // Phase 1: structural availability only. Phase 2 will replace
        // this with a real VZVirtualMachineConfiguration::validate() probe.
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
        // Phase 0 finding: virtio-console multi-port places the Carrier
        // bridge on the second console port, not the first.
        assert_eq!(CARRIER_GUEST_DEVICE_PATH, "/dev/hvc1");
    }

    #[test]
    fn phase_1_stub_message_references_phase_2_and_plan_doc() {
        // One source of truth for the fail-closed message; tests pin
        // both halves so future edits don't drift.
        assert!(PHASE_1_STUB_MESSAGE.contains("Phase 2"));
        assert!(PHASE_1_STUB_MESSAGE.contains("docs/vz-backend/PLAN.md"));
    }
}
