//! Guest-network compatibility mode for crosvm VMs.
//!
//! Creates TAP devices via ioctl (no external commands).
//! Requires CAP_NET_ADMIN on the runtime binary.
//! Ordinary app capsules should stay on the serial Carrier bridge and not use this.
//! No iptables, no ip_forward — guest-network capsules can only reach the host runtime.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::AtomicI32;

use elastos_common::{ElastosError, Result};

// ── Linux ioctl constants ──────────────────────────────────────────

// TUNSETIFF = _IOW('T', 202, int) = 0x400454ca (same on x86_64 and aarch64)
// ioctl uses nix::ioctl_write_ptr_bad! pattern, but we avoid the nix dep.
// libc::ioctl second param is c_ulong on x86_64, c_int on aarch64.
// We use the nix approach: define as u32, cast at call site.
const TUNSETIFF: u32 = 0x400454ca;
const TUNSETOWNER: u32 = 0x400454cc;
const IFF_TAP: libc::c_short = 0x0002;
const IFF_NO_PI: libc::c_short = 0x1000;
const SIOCGIFFLAGS: u32 = 0x8913;
const SIOCSIFFLAGS: u32 = 0x8914;
const SIOCSIFADDR: u32 = 0x8916;
const SIOCSIFNETMASK: u32 = 0x891c;
const IFF_UP: libc::c_short = 0x1;

/// Call ioctl with the correct type for the platform.
/// libc::ioctl takes c_ulong on glibc x86_64, c_int everywhere else (musl, aarch64).
unsafe fn ioctl_raw(fd: i32, request: u32, arg: *mut libc::c_void) -> i32 {
    // Cast to the platform's ioctl request type at the call site.
    // On all Linux targets, the kernel ioctl number fits in 32 bits.
    #[cfg(target_env = "gnu")]
    {
        libc::ioctl(fd, request as libc::c_ulong, arg)
    }
    #[cfg(not(target_env = "gnu"))]
    {
        libc::ioctl(fd, request as libc::c_int, arg)
    }
}

// ── FFI struct ─────────────────────────────────────────────────────

/// Mirrors `struct ifreq` from <net/if.h>.
#[repr(C)]
struct Ifreq {
    ifr_name: [u8; libc::IFNAMSIZ], // 16 bytes
    ifr_data: [u8; 24],             // union: sockaddr, flags, etc.
}

impl Ifreq {
    fn new(name: &str) -> Self {
        let mut ifr: Self = unsafe { std::mem::zeroed() };
        let bytes = name.as_bytes();
        let len = bytes.len().min(libc::IFNAMSIZ - 1);
        ifr.ifr_name[..len].copy_from_slice(&bytes[..len]);
        ifr
    }

    fn set_addr(&mut self, ip: std::net::Ipv4Addr) {
        let sa = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from(ip).to_be(),
            },
            sin_zero: [0; 8],
        };
        let bytes: [u8; std::mem::size_of::<libc::sockaddr_in>()] =
            unsafe { std::mem::transmute(sa) };
        self.ifr_data[..bytes.len()].copy_from_slice(&bytes);
    }

    fn set_flags(&mut self, flags: libc::c_short) {
        self.ifr_data[..2].copy_from_slice(&flags.to_ne_bytes());
    }

    fn get_flags(&self) -> libc::c_short {
        libc::c_short::from_ne_bytes([self.ifr_data[0], self.ifr_data[1]])
    }
}

fn udp_socket() -> Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(ElastosError::Compute(format!(
            "socket() failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

// ── NetworkConfig ──────────────────────────────────────────────────

/// Network configuration for a VM.
/// Each VM gets a private /30 point-to-point link to the host.
/// The capsule can only reach the host runtime — no internet access.
pub struct NetworkConfig {
    pub tap_name: String,
    pub host_ip: String,
    pub guest_ip: String,
    pub mask: String,
    pub prefix_len: u8,
    pub guest_mac: String,
    /// Test-only marker proving cloned configs do not inherit ownership state.
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
            _tap_fd: AtomicI32::new(-1), // Clones don't own the fd
        }
    }
}

impl Drop for NetworkConfig {
    fn drop(&mut self) {
        // Persistent TAP — teardown handles cleanup
    }
}

impl NetworkConfig {
    /// Create a new network config for a VM.
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

    /// Create the TAP device and configure the network.
    /// Requires CAP_NET_ADMIN (set via `setcap cap_net_admin+ep` on the binary).
    pub fn setup(&self) -> Result<()> {
        // 1. Create TAP device
        let fd = self.create_tap()?;

        // 2. Assign host IP
        self.set_ip()?;

        // 3. Set netmask
        self.set_netmask()?;

        // 4. Bring interface up
        self.bring_up()?;

        // 5. Make TAP persistent and close our fd.
        // TUNSETPERSIST keeps the device alive after we close the fd.
        // crosvm will reopen it by name via --net tap-name=...
        const TUNSETPERSIST: u32 = 0x400454cb;
        unsafe {
            ioctl_raw(fd, TUNSETPERSIST, std::ptr::without_provenance_mut(1));
        }
        unsafe {
            libc::close(fd);
        }

        tracing::info!(
            "Guest-network TAP configured (ioctl): tap={} host={} guest={}",
            self.tap_name,
            self.host_ip,
            self.guest_ip
        );

        Ok(())
    }

    /// Tear down the TAP device.
    pub fn teardown(&self) -> Result<()> {
        // Open the persistent TAP to clear TUNSETPERSIST, then close.
        // This destroys the device.
        let fd = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR) };
        if fd >= 0 {
            let mut ifr = Ifreq::new(&self.tap_name);
            ifr.set_flags(IFF_TAP | IFF_NO_PI);
            if unsafe { ioctl_raw(fd, TUNSETIFF, &mut ifr as *mut Ifreq as *mut _) } == 0 {
                const TUNSETPERSIST: u32 = 0x400454cb;
                unsafe {
                    ioctl_raw(fd, TUNSETPERSIST, std::ptr::null_mut());
                }
            }
            unsafe {
                libc::close(fd);
            }
            tracing::info!("Guest-network TAP torn down: tap={}", self.tap_name);
        }
        Ok(())
    }

    fn create_tap(&self) -> Result<i32> {
        let fd = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(ElastosError::Compute(format!(
                "open(/dev/net/tun) failed: {}. Run: sudo setcap cap_net_admin+ep $(which elastos)",
                std::io::Error::last_os_error()
            )));
        }

        let mut ifr = Ifreq::new(&self.tap_name);
        ifr.set_flags(IFF_TAP | IFF_NO_PI);

        if unsafe { ioctl_raw(fd, TUNSETIFF, &mut ifr as *mut Ifreq as *mut _) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(ElastosError::Compute(format!(
                "TUNSETIFF failed for '{}': {}. Run: sudo setcap cap_net_admin+ep $(which elastos)",
                self.tap_name, err
            )));
        }

        // Set owner to current user
        let uid = unsafe { libc::geteuid() } as libc::c_ulong;
        if unsafe { ioctl_raw(fd, TUNSETOWNER, uid as *mut _) } < 0 {
            tracing::warn!(
                "TUNSETOWNER failed (non-fatal): {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(fd)
    }

    fn set_ip(&self) -> Result<()> {
        let sock = udp_socket()?;
        let ip: std::net::Ipv4Addr = self
            .host_ip
            .parse()
            .map_err(|e| ElastosError::Compute(format!("Invalid IP '{}': {}", self.host_ip, e)))?;
        let mut ifr = Ifreq::new(&self.tap_name);
        ifr.set_addr(ip);
        if unsafe {
            ioctl_raw(
                sock.as_raw_fd(),
                SIOCSIFADDR,
                &ifr as *const Ifreq as *mut _,
            )
        } < 0
        {
            return Err(ElastosError::Compute(format!(
                "SIOCSIFADDR failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn set_netmask(&self) -> Result<()> {
        let sock = udp_socket()?;
        let mask: std::net::Ipv4Addr = self
            .mask
            .parse()
            .map_err(|e| ElastosError::Compute(format!("Invalid mask '{}': {}", self.mask, e)))?;
        let mut ifr = Ifreq::new(&self.tap_name);
        ifr.set_addr(mask);
        if unsafe {
            ioctl_raw(
                sock.as_raw_fd(),
                SIOCSIFNETMASK,
                &ifr as *const Ifreq as *mut _,
            )
        } < 0
        {
            return Err(ElastosError::Compute(format!(
                "SIOCSIFNETMASK failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn bring_up(&self) -> Result<()> {
        let sock = udp_socket()?;
        let mut ifr = Ifreq::new(&self.tap_name);
        if unsafe {
            ioctl_raw(
                sock.as_raw_fd(),
                SIOCGIFFLAGS,
                &mut ifr as *mut Ifreq as *mut _,
            )
        } < 0
        {
            return Err(ElastosError::Compute(format!(
                "SIOCGIFFLAGS failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        ifr.set_flags(ifr.get_flags() | IFF_UP);
        if unsafe {
            ioctl_raw(
                sock.as_raw_fd(),
                SIOCSIFFLAGS,
                &ifr as *const Ifreq as *mut _,
            )
        } < 0
        {
            return Err(ElastosError::Compute(format!(
                "SIOCSIFFLAGS failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Generate a deterministic MAC address from a VM ID.
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

/// Map a VM ID to a subnet octet (1-250) for the /30 allocation.
pub fn subnet_octet_for_vm(vm_id: &str) -> u8 {
    let hash: u64 = vm_id
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(131).wrapping_add(b as u64));
    ((hash % 250) as u8) + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_network_config_new() {
        let config = NetworkConfig::new("test-vm-123");
        assert!(config.tap_name.starts_with("cv"));
        assert!(config.tap_name.len() <= 15);
        assert!(config.host_ip.starts_with("172.16."));
        assert!(config.host_ip.ends_with(".1"));
        assert!(config.guest_ip.ends_with(".2"));
        assert_eq!(config.prefix_len, 30);
    }

    #[test]
    fn test_different_vms_get_different_subnets() {
        let a = NetworkConfig::new("vm-a");
        let b = NetworkConfig::new("vm-b");
        assert_ne!(a.host_ip, b.host_ip);
    }

    #[test]
    fn test_mac_generation() {
        let mac = generate_mac("test");
        assert!(mac.starts_with("AA:FC:"));
        assert_eq!(mac.len(), 17);
    }

    #[test]
    fn test_clone_does_not_share_fd() {
        let config = NetworkConfig::new("test");
        config._tap_fd.store(42, Ordering::SeqCst);
        let cloned = config.clone();
        assert_eq!(cloned._tap_fd.load(Ordering::SeqCst), -1);
        config._tap_fd.store(-1, Ordering::SeqCst);
    }
}
