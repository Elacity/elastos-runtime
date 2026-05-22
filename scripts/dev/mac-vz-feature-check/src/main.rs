//! mac-vz-feature-check — Apple Virtualization.framework feature probe.
//!
//! This binary is a developer-side diagnostic, run manually on a Mac
//! before Phase 2 begins. It reports which Vz features the running
//! host actually exposes, so the Phase 2 implementation can rely on
//! a concrete capability matrix rather than the spec sheet.
//!
//! # Status
//!
//! **Phase 1 scaffold.** This file prints a TODO banner. Phase 1
//! Day 2 fills in the real probes per
//! `docs/vz-backend/PHASE_0_SCOPE.md` Appendix A:
//!
//! 1. `VZVirtualMachineConfiguration::validate()` on a minimal config
//!    (1 vCPU, 128 MiB, no devices) — confirms the host meets the
//!    macOS-version + entitlement baseline.
//! 2. Construct each device class from the §B coverage table and
//!    check `validate()` does not throw:
//!    - `VZLinuxBootLoader`
//!    - `VZVirtioBlockDeviceConfiguration`
//!    - `VZVirtioSocketDeviceConfiguration`
//!    - `VZVirtioConsoleDeviceConfiguration` + multi-port
//!    - `VZNATNetworkDeviceAttachment`
//!    - `VZVirtioEntropyDeviceConfiguration`
//!    - `VZMacOSBootLoader` (negative check — should be unavailable
//!      for Linux guests)
//! 3. Probe macOS version via `NSProcessInfo.operatingSystemVersion`
//!    and report the (major, minor, patch) triple.
//! 4. Probe CPU model via sysctl `hw.optional.arm64` and confirm
//!    Apple Silicon.
//! 5. Optionally read the `com.apple.security.virtualization` and
//!    `com.apple.vm.networking` entitlements off the running binary
//!    (so unsigned dev builds get a clear "you need entitlements"
//!    message before Phase 2 hits the same wall at runtime).
//!
//! Run with: `cargo run --manifest-path scripts/dev/mac-vz-feature-check/Cargo.toml`

fn main() {
    println!("mac-vz-feature-check (Phase 1 scaffold)");
    println!();
    println!("This binary is a placeholder. Phase 1 Day 2 fills it in per");
    println!("docs/vz-backend/PHASE_0_SCOPE.md Appendix A:");
    println!();
    println!("  1. VZVirtualMachineConfiguration::validate baseline");
    println!("  2. Per-device construct + validate matrix");
    println!("  3. macOS version + CPU model report");
    println!("  4. Entitlement audit on the running binary");
    println!();
    println!("Until then, treat the §B feature-coverage table as the");
    println!("source of truth; this binary will replace that lookup with");
    println!("a live host probe before Phase 2 lands.");
}
