//! `VzMachineBuilder` — assembles a complete
//! `VZVirtualMachineConfiguration` from `VmConfig` + `VzConfig`.
//!
//! This is the **integration point** for every other module in
//! `ffi/`. Every Day-2 wrapper feeds into one of the
//! `VZVirtualMachineConfiguration::set*` array setters here.
//!
//! Day 2 ships only the **builder**; the result is not yet
//! handed to `VZVirtualMachine::initWithConfiguration:queue:`.
//! That integration is Day 3's lifecycle module — at which
//! point `provider.rs::load()` will stop returning the Phase 1
//! stub error and instead store the `BuiltMachine` inside the
//! running-VM map.
//!
//! Construction order is intentional: anything that returns an
//! `Err(String)` (filesystem / Vz attachment failures) is
//! evaluated **before** any device is attached to the
//! configuration, so a partial assembly never escapes this
//! function.

#![cfg(target_os = "macos")]

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2_foundation::NSArray;
use objc2_virtualization::{VZVirtioConsoleDeviceConfiguration, VZVirtualMachineConfiguration};

use crate::config::{VmConfig, VzConfig};

use super::balloon::build_balloon_device;
use super::block::build_block_device;
use super::boot_loader::build_boot_loader;
use super::console::{build_carrier_console_slot, build_kernel_console};
use super::entropy::build_entropy_device;
use super::network::build_nat_network;
use super::platform::build_platform;
use super::vsock::build_vsock_device;

/// Final product of the builder. Holds the Vz configuration
/// **and** every Rust-side handle that needs to outlive the
/// configuration (specifically the kernel-console read end, so
/// the lifecycle module can forward bytes to `tracing`).
#[derive(Debug)]
pub(crate) struct BuiltMachine {
    /// Configuration ready for `VZVirtualMachine::initWithConfiguration:queue:`.
    pub(crate) vz_config: Retained<VZVirtualMachineConfiguration>,

    /// Host-side read end of the kernel-console pipe. Phase 2
    /// Day 3 attaches a `tracing` forwarder; Phase 2 Day 2 just
    /// guarantees the handle is reachable and owned.
    #[allow(dead_code)]
    pub(crate) kernel_console_host_read: std::fs::File,

    /// Carrier multi-port console kept alive separately so
    /// Phase 3 can swap its placeholder attachment for a real
    /// socketpair without re-walking the whole configuration.
    /// (`VZVirtualMachineConfiguration` already retains it via
    /// `setConsoleDevices`, but holding our own `Retained` makes
    /// the Phase 3 patch point explicit.)
    #[allow(dead_code)]
    pub(crate) carrier_console: Retained<VZVirtioConsoleDeviceConfiguration>,

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
        // Phase 1: gather every fallible Rust- and Vz-side
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

        let kernel_console =
            build_kernel_console().map_err(|e| format!("vz machine builder: {e}"))?;

        let carrier_console = build_carrier_console_slot("elastos-carrier")
            .map_err(|e| format!("vz machine builder: {e}"))?;

        let vsock = build_vsock_device();
        let network = build_nat_network();
        let entropy = build_entropy_device();
        let balloon = build_balloon_device();

        // ------------------------------------------------------
        // Phase 2: assemble the VZVirtualMachineConfiguration.
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
        // bridge in Phase 3 negotiates per-connection.
        let socket_devices = NSArray::from_retained_slice(&[vsock.clone().into_super()]);
        unsafe { cfg.setSocketDevices(&socket_devices) };

        // Network — NAT only in Day 2.
        let network_devices = NSArray::from_retained_slice(&[network.clone().into_super()]);
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
        std::fs::write(&path, b"# placeholder kernel for tests\n").unwrap();
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
            vm_id: "phase2-day2-builder-test".to_string(),
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

        // The host-side fd must remain valid and owned by us.
        assert!(
            built.kernel_console_host_read.as_raw_fd() >= 0,
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
        // Day 5 contract: the `initramfs_path` field on `VmConfig`
        // must reach `VZLinuxBootLoader.initialRamdiskURL`, not
        // disappear into the builder. Without this gate, an
        // operator passing `--initramfs` would get a silent
        // `nil`-on-the-boot-loader and a guest kernel that panics
        // with "no working init found".
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let rootfs = write_fake_disk(tmp.path(), "rootfs.img");
        let initramfs = {
            let p = tmp.path().join("initramfs.img");
            std::fs::write(&p, b"# placeholder initramfs for tests\n").unwrap();
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
}
