//! Non-macOS stub for the Vz guest-network module.
//!
//! On non-macOS hosts (Linux, Windows) the Vz substrate is not
//! available at all (`is_supported()` returns false), so this stub
//! exists only to keep the crate compilable in the workspace and to
//! mirror the public shape of [`elastos_crosvm::NetworkConfig`]. Any
//! attempt to invoke a microVM path on a non-Mac host MUST fail
//! closed before reaching this code — these methods just provide a
//! second line of defence with a typed error.

use std::sync::atomic::AtomicI32;

use elastos_common::{ElastosError, Result};

/// Network configuration mirror for non-macOS platforms.
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

    /// Fails closed on non-macOS: the Vz substrate requires macOS.
    pub fn setup(&self) -> Result<()> {
        Err(ElastosError::Compute(
            "vz backend requires macOS (Apple Virtualization.framework); \
             this build target does not support Vz networking"
                .into(),
        ))
    }

    /// Idempotent no-op.
    pub fn teardown(&self) -> Result<()> {
        Ok(())
    }
}

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
    fn stub_setup_fails_closed_with_macos_reason() {
        let config = NetworkConfig::new("vm");
        let err = config.setup().unwrap_err();
        assert!(err.to_string().contains("macOS"));
    }

    #[test]
    fn stub_teardown_is_noop_ok() {
        let config = NetworkConfig::new("vm");
        assert!(config.teardown().is_ok());
    }
}
