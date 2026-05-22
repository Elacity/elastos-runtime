//! `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` wrapper.
//!
//! Phase 0 §B row "balloon": Vz exposes the standard virtio
//! memory balloon device for cooperative memory reclaim. The
//! supervisor's existing memory accounting (Phase 0 §B sequence
//! diagram, "balloon" arrow) drives it from the host side; the
//! configuration here only ensures the guest sees the device.
//!
//! Day 1 reality probe verified the construction. No
//! configurable properties.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2_virtualization::VZVirtioTraditionalMemoryBalloonDeviceConfiguration;

pub(crate) fn build_balloon_device() -> Retained<VZVirtioTraditionalMemoryBalloonDeviceConfiguration>
{
    // SAFETY: `new()` is the standard objc2 allocator + init;
    // the device is inert until attached to the VM.
    unsafe { VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balloon_device_constructs() {
        let _cfg = build_balloon_device();
    }
}
