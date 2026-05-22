//! Apple `Virtualization.framework` FFI surface.
//!
//! **Phase 1: placeholder only.** This module exists to anchor the
//! crate's Vz-specific subdirectory and to make the Phase 2 fill-in
//! point greppable. No `objc2_virtualization` symbols are referenced
//! anywhere yet; Phase 2 will add per-class wrapper modules here:
//!
//! Phase 2 adds (one file per concern):
//!
//! - `boot_loader.rs` — wraps `VZLinuxBootLoader` (kernel +
//!   command-line + optional initrd).
//! - `platform.rs` — wraps `VZGenericPlatformConfiguration` and
//!   persisted `VZGenericMachineIdentifier`.
//! - `console.rs` — `VZVirtioConsoleDeviceSerialPortConfiguration`
//!   for kernel console + multi-port
//!   `VZVirtioConsoleDeviceConfiguration` for the Carrier bridge
//!   (`/dev/hvc1`).
//! - `block.rs` — `VZVirtioBlockDeviceConfiguration` backed by
//!   `VZDiskImageStorageDeviceAttachment` (caching=Cached,
//!   sync=Fsync per `PHASE_0_SCOPE.md` §D pitfall #1).
//! - `network.rs` — `VZNATNetworkDeviceAttachment` +
//!   `VZVirtioNetworkDeviceConfiguration`.
//! - `vsock.rs` — `VZVirtioSocketDeviceConfiguration` host-side
//!   adapter exposing the `(reader, writer, raw_fd)` triple
//!   `elastos-server/src/vm_provider.rs` needs.
//! - `entropy.rs` and `balloon.rs` — single-line constructors.
//! - `lifecycle.rs` — `VZVirtualMachine.start`/`requestStop` wired
//!   through a `VZVirtualMachineDelegate` into a Tokio channel.
//! - `dispatch.rs` — one `dispatch_queue` per `VzProvider` for Vz
//!   delegate callbacks (`PHASE_0_SCOPE.md` §D pitfall #10).
//!
//! Reading order before opening Phase 2: this file, then
//! [`docs/vz-backend/PHASE_0_SCOPE.md`][scope] §B (feature-coverage
//! table) and §D (pitfalls). Every Vz call written in Phase 2 should
//! be traceable to a §B row.
//!
//! [scope]: ../../../../docs/vz-backend/PHASE_0_SCOPE.md
