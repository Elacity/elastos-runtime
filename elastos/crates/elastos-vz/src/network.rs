//! Guest-network configuration (macOS, Phase 1: data-only).
//!
//! Mirrors the public shape of [`elastos_crosvm::NetworkConfig`] so
//! the supervisor's existing call sites map cleanly. Phase 1 is
//! data-only — no `VZNATNetworkDeviceAttachment` construction. Phase
//! 3 wires the real Vz network device per
//! `docs/vz-backend/PLAN.md` Phase 3 and the Phase 0
//! pitfalls (NAT default; bridged mode deferred behind the
//! `com.apple.vm.networking` entitlement).

use std::sync::atomic::AtomicI32;

use elastos_common::{ElastosError, Result};

use crate::PHASE_1_STUB_MESSAGE;

/// Network configuration for a single Vz VM. Shape mirrors
/// [`elastos_crosvm::NetworkConfig`].
pub struct NetworkConfig {
    pub tap_name: String,
    pub host_ip: String,
    pub guest_ip: String,
    pub mask: String,
    pub prefix_len: u8,
    pub guest_mac: String,
    /// Reserved for Phase 3 wiring; unused in Phase 1.
    _tap_fd: AtomicI32,
}

impl std::fmt::Debug for NetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkConfig")
            .field("tap_name", &self.tap_name)
            .field("host_ip", &self.host_ip)
            .field("guest_ip", &self.guest_ip)
            .finish()
    }
}

impl Clone for NetworkConfig {
    fn clone(&self) -> Self {
        Self {
            tap_name: self.tap_name.clone(),
            host_ip: self.host_ip.clone(),
            guest_ip: self.guest_ip.clone(),
            mask: self.mask.clone(),
            prefix_len: self.prefix_len,
            guest_mac: self.guest_mac.clone(),
            _tap_fd: AtomicI32::new(-1),
        }
    }
}

impl NetworkConfig {
    /// Derive a deterministic per-VM network config. Pure logic —
    /// identical to the crosvm version so capsules see the same
    /// host/guest IP pair across substrates when network is opt-in.
    pub fn new(vm_id: &str) -> Self {
        let tap_suffix: String = vm_id.chars().take(8).collect();
        let tap_name = format!("vz{}", tap_suffix);
        let subnet_octet = subnet_octet_for_vm(vm_id);

        Self {
            tap_name,
            host_ip: format!("172.16.{}.1", subnet_octet),
            guest_ip: format!("172.16.{}.2", subnet_octet),
            mask: "255.255.255.252".to_string(),
            prefix_len: 30,
            guest_mac: generate_mac(vm_id),
            _tap_fd: AtomicI32::new(-1),
        }
    }

    /// Set up the network. Phase 1: fail closed. Phase 3 attaches a
    /// `VZNATNetworkDeviceAttachment` to the running config.
    pub fn setup(&self) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (NetworkConfig::setup: tap='{}')",
            PHASE_1_STUB_MESSAGE, self.tap_name
        )))
    }

    /// Tear down the network. Phase 1: idempotent no-op because
    /// nothing was set up.
    pub fn teardown(&self) -> Result<()> {
        Ok(())
    }
}

/// Deterministic MAC derivation. Identical to the crosvm helper.
pub fn generate_mac(vm_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    vm_id.hash(&mut hasher);
    let hash = hasher.finish();
    format!(
        "AA:FC:{:02X}:{:02X}:{:02X}:{:02X}",
        (hash >> 8) as u8,
        (hash >> 16) as u8,
        (hash >> 24) as u8,
        (hash >> 32) as u8,
    )
}

/// Deterministic subnet allocator. Identical to the crosvm helper.
pub fn subnet_octet_for_vm(vm_id: &str) -> u8 {
    let hash: u64 = vm_id
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(131).wrapping_add(b as u64));
    ((hash % 250) as u8) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_config_new_matches_crosvm_shape() {
        let config = NetworkConfig::new("vm-phase1");
        assert!(config.tap_name.starts_with("vz"));
        assert!(config.host_ip.starts_with("172.16."));
        assert!(config.host_ip.ends_with(".1"));
        assert!(config.guest_ip.ends_with(".2"));
        assert_eq!(config.prefix_len, 30);
        assert!(config.guest_mac.starts_with("AA:FC:"));
    }

    #[test]
    fn network_setup_fails_closed_in_phase_1() {
        let config = NetworkConfig::new("vm-phase1");
        let err = config.setup().unwrap_err();
        assert!(err.to_string().contains(PHASE_1_STUB_MESSAGE));
    }

    #[test]
    fn network_teardown_is_noop_ok() {
        let config = NetworkConfig::new("vm-phase1");
        assert!(config.teardown().is_ok());
    }

    #[test]
    fn generate_mac_is_deterministic() {
        let a = generate_mac("vm-x");
        let b = generate_mac("vm-x");
        let c = generate_mac("vm-y");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
