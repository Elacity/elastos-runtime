//! `VZVirtioSocketDeviceConfiguration` wrapper (vsock).
//!
//! Phase 0 §D pitfall #5 (`docs/vz-backend/PHASE_0_SCOPE.md`):
//! Apple's Vz does **not** expose a public API to set a vsock
//! CID. The crosvm path on Linux negotiates CIDs explicitly via
//! `AF_VSOCK`; the Vz path uses per-VM connections through
//! `VZVirtioSocketConnection` once the VM is running. That
//! adaptation lives in the (Phase 3) supervisor bridge — Day 2
//! only needs to attach the device class so the VM exposes a
//! vsock interface at all.
//!
//! Day 1 reality probe verified the configuration constructs.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2_virtualization::VZVirtioSocketDeviceConfiguration;

/// Build a vsock device configuration. There are no per-VM
/// knobs to set; Vz allocates the CID itself and exposes it via
/// `VZVirtualMachine.socketDevices[0]` once the VM starts.
pub(crate) fn build_vsock_device() -> Retained<VZVirtioSocketDeviceConfiguration> {
    // SAFETY: `new()` allocates and initialises a vsock
    // configuration; no thread-safety constraint applies before
    // it's attached to a `VZVirtualMachineConfiguration`.
    unsafe { VZVirtioSocketDeviceConfiguration::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vsock_device_constructs() {
        let _cfg = build_vsock_device();
        // No public properties to assert; the contract is that
        // construction is infallible (matches the probe binary).
    }
}
