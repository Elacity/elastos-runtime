//! `VZVirtioEntropyDeviceConfiguration` wrapper.
//!
//! Vz exposes the host's CSPRNG to the guest through a virtio-rng
//! device. No knobs to configure; attaching the device is enough.
//!
//! Including this device matters because the guest's
//! `random.trust_cpu=on` (set in `VmConfig::vz_boot_args`) covers
//! Apple Silicon's hardware entropy source but virtio-rng still
//! seeds `/dev/urandom` faster on early-boot, which keeps TLS
//! handshakes from stalling.
//!

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2_virtualization::VZVirtioEntropyDeviceConfiguration;

pub(crate) fn build_entropy_device() -> Retained<VZVirtioEntropyDeviceConfiguration> {
    // SAFETY: `new()` is the standard objc2 allocator + init;
    // no thread-affinity constraint applies pre-attachment.
    unsafe { VZVirtioEntropyDeviceConfiguration::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_device_constructs() {
        let _cfg = build_entropy_device();
    }
}
