//! Non-Linux stub for the guest-network module.
//!
//! crosvm guest networking is implemented via Linux TAP devices and ioctl.
//! On non-Linux platforms (macOS, Windows) the runtime can still compile and
//! run for KVM-independent paths (browser-hosted Home, WASM capsules, data
//! capsules). MicroVM launch already fails closed via the `/dev/kvm` check in
//! `vm.rs::start()`; this stub keeps the rest of the crate buildable.
//!
//! The default Home path remains KVM-independent so macOS and Windows stay
//! in scope without pretending to offer Linux microVM parity.

use std::sync::atomic::AtomicI32;

use elastos_common::{ElastosError, Result};

/// Network configuration mirror for non-Linux platforms.
///
/// The field shape mirrors the Linux implementation so that callers compile
/// unchanged. All operations that would touch a real TAP device fail closed.
pub struct NetworkConfig {
    pub tap_name: String,
    pub host_ip: String,
    pub guest_ip: String,
    pub mask: String,
    pub prefix_len: u8,
    pub guest_mac: String,
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
    pub fn new(vm_id: &str) -> Self {
        let tap_suffix: String = vm_id.chars().take(8).collect();
        let tap_name = format!("cv{}", tap_suffix);
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

    /// Fails closed on non-Linux: TAP networking requires the Linux tun/tap
    /// stack and CAP_NET_ADMIN. Callers should never reach this path because
    /// `crosvm::is_supported()` returns `false` here, but the error is explicit
    /// in case a code path slips through.
    pub fn setup(&self) -> Result<()> {
        Err(ElastosError::Compute(
            "crosvm guest networking requires Linux (TAP via /dev/net/tun); \
             this build target does not support microVM networking"
                .into(),
        ))
    }

    /// Idempotent on non-Linux: nothing was set up, so nothing to tear down.
    pub fn teardown(&self) -> Result<()> {
        Ok(())
    }
}

/// Deterministic MAC derivation. Pure logic, identical to the Linux module.
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

/// Deterministic subnet allocator. Pure logic, identical to the Linux module.
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
    fn stub_network_config_new_matches_linux_shape() {
        let config = NetworkConfig::new("test-vm-123");
        assert!(config.tap_name.starts_with("cv"));
        assert!(config.host_ip.starts_with("172.16."));
        assert!(config.host_ip.ends_with(".1"));
        assert!(config.guest_ip.ends_with(".2"));
        assert_eq!(config.prefix_len, 30);
    }

    #[test]
    fn stub_setup_fails_closed_with_explicit_reason() {
        let config = NetworkConfig::new("test-vm");
        let err = config.setup().unwrap_err();
        assert!(
            err.to_string().contains("Linux"),
            "expected error to mention Linux requirement, got: {err}"
        );
    }

    #[test]
    fn stub_teardown_is_noop_ok() {
        let config = NetworkConfig::new("test-vm");
        assert!(config.teardown().is_ok());
    }
}
