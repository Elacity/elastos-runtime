//! `VzMachineBuilder` — assembles a complete
//! `VZVirtualMachineConfiguration` from `VmConfig` + `VzConfig`.
//!
//! This is the **integration point** for every other module in
//! `ffi/`. Every device wrapper feeds into one of the
//! `VZVirtualMachineConfiguration::set*` array setters here. The
//! lifecycle module consumes the resulting `BuiltMachine`.
//!
//! Construction order is intentional: anything that returns an
//! `Err(String)` (filesystem / Vz attachment failures) is
//! evaluated **before** any device is attached to the
//! configuration, so a partial assembly never escapes this
//! function.

#![cfg(target_os = "macos")]

use std::os::fd::OwnedFd;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2_foundation::NSArray;
use objc2_virtualization::{VZVirtioConsoleDeviceConfiguration, VZVirtualMachineConfiguration};

use crate::config::{VmConfig, VzConfig};

use super::balloon::build_balloon_device;
use super::block::build_block_device;
use super::boot_loader::build_boot_loader;
use super::console::{build_carrier_console_slot, build_kernel_console};
use super::entitlement::has_vm_networking_entitlement;
use super::entropy::build_entropy_device;
use super::network::{build_bridged_network, build_nat_network};
use super::platform::build_platform;
use super::vsock::build_vsock_device;

/// Final product of the builder. Holds the Vz configuration
/// **and** every Rust-side handle that needs to outlive the
/// configuration (specifically the kernel-console read end, so
/// the lifecycle module can forward bytes to the logger).
#[derive(Debug)]
pub(crate) struct BuiltMachine {
    /// Configuration ready for `VZVirtualMachine::initWithConfiguration:queue:`.
    pub(crate) vz_config: Retained<VZVirtualMachineConfiguration>,

    /// Host-owned read end of the pipe-backed kernel console, or
    /// `None` when the VM was built with `interactive_stdio=true`
    /// (interactive stdio wires Vz directly to host stdio
    /// and there's no in-process pipe to forward).
    #[allow(dead_code)]
    pub(crate) kernel_console_host_read: Option<std::fs::File>,

    /// Carrier multi-port console kept alive separately so
    /// the bridge can be re-attached without re-walking the
    /// whole configuration.
    /// (`VZVirtualMachineConfiguration` already retains it via
    /// `setConsoleDevices`, but holding our own `Retained`
    /// keeps the lifecycle explicit.)
    #[allow(dead_code)]
    pub(crate) carrier_console: Retained<VZVirtioConsoleDeviceConfiguration>,

    /// Host-side endpoint of the Carrier console
    /// `socketpair(AF_UNIX, SOCK_STREAM)`.
    /// Already configured non-blocking — the supervisor wraps
    /// it in `tokio::net::UnixStream::from_std` and feeds it to
    /// the Carrier bridge dispatch loop. The Vz-side fd lives
    /// inside the `carrier_console` attachment above.
    pub(crate) carrier_host_fd: OwnedFd,

    /// On-disk identifier path used for this VM, for log/UX use.
    #[allow(dead_code)]
    pub(crate) identifier_path: PathBuf,
}

impl BuiltMachine {
    /// Build a complete `VZVirtualMachineConfiguration` from
    /// `vm` + provider-wide `provider_config`.
    pub(crate) fn from_vm_config(
        vm: &VmConfig,
        provider_config: &VzConfig,
    ) -> Result<Self, String> {
        // ------------------------------------------------------
        // gather every fallible Rust- and Vz-side
        // resource. If any step errors, no device has been
        // attached to a configuration object yet.
        // ------------------------------------------------------

        let platform = build_platform(&provider_config.state_dir, &vm.vm_id)
            .map_err(|e| format!("vz machine builder: {e}"))?;

        let boot_loader = build_boot_loader(
            &vm.kernel_path,
            &vm.vz_boot_args(),
            vm.initramfs_path.as_deref(),
        )
        .map_err(|e| format!("vz machine builder: {e}"))?;

        let rootfs = build_block_device(&vm.rootfs_path, vm.rootfs_readonly)
            .map_err(|e| format!("vz machine builder: {e}"))?;

        let data_disk = match vm.data_disk_path.as_ref() {
            Some(path) => Some(
                build_block_device(path, false)
                    .map_err(|e| format!("vz machine builder (data disk): {e}"))?,
            ),
            None => None,
        };

        let kernel_console = build_kernel_console(vm.interactive_stdio)
            .map_err(|e| format!("vz machine builder: {e}"))?;

        let carrier = build_carrier_console_slot("elastos-carrier")
            .map_err(|e| format!("vz machine builder: {e}"))?;
        let carrier_console = carrier.device;
        let carrier_host_fd = carrier.host_fd;

        let vsock = build_vsock_device();
        // network device selection.
        //
        // - `vm.network = None` → NAT (no entitlement needed).
        // - `vm.network = Some(_)` + entitlement granted →
        //   bridged attachment, deterministic MAC from
        //   `NetworkConfig.guest_mac`.
        // - `vm.network = Some(_)` + entitlement missing →
        //   typed fail-closed. NO silent NAT downgrade — the
        //   capsule explicitly asked for routable networking
        //   and must either get it or be told why it can't.
        if vm.network_disabled && vm.network.is_some() {
            return Err(
                "vz machine builder: network_disabled conflicts with guest_network".to_string(),
            );
        }
        let network = if vm.network_disabled {
            None
        } else {
            Some(match vm.network.as_ref() {
                None => build_nat_network(),
                Some(net_cfg) => {
                    if !has_vm_networking_entitlement() {
                        return Err(format!(
                            "vz machine builder: capsule '{}' requested guest_network (bridged \
                         attachment) but this binary lacks the `com.apple.vm.networking` Apple \
                         entitlement. Drop `permissions.guest_network` from the manifest, OR \
                         install the signed dev build that carries the entitlement. See \
                         docs/MAC.md.",
                            vm.vm_id
                        ));
                    }
                    build_bridged_network(net_cfg)
                        .map_err(|e| format!("vz machine builder: {e}"))?
                }
            })
        };
        let entropy = build_entropy_device();
        let balloon = build_balloon_device();

        // ------------------------------------------------------
        // assemble the VZVirtualMachineConfiguration.
        // ------------------------------------------------------

        // SAFETY: `VZVirtualMachineConfiguration::new()` is the
        // standard objc2 allocator + init. All set* calls below
        // are documented as thread-safe before the configuration
        // is handed to a VZVirtualMachine.
        let cfg = unsafe { VZVirtualMachineConfiguration::new() };

        unsafe { cfg.setCPUCount(vm.vcpu_count as usize) };
        unsafe { cfg.setMemorySize(u64::from(vm.mem_size_mib) * 1024 * 1024) };
        unsafe { cfg.setPlatform(&platform.config) };
        unsafe { cfg.setBootLoader(Some(&boot_loader)) };

        // Storage: rootfs is mandatory; data disk is optional.
        let storage = match data_disk.as_ref() {
            Some(d) => {
                NSArray::from_retained_slice(&[rootfs.clone().into_super(), d.clone().into_super()])
            }
            None => NSArray::from_retained_slice(&[rootfs.clone().into_super()]),
        };
        unsafe { cfg.setStorageDevices(&storage) };

        // Serial port = kernel console (`/dev/hvc0` inside the
        // guest because of the virtio-console serial port class
        // wrapper).
        let serial_ports =
            NSArray::from_retained_slice(&[kernel_console.serial_port_cfg.clone().into_super()]);
        unsafe { cfg.setSerialPorts(&serial_ports) };

        // Console device = multi-port carrier bridge slot
        // (`/dev/hvc1` inside the guest).
        let console_devices = NSArray::from_retained_slice(&[carrier_console.clone().into_super()]);
        unsafe { cfg.setConsoleDevices(&console_devices) };

        // Vsock — Apple does not let us set a CID; the supervisor
        // bridge negotiates per connection.
        let socket_devices = NSArray::from_retained_slice(&[vsock.clone().into_super()]);
        unsafe { cfg.setSocketDevices(&socket_devices) };

        // Network — explicitly empty for no-NIC VMs, otherwise NAT by default
        // or bridged when explicitly configured.
        let network_devices = match network.as_ref() {
            Some(network) => NSArray::from_retained_slice(&[network.clone().into_super()]),
            None => NSArray::new(),
        };
        unsafe { cfg.setNetworkDevices(&network_devices) };

        // Entropy.
        let entropy_devices = NSArray::from_retained_slice(&[entropy.clone().into_super()]);
        unsafe { cfg.setEntropyDevices(&entropy_devices) };

        // Memory balloon.
        let balloon_devices = NSArray::from_retained_slice(&[balloon.clone().into_super()]);
        unsafe { cfg.setMemoryBalloonDevices(&balloon_devices) };

        Ok(Self {
            vz_config: cfg,
            kernel_console_host_read: kernel_console.host_read,
            carrier_console,
            carrier_host_fd,
            identifier_path: platform.identifier_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// 1 MiB — minimum Vz-compatible block device size.
    const FAKE_DISK_SIZE: u64 = 1024 * 1024;

    /// Smallest possible kernel-shaped file that gets through the
    /// builder's `build_boot_loader` existence check. The
    /// builder does NOT re-validate the kernel format — that's
    /// `VzConfig::validate` upstream.
    fn write_fake_kernel(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("vmlinux");
        std::fs::write(&path, b"# test kernel\n").unwrap();
        path
    }

    fn write_fake_disk(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.set_len(FAKE_DISK_SIZE).unwrap();
        path
    }

    fn make_vm_config(kernel: &std::path::Path, rootfs: &std::path::Path) -> VmConfig {
        VmConfig {
            vm_id: "vz-builder-test".to_string(),
            kernel_path: kernel.to_path_buf(),
            boot_args: "console=hvc0 reboot=k panic=1 init=/init random.trust_cpu=on".to_string(),
            rootfs_path: rootfs.to_path_buf(),
            rootfs_readonly: false,
            mem_size_mib: 128,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            network_disabled: false,
            interactive_stdio: false,
            carrier_socket_path: None,
            initramfs_path: None,
        }
    }

    fn make_vz_config(state_dir: PathBuf, kernel: PathBuf) -> VzConfig {
        VzConfig::new()
            .with_state_dir(state_dir)
            .with_kernel_path(kernel)
    }

    #[test]
    fn from_vm_config_builds_a_full_configuration() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");
        let state_dir = tmp.path().join("vz-state");

        let vm = make_vm_config(&kernel, &rootfs);
        let provider = make_vz_config(state_dir.clone(), kernel.clone());

        let built =
            BuiltMachine::from_vm_config(&vm, &provider).expect("builder succeeds with fixtures");

        // The host-side fd must remain valid and owned by us in
        // the non-interactive (pipe-backed) build path.
        let host_read = built
            .kernel_console_host_read
            .as_ref()
            .expect("pipe-backed build path must yield a kernel_console_host_read");
        assert!(
            host_read.as_raw_fd() >= 0,
            "kernel-console host fd must be live after build"
        );
        // Identifier must have been persisted under the state dir.
        assert!(
            built.identifier_path.is_file(),
            "machine identifier file expected at {}",
            built.identifier_path.display()
        );
        assert!(built.identifier_path.starts_with(&state_dir));

        // Spot-check resource sizing made it through.
        let cpus = unsafe { built.vz_config.CPUCount() };
        let mem = unsafe { built.vz_config.memorySize() };
        assert_eq!(cpus, 1);
        assert_eq!(mem, 128 * 1024 * 1024);
    }

    #[test]
    fn from_vm_config_with_data_disk_attaches_two_block_devices() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");
        let data = write_fake_disk(tmp.path(), "data.img");

        let mut vm = make_vm_config(&kernel, &rootfs);
        vm.data_disk_path = Some(data);

        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());
        let built = BuiltMachine::from_vm_config(&vm, &provider)
            .expect("builder succeeds with two block devices");

        let storage = unsafe { built.vz_config.storageDevices() };
        assert_eq!(
            storage.count(),
            2,
            "expected rootfs + data disk; got {} storage device(s)",
            storage.count()
        );
    }

    #[test]
    fn from_vm_config_fails_closed_when_rootfs_missing() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let bogus_rootfs = tmp.path().join("does-not-exist.img");

        let vm = make_vm_config(&kernel, &bogus_rootfs);
        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());

        let err = BuiltMachine::from_vm_config(&vm, &provider).unwrap_err();
        assert!(
            err.contains("not found") || err.contains("does not exist"),
            "expected typed not-found error, got: {err}"
        );
    }

    #[test]
    fn from_vm_config_with_initramfs_threads_through_to_boot_loader() {
        // The `initramfs_path` field on `VmConfig` must reach
        // `VZLinuxBootLoader.initialRamdiskURL`, not
        // disappear into the builder. Without this gate, an
        // operator passing `--initramfs` would get a silent
        // `nil`-on-the-boot-loader and a guest kernel that panics
        // with "no working init found".
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");
        let initramfs = {
            let p = tmp.path().join("initramfs.img");
            std::fs::write(&p, b"# test initramfs\n").unwrap();
            p
        };

        let mut vm = make_vm_config(&kernel, &rootfs);
        vm.initramfs_path = Some(initramfs.clone());

        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());
        let built =
            BuiltMachine::from_vm_config(&vm, &provider).expect("builder succeeds with initramfs");

        let boot_loader = unsafe { built.vz_config.bootLoader() }
            .expect("configuration has a boot loader after build");
        // Downcast the VZBootLoader to VZLinuxBootLoader so we can
        // read its initramfs URL. We pre-condition on the only
        // boot-loader class the builder constructs (Linux). If
        // Apple ever returns nil or a different subclass here, the
        // test correctly fails.
        let linux_bl: Retained<objc2_virtualization::VZLinuxBootLoader> = boot_loader
            .downcast()
            .expect("builder must produce a VZLinuxBootLoader");
        let stored = unsafe { linux_bl.initialRamdiskURL() }
            .expect("initialRamdiskURL must be set when VmConfig.initramfs_path is Some");
        let stored_path = stored
            .path()
            .expect("file URL must round-trip back to a path")
            .to_string();
        assert!(
            stored_path.ends_with("initramfs.img"),
            "expected initramfs.img tail in stored URL, got: {stored_path}"
        );
    }

    #[test]
    fn from_vm_config_attaches_every_required_device_class() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");

        let vm = make_vm_config(&kernel, &rootfs);
        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());
        let built = BuiltMachine::from_vm_config(&vm, &provider).unwrap();

        let cfg = &built.vz_config;
        assert_eq!(unsafe { cfg.storageDevices() }.count(), 1);
        assert_eq!(unsafe { cfg.serialPorts() }.count(), 1);
        assert_eq!(unsafe { cfg.consoleDevices() }.count(), 1);
        assert_eq!(unsafe { cfg.socketDevices() }.count(), 1);
        assert_eq!(unsafe { cfg.networkDevices() }.count(), 1);
        assert_eq!(unsafe { cfg.entropyDevices() }.count(), 1);
        assert_eq!(unsafe { cfg.memoryBalloonDevices() }.count(), 1);
    }

    #[test]
    fn from_vm_config_can_attach_zero_network_devices() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");
        let mut vm = make_vm_config(&kernel, &rootfs);
        vm.network_disabled = true;
        let provider = make_vz_config(tmp.path().join("vz-state"), kernel);

        let built = BuiltMachine::from_vm_config(&vm, &provider).unwrap();

        assert_eq!(unsafe { built.vz_config.networkDevices() }.count(), 0);
    }

    // --------------------------------------------------------------
    // guest_network entitlement gating
    // --------------------------------------------------------------

    /// When the capsule asks for bridged networking but the
    /// binary lacks `com.apple.vm.networking`, the builder
    /// must fail closed with a typed message naming both the
    /// entitlement and the manifest field. NO silent NAT
    /// downgrade — capsule asked for routable networking and
    /// must be told why it can't have it.
    #[test]
    fn builder_surfaces_typed_error_when_entitlement_absent_and_network_requested() {
        use crate::ffi::entitlement::override_for_testing;
        use crate::network::NetworkConfig;

        let _guard = override_for_testing(false);

        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");

        let mut vm = make_vm_config(&kernel, &rootfs);
        vm.network = Some(NetworkConfig::new(&vm.vm_id));

        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());
        let err = BuiltMachine::from_vm_config(&vm, &provider)
            .expect_err("builder must reject vm.network = Some(_) when entitlement is absent");

        assert!(
            err.contains("com.apple.vm.networking"),
            "expected entitlement name in error, got: {err}"
        );
        assert!(
            err.contains("guest_network"),
            "expected manifest field name in error, got: {err}"
        );
        // The capsule's vm_id (or the manifest entry that
        // produced it) must surface so operators can trace
        // which capsule was rejected.
        assert!(
            err.contains(&vm.vm_id),
            "expected vm_id in error, got: {err}"
        );
    }

    /// When the entitlement IS granted (override-true), the
    /// builder must produce a configuration with exactly one
    /// network device — same shape as the NAT path. The
    /// underlying attachment is `VZBridgedNetworkDeviceAttachment`
    /// instead of `VZNATNetworkDeviceAttachment`; we don't
    /// downcast in this test because Apple's class hierarchy
    /// doesn't expose a public discriminant on
    /// `VZNetworkDeviceConfiguration` and the device-count
    /// invariant is the contract that matters for the
    /// `VZVirtualMachineConfiguration::validate` path.
    ///
    /// Note: on a host with no bridge-capable interfaces (rare),
    /// this test surfaces the `no host interface available`
    /// error from `pick_first_bridged_interface` — also a
    /// correct fail-closed. We accept either Ok or that
    /// specific error.
    #[test]
    fn builder_attaches_bridged_attachment_when_entitlement_present() {
        use crate::ffi::entitlement::override_for_testing;
        use crate::network::NetworkConfig;

        let _guard = override_for_testing(true);

        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");

        let mut vm = make_vm_config(&kernel, &rootfs);
        vm.network = Some(NetworkConfig::new(&vm.vm_id));

        let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());
        match BuiltMachine::from_vm_config(&vm, &provider) {
            Ok(built) => {
                let net_devices = unsafe { built.vz_config.networkDevices() };
                assert_eq!(
                    net_devices.count(),
                    1,
                    "expected exactly one network device on the bridged path"
                );
            }
            Err(e) => {
                // The only acceptable error here is the
                // "no host interface" surface; the entitlement
                // gate already passed via override.
                assert!(
                    e.contains("no host interface"),
                    "unexpected builder error on entitlement-present path: {e}"
                );
            }
        }
    }

    /// Defensive: when the capsule does NOT request
    /// `guest_network`, the entitlement state must be
    /// irrelevant. Whether the override says yes or no, the
    /// builder produces the NAT-attached configuration and
    /// never consults the bridged path. Guards against an
    /// accidental "entitlement controls all networking"
    /// regression.
    #[test]
    fn builder_ignores_entitlement_when_capsule_uses_nat_only() {
        use crate::ffi::entitlement::override_for_testing;

        for override_value in [false, true] {
            let _guard = override_for_testing(override_value);

            let tmp = TempDir::new().unwrap();
            let kernel = write_fake_kernel(tmp.path());
            let rootfs = write_fake_disk(tmp.path(), "rootfs.img");

            let vm = make_vm_config(&kernel, &rootfs); // .network = None
            let provider = make_vz_config(tmp.path().join("vz-state"), kernel.clone());

            let built = BuiltMachine::from_vm_config(&vm, &provider)
                .expect("NAT-only capsule must succeed regardless of entitlement state");
            assert_eq!(unsafe { built.vz_config.networkDevices() }.count(), 1);
        }
    }
}
