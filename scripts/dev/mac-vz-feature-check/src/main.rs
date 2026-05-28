//! mac-vz-feature-check — Apple Virtualization.framework reality probe.
//!
//! Phase 2 Day 1 deliverable of [`docs/vz-backend/PLAN.md`](../../../docs/vz-backend/PLAN.md).
//!
//! Standalone developer-side diagnostic, **not** part of the runtime
//! build and **not** linked into `elastos-vz`. Its only job is to
//! convert every desk-research claim in
//! [`docs/vz-backend/PHASE_0_SCOPE.md`](../../../docs/vz-backend/PHASE_0_SCOPE.md)
//! §B (feature-coverage table) and §D (pitfalls) into runtime-verified
//! fact on this Mac before we write a single line of production Vz
//! code in `elastos-vz/src/ffi/`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --manifest-path scripts/dev/mac-vz-feature-check/Cargo.toml
//! ```
//!
//! # What it checks (anchored to Phase 0 §B)
//!
//! 1. Host environment — macOS version (`NSProcessInfo.operatingSystemVersion`),
//!    arch (Apple Silicon detection), `dispatch_queue` creation.
//! 2. `VZGenericMachineIdentifier` + round-trip `dataRepresentation` (§D #2).
//! 3. `VZGenericPlatformConfiguration` with the identifier attached.
//! 4. `VZLinuxBootLoader` with `console=hvc0` command line (§D #3).
//! 5. `VZDiskImageStorageDeviceAttachment` with `cachingMode=Cached`,
//!    `synchronizationMode=Fsync` (§D #1, UTM #4840).
//! 6. `VZVirtioBlockDeviceConfiguration` wrapping the attachment.
//! 7. Kernel-console `VZVirtioConsoleDeviceSerialPortConfiguration` over
//!    a `pipe()`-backed `VZFileHandleSerialPortAttachment` (§D #7).
//! 8. Multi-port `VZVirtioConsoleDeviceConfiguration` +
//!    `VZVirtioConsolePortConfigurationArray` (§D #4, macOS 12+).
//! 9. `VZVirtioSocketDeviceConfiguration` (§D #5, no CID API).
//! 10. `VZNATNetworkDeviceAttachment` + `VZVirtioNetworkDeviceConfiguration`
//!     with random locally-administered `VZMACAddress` (§B "no entitlement").
//! 11. `VZVirtioEntropyDeviceConfiguration`.
//! 12. `VZVirtioTraditionalMemoryBalloonDeviceConfiguration`.
//! 13. Full `VZVirtualMachineConfiguration` assembled from the above +
//!     `validateWithError(&self)`.
//!
//! # How to read the output
//!
//! Each probe reports `OK` or `FAIL: <reason>`. A FAIL on
//! `validate()` with message containing `entitlement` is **expected**
//! on unsigned dev builds (Apple requires
//! `com.apple.security.virtualization` for `VZVirtualMachineConfiguration`
//! to validate); this is the signal that Phase 6 must code-sign +
//! notarize. The binary's exit code is `0` if every device class is
//! constructible **and** the only `validate()` failure (if any) is the
//! entitlement gap. Any other failure is non-zero with a single-line
//! diagnostic.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "mac-vz-feature-check: not on macOS — this binary only runs on \
         Apple Silicon macOS 12.0+. Exiting with code 2 (not-applicable)."
    );
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod probes {
    use std::fs::OpenOptions;
    use std::os::fd::IntoRawFd;
    use std::os::unix::io::FromRawFd;

    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSError, NSFileHandle, NSProcessInfo, NSString, NSURL};
    use objc2_virtualization::{
        VZDiskImageCachingMode, VZDiskImageStorageDeviceAttachment, VZDiskImageSynchronizationMode,
        VZFileHandleSerialPortAttachment, VZGenericMachineIdentifier,
        VZGenericPlatformConfiguration, VZLinuxBootLoader, VZMACAddress,
        VZNATNetworkDeviceAttachment, VZVirtioBlockDeviceConfiguration,
        VZVirtioConsoleDeviceConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
        VZVirtioConsolePortConfiguration, VZVirtioConsolePortConfigurationArray,
        VZVirtioEntropyDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
        VZVirtioSocketDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
        VZVirtualMachineConfiguration,
    };
    use tempfile::TempDir;

    /// Per-probe result. A `Skip` is reserved for probes whose
    /// preconditions are unmet (e.g. multi-port virtio-console on
    /// macOS 11); they neither pass nor fail the binary's exit code.
    pub enum Status {
        Pass,
        Skip,
        Fail,
    }

    pub struct Probe {
        pub name: &'static str,
        pub status: Status,
        pub detail: String,
    }

    impl Probe {
        fn pass(name: &'static str, detail: impl Into<String>) -> Self {
            Self {
                name,
                status: Status::Pass,
                detail: detail.into(),
            }
        }

        fn skip(name: &'static str, detail: impl Into<String>) -> Self {
            Self {
                name,
                status: Status::Skip,
                detail: detail.into(),
            }
        }

        fn fail(name: &'static str, detail: impl Into<String>) -> Self {
            Self {
                name,
                status: Status::Fail,
                detail: detail.into(),
            }
        }
    }

    /// Run every probe and return the result list. The final entry is
    /// always the full-configuration `validate()` probe.
    pub fn run_all() -> Vec<Probe> {
        let tmp = match TempDir::new() {
            Ok(t) => t,
            Err(e) => {
                return vec![Probe::fail(
                    "host",
                    format!("could not create tempdir for probe fixtures: {e}"),
                )];
            }
        };

        let mut out = Vec::new();
        out.push(probe_host());
        out.push(probe_dispatch());

        let identifier = match probe_identifier() {
            (probe, Some(id)) => {
                out.push(probe);
                Some(id)
            }
            (probe, None) => {
                out.push(probe);
                None
            }
        };

        let platform = identifier.as_ref().and_then(|id| {
            let (probe, p) = probe_platform(id);
            out.push(probe);
            p
        });

        let boot_loader = {
            let (probe, bl) = probe_boot_loader(tmp.path());
            out.push(probe);
            bl
        };

        let storage = {
            let (probe, st) = probe_storage(tmp.path());
            out.push(probe);
            st
        };

        let kernel_console = {
            let (probe, kc) = probe_console();
            out.push(probe);
            kc
        };

        let console_mp = {
            let (probe, mp) = probe_console_multiport();
            out.push(probe);
            mp
        };

        let vsock = {
            let (probe, v) = probe_vsock();
            out.push(probe);
            v
        };

        let network = {
            let (probe, n) = probe_network();
            out.push(probe);
            n
        };

        let entropy = {
            let (probe, e) = probe_entropy();
            out.push(probe);
            e
        };

        let balloon = {
            let (probe, b) = probe_balloon();
            out.push(probe);
            b
        };

        out.push(probe_full_validate(
            platform,
            boot_loader,
            storage,
            kernel_console,
            console_mp,
            vsock,
            network,
            entropy,
            balloon,
        ));

        out
    }

    fn probe_host() -> Probe {
        let info = NSProcessInfo::processInfo();
        let v = info.operatingSystemVersion();
        let version_string = format!(
            "macOS {}.{}.{}",
            v.majorVersion, v.minorVersion, v.patchVersion
        );

        let arch = std::env::consts::ARCH;
        let apple_silicon = arch == "aarch64";

        let detail = if apple_silicon {
            format!("{version_string} / {arch} (Apple Silicon)")
        } else {
            format!(
                "{version_string} / {arch} — NOT Apple Silicon; \
                 elastos-vz Phase 6 ships aarch64-apple-darwin only"
            )
        };

        if apple_silicon {
            Probe::pass("host", detail)
        } else {
            Probe::fail("host", detail)
        }
    }

    fn probe_dispatch() -> Probe {
        // dispatch2 0.3 exposes `DispatchQueue::new(label, attr)`.
        // Passing `None` for the attribute yields the default serial
        // queue — exactly what Phase 0 §D pitfall #10 calls for
        // (one queue per VzProvider for delegate callbacks).
        let _q = dispatch2::DispatchQueue::new("vz-probe.queue", None);
        Probe::pass("dispatch", "serial GCD queue created (dispatch2)")
    }

    fn probe_identifier() -> (Probe, Option<Retained<VZGenericMachineIdentifier>>) {
        let id = unsafe { VZGenericMachineIdentifier::new() };
        let data = unsafe { id.dataRepresentation() };
        let len = data.length();
        if len == 0 {
            return (
                Probe::fail("identifier", "dataRepresentation returned 0 bytes"),
                None,
            );
        }
        (
            Probe::pass(
                "identifier",
                format!("VZGenericMachineIdentifier ({} bytes serialized)", len),
            ),
            Some(id),
        )
    }

    fn probe_platform(
        identifier: &VZGenericMachineIdentifier,
    ) -> (Probe, Option<Retained<VZGenericPlatformConfiguration>>) {
        let platform = unsafe { VZGenericPlatformConfiguration::new() };
        unsafe { platform.setMachineIdentifier(identifier) };
        (
            Probe::pass("platform", "VZGenericPlatformConfiguration + identifier"),
            Some(platform),
        )
    }

    fn probe_boot_loader(
        tmp_dir: &std::path::Path,
    ) -> (Probe, Option<Retained<VZLinuxBootLoader>>) {
        let fake_kernel = tmp_dir.join("fake-vmlinux");
        if let Err(e) = std::fs::write(&fake_kernel, b"# placeholder kernel for probe\n") {
            return (
                Probe::fail("boot-loader", format!("write fake kernel: {e}")),
                None,
            );
        }

        let path_str = match fake_kernel.to_str() {
            Some(s) => s,
            None => {
                return (
                    Probe::fail("boot-loader", "tempdir path contains invalid UTF-8"),
                    None,
                );
            }
        };

        let ns_path = NSString::from_str(path_str);
        let url = NSURL::fileURLWithPath(&ns_path);

        let bl = unsafe { VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &url) };

        // Phase 0 §D pitfall #3: Vz boots silently without `console=hvc0`.
        let cmdline =
            NSString::from_str("console=hvc0 reboot=k panic=1 init=/init random.trust_cpu=on");
        unsafe { bl.setCommandLine(&cmdline) };

        (
            Probe::pass(
                "boot-loader",
                format!(
                    "VZLinuxBootLoader kernelURL={} commandLine=console=hvc0 ...",
                    path_str
                ),
            ),
            Some(bl),
        )
    }

    fn probe_storage(
        tmp_dir: &std::path::Path,
    ) -> (Probe, Option<Retained<VZVirtioBlockDeviceConfiguration>>) {
        // Vz requires a regular file whose length is a 512-byte
        // multiple. Make a 1 MiB temp file.
        let disk_path = tmp_dir.join("fake-rootfs.img");
        let f = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&disk_path)
        {
            Ok(f) => f,
            Err(e) => {
                return (
                    Probe::fail("storage", format!("create fake disk: {e}")),
                    None,
                );
            }
        };
        if let Err(e) = f.set_len(1024 * 1024) {
            return (
                Probe::fail("storage", format!("truncate to 1 MiB: {e}")),
                None,
            );
        }
        drop(f);

        let path_str = match disk_path.to_str() {
            Some(s) => s,
            None => {
                return (
                    Probe::fail("storage", "tempdir path contains invalid UTF-8"),
                    None,
                );
            }
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));

        // Phase 0 §D pitfall #1: caching=Cached avoids UTM #4840 corruption.
        // synchronization=Fsync mirrors Lima vm_darwin.go L495.
        let attachment_res = unsafe {
            VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &url,
                false,
                VZDiskImageCachingMode::Cached,
                VZDiskImageSynchronizationMode::Fsync,
            )
        };

        let attachment = match attachment_res {
            Ok(a) => a,
            Err(e) => {
                return (
                    Probe::fail(
                        "storage",
                        format!("attachment init: {}", ns_error_string(&e)),
                    ),
                    None,
                );
            }
        };

        let block = unsafe {
            VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attachment,
            )
        };

        (
            Probe::pass(
                "storage",
                "VZVirtioBlockDeviceConfiguration (cachingMode=Cached, synchronizationMode=Fsync)",
            ),
            Some(block),
        )
    }

    /// Build a `VZFileHandleSerialPortAttachment` over a pair of OS
    /// pipes (Phase 0 §D pitfall #7: avoid file-backed console growth).
    /// Used twice — once for the kernel console probe, once again for
    /// the multi-port probe.
    fn pipe_backed_serial_attachment() -> Result<Retained<VZFileHandleSerialPortAttachment>, String>
    {
        // SAFETY: libc::pipe(fds) is the standard POSIX call; we own
        // the resulting file descriptors and pass ownership to
        // NSFileHandle.
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if rc != 0 {
            return Err(format!(
                "pipe() failed: errno={}",
                std::io::Error::last_os_error()
            ));
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);

        // SAFETY: read_fd / write_fd were just returned by pipe(2)
        // and have no other Rust owner.
        let reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let writer = unsafe { std::fs::File::from_raw_fd(write_fd) };

        let reader_fd = reader.into_raw_fd();
        let writer_fd = writer.into_raw_fd();

        // NSFileHandle takes ownership of the fd via
        // `closeOnDealloc:YES`. Once initialised, NSFileHandle owns
        // the fd and the OS will close it when the handle drops.
        let r_handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
            NSFileHandle::alloc(),
            reader_fd,
            true,
        );
        let w_handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
            NSFileHandle::alloc(),
            writer_fd,
            true,
        );

        let attachment = unsafe {
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                Some(&r_handle),
                Some(&w_handle),
            )
        };
        Ok(attachment)
    }

    fn probe_console() -> (
        Probe,
        Option<Retained<VZVirtioConsoleDeviceSerialPortConfiguration>>,
    ) {
        let attachment = match pipe_backed_serial_attachment() {
            Ok(a) => a,
            Err(e) => return (Probe::fail("console", e), None),
        };
        let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
        unsafe { serial.setAttachment(Some(&attachment)) };
        (
            Probe::pass(
                "console",
                "VZVirtioConsoleDeviceSerialPortConfiguration via pipe-backed FileHandle",
            ),
            Some(serial),
        )
    }

    fn probe_console_multiport() -> (Probe, Option<Retained<VZVirtioConsoleDeviceConfiguration>>) {
        // Phase 0 §D pitfall #4: multi-port virtio-console = macOS 12+.
        // Construct a single-port configuration array as the Carrier
        // bridge slot Phase 3 will fill in. VZVirtioConsoleDevice owns
        // the port array via `ports()`; we mutate the array in place
        // through Apple's subscript setter.
        let attachment = match pipe_backed_serial_attachment() {
            Ok(a) => a,
            Err(e) => return (Probe::fail("console-mp", e), None),
        };

        let port = unsafe { VZVirtioConsolePortConfiguration::new() };
        unsafe { port.setName(Some(&NSString::from_str("elastos-carrier-probe"))) };
        unsafe { port.setAttachment(Some(&attachment)) };

        let device = unsafe { VZVirtioConsoleDeviceConfiguration::new() };
        let array: Retained<VZVirtioConsolePortConfigurationArray> = unsafe { device.ports() };
        unsafe { array.setObject_atIndexedSubscript(Some(&port), 0) };

        (
            Probe::pass(
                "console-mp",
                "VZVirtioConsoleDeviceConfiguration with 1-port array (Carrier slot)",
            ),
            Some(device),
        )
    }

    fn probe_vsock() -> (Probe, Option<Retained<VZVirtioSocketDeviceConfiguration>>) {
        let cfg = unsafe { VZVirtioSocketDeviceConfiguration::new() };
        (
            Probe::pass(
                "vsock",
                "VZVirtioSocketDeviceConfiguration (no CID API — per-VM connection in Phase 3)",
            ),
            Some(cfg),
        )
    }

    fn probe_network() -> (Probe, Option<Retained<VZVirtioNetworkDeviceConfiguration>>) {
        let attachment = unsafe { VZNATNetworkDeviceAttachment::new() };
        let mac = unsafe { VZMACAddress::randomLocallyAdministeredAddress() };

        let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
        unsafe { net.setAttachment(Some(&attachment)) };
        unsafe { net.setMACAddress(&mac) };

        let mac_string = unsafe { mac.string() }.to_string();
        (
            Probe::pass(
                "network",
                format!("VZNATNetworkDeviceAttachment + virtio-net (mac={mac_string})"),
            ),
            Some(net),
        )
    }

    fn probe_entropy() -> (Probe, Option<Retained<VZVirtioEntropyDeviceConfiguration>>) {
        let cfg = unsafe { VZVirtioEntropyDeviceConfiguration::new() };
        (
            Probe::pass("entropy", "VZVirtioEntropyDeviceConfiguration"),
            Some(cfg),
        )
    }

    fn probe_balloon() -> (
        Probe,
        Option<Retained<VZVirtioTraditionalMemoryBalloonDeviceConfiguration>>,
    ) {
        let cfg = unsafe { VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new() };
        (
            Probe::pass(
                "balloon",
                "VZVirtioTraditionalMemoryBalloonDeviceConfiguration",
            ),
            Some(cfg),
        )
    }

    /// Assemble all constructible device classes into a
    /// `VZVirtualMachineConfiguration` (1 vCPU, 128 MiB) and call
    /// `validateWithError`. Reports the result.
    ///
    /// Important interpretation: an unsigned dev binary will report a
    /// failure containing "entitlement". That is **expected**; Phase 6
    /// adds code-signing + `com.apple.security.virtualization` to fix
    /// it. The caller treats that case specifically when deciding the
    /// process exit code.
    #[allow(clippy::too_many_arguments)]
    fn probe_full_validate(
        platform: Option<Retained<VZGenericPlatformConfiguration>>,
        boot_loader: Option<Retained<VZLinuxBootLoader>>,
        storage: Option<Retained<VZVirtioBlockDeviceConfiguration>>,
        kernel_console: Option<Retained<VZVirtioConsoleDeviceSerialPortConfiguration>>,
        console_mp: Option<Retained<VZVirtioConsoleDeviceConfiguration>>,
        vsock: Option<Retained<VZVirtioSocketDeviceConfiguration>>,
        network: Option<Retained<VZVirtioNetworkDeviceConfiguration>>,
        entropy: Option<Retained<VZVirtioEntropyDeviceConfiguration>>,
        balloon: Option<Retained<VZVirtioTraditionalMemoryBalloonDeviceConfiguration>>,
    ) -> Probe {
        let (Some(platform), Some(boot_loader)) = (platform.as_ref(), boot_loader.as_ref()) else {
            return Probe::fail(
                "validate",
                "skipped — prerequisite probe(s) failed (platform/bootLoader missing)",
            );
        };

        let cfg = unsafe { VZVirtualMachineConfiguration::new() };
        unsafe { cfg.setCPUCount(1) };
        unsafe { cfg.setMemorySize(128 * 1024 * 1024) };
        unsafe { cfg.setPlatform(platform) };
        unsafe { cfg.setBootLoader(Some(boot_loader)) };

        if let Some(s) = storage.as_ref() {
            let arr: Retained<NSArray<_>> = NSArray::from_retained_slice(&[s.clone().into_super()]);
            unsafe { cfg.setStorageDevices(&arr) };
        }
        if let Some(kc) = kernel_console.as_ref() {
            let arr: Retained<NSArray<_>> =
                NSArray::from_retained_slice(&[kc.clone().into_super()]);
            unsafe { cfg.setSerialPorts(&arr) };
        }
        if let Some(mp) = console_mp.as_ref() {
            let arr: Retained<NSArray<_>> =
                NSArray::from_retained_slice(&[mp.clone().into_super()]);
            unsafe { cfg.setConsoleDevices(&arr) };
        }
        if let Some(v) = vsock.as_ref() {
            let arr: Retained<NSArray<_>> = NSArray::from_retained_slice(&[v.clone().into_super()]);
            unsafe { cfg.setSocketDevices(&arr) };
        }
        if let Some(n) = network.as_ref() {
            let arr: Retained<NSArray<_>> = NSArray::from_retained_slice(&[n.clone().into_super()]);
            unsafe { cfg.setNetworkDevices(&arr) };
        }
        if let Some(e) = entropy.as_ref() {
            let arr: Retained<NSArray<_>> = NSArray::from_retained_slice(&[e.clone().into_super()]);
            unsafe { cfg.setEntropyDevices(&arr) };
        }
        if let Some(b) = balloon.as_ref() {
            let arr: Retained<NSArray<_>> = NSArray::from_retained_slice(&[b.clone().into_super()]);
            unsafe { cfg.setMemoryBalloonDevices(&arr) };
        }

        match unsafe { cfg.validateWithError() } {
            Ok(()) => Probe::pass(
                "validate",
                "VZVirtualMachineConfiguration.validateWithError -> OK",
            ),
            Err(e) => {
                let msg = ns_error_string(&e);
                if msg.to_lowercase().contains("entitlement") {
                    // Phase 6 known-issue. Recorded as Skip so the
                    // exit code stays 0 — every device class still
                    // constructed cleanly, which is the Phase 2 Day 1
                    // contract.
                    Probe::skip(
                        "validate",
                        format!(
                            "missing com.apple.security.virtualization entitlement \
                             (expected for unsigned dev builds; Phase 6 code-signs). \
                             Apple error: {msg}"
                        ),
                    )
                } else {
                    Probe::fail("validate", format!("validateWithError: {msg}"))
                }
            }
        }
    }

    fn ns_error_string(err: &NSError) -> String {
        err.localizedDescription().to_string()
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use probes::Status;

    let banner = format!(
        "mac-vz-feature-check {} (Phase 2 Day 1)",
        env!("CARGO_PKG_VERSION")
    );
    println!("{banner}");
    println!("{}", "=".repeat(banner.len()));

    let probes = probes::run_all();

    let mut had_fail = false;
    for probe in &probes {
        let (label, body) = match probe.status {
            Status::Pass => ("OK", probe.detail.clone()),
            Status::Skip => ("SKIP", probe.detail.clone()),
            Status::Fail => {
                had_fail = true;
                ("FAIL", probe.detail.clone())
            }
        };
        println!("{:<12}: {:<4} — {}", probe.name, label, body);
    }
    println!("{}", "=".repeat(banner.len()));

    if had_fail {
        eprintln!(
            "PHASE_2_DAY_1: AT LEAST ONE PROBE FAILED — see lines above. \
             Adjust docs/vz-backend/PHASE_0_SCOPE.md before opening Phase 2 main."
        );
        std::process::exit(1);
    }

    println!("PHASE_2_DAY_1: ALL DEVICE CLASSES CONSTRUCTIBLE. Phase 2 main may proceed.");
}
