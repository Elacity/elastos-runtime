//! Apple `Virtualization.framework` FFI surface.
//!
//! Submodules — one per concern, each anchored to a row in
//! [`docs/vz-backend/PHASE_0_SCOPE.md`][scope] §B (feature
//! coverage) or §D (pitfalls):
//!
//! - [`boot_loader`] — `VZLinuxBootLoader` (§B + §D pitfall #3).
//! - [`platform`] — `VZGenericPlatformConfiguration` and persisted
//!   `VZGenericMachineIdentifier` (§D pitfall #2).
//! - [`console`] — kernel `VZVirtioConsoleDeviceSerialPortConfiguration`
//!   on `/dev/hvc0` **plus** the multi-port
//!   `VZVirtioConsoleDeviceConfiguration` slot for the Carrier
//!   bridge on `/dev/hvc1` (§D pitfalls #4, #7).
//! - [`block`] — `VZVirtioBlockDeviceConfiguration` backed by
//!   `VZDiskImageStorageDeviceAttachment` with
//!   `cachingMode=Cached, synchronizationMode=Fsync`
//!   (§D pitfall #1 / UTM #4840).
//! - [`network`] — `VZNATNetworkDeviceAttachment` +
//!   `VZVirtioNetworkDeviceConfiguration` (no entitlement).
//! - [`vsock`] — `VZVirtioSocketDeviceConfiguration` (§D pitfall #5;
//!   no CID API).
//! - [`entropy`], [`balloon`] — single-call constructors (§B).
//! - [`dispatch`] — per-`VzProvider` serial GCD queue (§D pitfall #10).
//! - [`error`] — `NSError` → Rust `String` helper.
//! - [`builder`] — [`builder::BuiltMachine::from_vm_config`]
//!   assembles every device above into a single
//!   `VZVirtualMachineConfiguration`. Phase 2 Day 3 hands the
//!   result to `VZVirtualMachine::initWithConfiguration:queue:`
//!   for actual VM start; Day 2 stops at "construction works".
//!
//! Every wrapper is `cfg(target_os = "macos")` so the Linux
//! workspace build keeps green; non-macOS callers use the
//! `network_stub` shim plus the Phase 1 fail-closed error path
//! in `provider.rs` / `vm.rs`.
//!
//! [scope]: ../../../../docs/vz-backend/PHASE_0_SCOPE.md

// The whole `ffi` module is `#[cfg(target_os = "macos")]`-gated
// in `lib.rs`, so this file is only compiled on macOS. We do
// **not** repeat the cfg here — clippy flags the duplication.
//
// Day 2 ships the FFI wrappers + builder, but the lifecycle
// integration (Day 3) is what actually calls them from
// `provider.rs::load()` and `vm.rs::start()`. Until then every
// submodule's public function is reachable only from its own
// `#[cfg(test)]` tests and from `builder.rs`, so a strict `cargo
// build` reports them as dead. Allow at the module root with a
// single, auditable annotation so dropping it in Day 3 is one
// line — and so we don't sprinkle `#[allow(dead_code)]` over
// every wrapper, which would mask real future bit-rot.
#![allow(dead_code)]

pub(crate) mod balloon;
pub(crate) mod block;
pub(crate) mod boot_loader;
pub(crate) mod builder;
pub(crate) mod console;
pub(crate) mod dispatch;
pub(crate) mod entropy;
pub(crate) mod error;
pub(crate) mod network;
pub(crate) mod platform;
pub(crate) mod vsock;
