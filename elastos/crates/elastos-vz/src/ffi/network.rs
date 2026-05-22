//! NAT-backed virtio-net wrapper.
//!
//! Phase 0 §B row "network — NAT": Apple's
//! `VZNATNetworkDeviceAttachment` gives every VM outbound NAT
//! plus DHCP through macOS's shared internet, with no
//! entitlement beyond `com.apple.security.virtualization`
//! (which Phase 6 adds). It is the only network attachment
//! Day 2 needs. `VZBridgedNetworkDeviceAttachment` requires the
//! additional `com.apple.vm.networking` entitlement and is
//! deferred to a later phase.
//!
//! Day 1 reality probe verified:
//! - `VZNATNetworkDeviceAttachment::new()`
//! - `VZMACAddress::randomLocallyAdministeredAddress()`
//! - `VZVirtioNetworkDeviceConfiguration::setAttachment` /
//!   `setMACAddress`
//!
//! We always assign a fresh **locally-administered** MAC so the
//! VM doesn't collide with the host's burned-in MAC and so it
//! is correctly disjoint from any IANA-registered prefix.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::NSString;
use objc2_virtualization::{
    VZBridgedNetworkDeviceAttachment, VZBridgedNetworkInterface, VZMACAddress,
    VZNATNetworkDeviceAttachment, VZVirtioNetworkDeviceConfiguration,
};

use crate::network::NetworkConfig;

/// Build a virtio-net device backed by Apple's NAT attachment.
///
/// The returned configuration is ready to hand to
/// `VZVirtualMachineConfiguration::setNetworkDevices`. The MAC
/// address is locally-administered and randomly generated, so
/// every call yields a different MAC; capsules that need a
/// stable MAC across reboots will get one in Phase 3 by reading
/// the persisted machine identifier and deriving from it.
pub(crate) fn build_nat_network() -> Retained<VZVirtioNetworkDeviceConfiguration> {
    // SAFETY: each Vz constructor below is the standard
    // `AnyThread::new()` allocator + `init` pattern. No thread
    // ownership constraints apply pre-attachment.
    let attachment = unsafe { VZNATNetworkDeviceAttachment::new() };
    let mac = unsafe { VZMACAddress::randomLocallyAdministeredAddress() };

    let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
    unsafe { net.setAttachment(Some(&attachment)) };
    unsafe { net.setMACAddress(&mac) };

    net
}

/// Build a virtio-net device backed by Apple's bridged
/// attachment. **Phase 3 Day 7.**
///
/// Requires the `com.apple.vm.networking` Apple entitlement.
/// The caller (the FFI builder) is responsible for verifying
/// that entitlement via
/// [`crate::ffi::entitlement::has_vm_networking_entitlement`]
/// *before* invoking this helper — calling it on an unentitled
/// process produces a runtime Vz error rather than a clean
/// build-time rejection.
///
/// The MAC address is derived deterministically from
/// `net_cfg.guest_mac` so capsules see the same MAC across
/// reboots, matching the crosvm flow (`NetworkConfig::new`
/// computes a hash-based MAC). If `net_cfg.guest_mac` is not a
/// valid MAC string (defensive — should never happen because
/// `NetworkConfig::new` always produces one), the call fails
/// closed.
///
/// Returns an error if no host interface is available for
/// bridging — e.g. on a Mac with all interfaces disabled, or
/// inside another VM. Surfacing this as a typed error lets the
/// supervisor refuse to start the capsule with a clear message
/// rather than producing a network device the guest can't use.
pub(crate) fn build_bridged_network(
    net_cfg: &NetworkConfig,
) -> Result<Retained<VZVirtioNetworkDeviceConfiguration>, String> {
    let interface = pick_first_bridged_interface()?;
    let attachment = unsafe {
        VZBridgedNetworkDeviceAttachment::initWithInterface(
            VZBridgedNetworkDeviceAttachment::alloc(),
            &interface,
        )
    };

    let mac_ns = NSString::from_str(&net_cfg.guest_mac);
    let mac = unsafe { VZMACAddress::initWithString(VZMACAddress::alloc(), &mac_ns) }.ok_or_else(
        || {
            format!(
                "vz bridged network: '{}' is not a valid MAC address",
                net_cfg.guest_mac
            )
        },
    )?;

    let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
    unsafe { net.setAttachment(Some(&attachment)) };
    unsafe { net.setMACAddress(&mac) };
    Ok(net)
}

/// Pick the first available bridged interface from
/// `VZBridgedNetworkInterface.networkInterfaces`.
///
/// On a properly entitled binary this is typically `en0` (the
/// primary Ethernet/Wi-Fi adapter). We do not let the manifest
/// pick the interface today — that's a Phase 4 refinement
/// (`NetworkConfig.bridge_interface`). For now we use the first
/// system-reported interface, which matches what Apple's
/// `vmnet`-based examples do.
fn pick_first_bridged_interface() -> Result<Retained<VZBridgedNetworkInterface>, String> {
    let interfaces = unsafe { VZBridgedNetworkInterface::networkInterfaces() };
    let count = interfaces.count();
    if count == 0 {
        return Err(
            "vz bridged network: no host interface available for bridging — \
             a signed binary with `com.apple.vm.networking` reports \
             `VZBridgedNetworkInterface.networkInterfaces` is empty. \
             Check that the host has an active Ethernet/Wi-Fi adapter."
                .to_string(),
        );
    }
    let first = interfaces.objectAtIndex(0);
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_network_round_trip_mac_address() {
        let net = build_nat_network();
        // `MACAddress` is defined on the superclass
        // `VZNetworkDeviceConfiguration`; the all-caps spelling
        // mirrors the Objective-C selector exactly so it does
        // not collide with any future `macAddress` accessor.
        let mac = unsafe { net.MACAddress() };
        let mac_string = unsafe { mac.string() }.to_string();
        // Locally-administered MACs have the second-least-significant bit
        // of the first octet set (xx:xx:xx:xx:xx:xx where (xx & 0x02) == 0x02).
        // Quick textual sanity since we don't have a MAC parser handy.
        let first_octet_hex = mac_string
            .split(':')
            .next()
            .expect("mac string has at least one octet");
        let first_octet =
            u8::from_str_radix(first_octet_hex, 16).expect("first octet parses as hex");
        assert!(
            first_octet & 0x02 != 0,
            "expected locally-administered MAC (0x02 bit set); got {mac_string}"
        );
        // Multicast bit (0x01) must NOT be set on a unicast MAC.
        assert_eq!(
            first_octet & 0x01,
            0,
            "unicast MAC required; got multicast {mac_string}"
        );
    }
}
