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
// As of Day 3 the FFI surface is reachable through
// `ffi::lifecycle::VzMachineHandle`, which is itself reached
// from `provider.rs::load`. The blanket `#![allow(dead_code)]`
// Day 2 used is therefore gone; specific items that won't be
// wired until later phases (e.g. the Carrier multi-port
// attachment Phase 3 will replace) carry their own
// `#[allow(dead_code)]` with an inline justification.

pub(crate) mod balloon;
pub(crate) mod block;
pub(crate) mod boot_loader;
pub(crate) mod builder;
pub(crate) mod console;
pub(crate) mod console_forwarder;
pub(crate) mod delegate;
pub(crate) mod dispatch;
pub(crate) mod entitlement;
pub(crate) mod entropy;
pub(crate) mod error;
pub(crate) mod lifecycle;
pub(crate) mod network;
pub(crate) mod platform;
pub(crate) mod vsock;
