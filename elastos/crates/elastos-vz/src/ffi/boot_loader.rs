//! `VZLinuxBootLoader` wrapper.
//!
//! Vz's only Linux boot path on Apple Silicon: feed it the kernel
//! `Image` (ARM64 raw) and the command line; optionally an
//! `initramfs`. Phase 0 §B row 1 + §D pitfall #3 anchor every
//! decision in this module:
//!
//! - Pitfall #3: Vz exposes the kernel console **only** as
//!   virtio-console; `console=ttyS0` produces a silent boot. The
//!   caller must hand us a command line that includes
//!   `console=hvc0`. `VmConfig::vz_boot_args()` already enforces
//!   this; we re-assert it via a debug-only check so a misuse
//!   doesn't slip through.
//! - Pitfall #6: initrd compression matters but is the caller's
//!   problem — Vz reads whatever bytes the file contains. We
//!   accept the path verbatim.
//!
//! Day 1 reality probe verified `VZLinuxBootLoader::initWithKernelURL`
//! + `setCommandLine` build cleanly with the runtime objc2 0.6 API.
//!
//! Day 5 wires `setInitialRamdiskURL:` for distro kernels that
//! require an initramfs to reach `/sbin/init` — every Ubuntu /
//! Debian / Alpine arm64 cloud-image kernel we know of depends on
//! one.

#![cfg(target_os = "macos")]

use std::path::Path;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::VZLinuxBootLoader;

/// Build a `VZLinuxBootLoader` for the given kernel path,
/// command line, and optional initial ramdisk.
///
/// Returns `Err(String)` only for inputs that would corrupt the
/// configuration — non-existent kernel file, non-existent initramfs
/// when one was supplied, or invalid UTF-8 paths. All Vz-side
/// failures (kernel format, command-line length, initramfs format)
/// are reported by `VZVirtualMachineConfiguration::validate()`, not
/// here.
///
/// Passing `initramfs_path = None` leaves Vz's `initialRamdiskURL`
/// at its default `nil`, which is the correct boot path for
/// kernels with a baked-in initramfs (rare on stock distro kernels
/// for arm64 but common for hand-built kernels).
///
/// The caller owns the returned `Retained<VZLinuxBootLoader>` and
/// is expected to hand it to `VZVirtualMachineConfiguration::setBootLoader`.
pub(crate) fn build_boot_loader(
    kernel_path: &Path,
    command_line: &str,
    initramfs_path: Option<&Path>,
) -> Result<Retained<VZLinuxBootLoader>, String> {
    if !kernel_path.exists() {
        return Err(format!(
            "boot loader: kernel file does not exist at {}",
            kernel_path.display()
        ));
    }

    let path_str = kernel_path.to_str().ok_or_else(|| {
        format!(
            "boot loader: kernel path is not valid UTF-8 ({})",
            kernel_path.display()
        )
    })?;

    // Validate the initramfs path BEFORE any Vz object is built so
    // a misconfigured invocation doesn't leave a half-assembled
    // boot loader behind. The matching contract for kernels is to
    // surface "does not exist" exactly — that's what the
    // `build_boot_loader_rejects_missing_kernel` test asserts —
    // so we keep the same shape here.
    let initramfs_path_str: Option<&str> = if let Some(path) = initramfs_path {
        if !path.exists() {
            return Err(format!(
                "boot loader: initramfs file does not exist at {}",
                path.display()
            ));
        }
        Some(path.to_str().ok_or_else(|| {
            format!(
                "boot loader: initramfs path is not valid UTF-8 ({})",
                path.display()
            )
        })?)
    } else {
        None
    };

    debug_assert!(
        command_line.contains("console=hvc0"),
        "boot loader command line must use console=hvc0 on Vz (got: {command_line})",
    );

    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));

    // SAFETY: `VZLinuxBootLoader::alloc` is the standard objc2
    // `AnyThread::alloc()` (uninitialised instance); we hand it to
    // `initWithKernelURL:` immediately. The URL has been freshly
    // created from a valid UTF-8 path on the local filesystem.
    let bl = unsafe { VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &url) };

    // SAFETY: `setCommandLine:` is a copy-property setter; Apple's
    // doc guarantees it copies the NSString contents and does not
    // retain the Rust-side reference.
    let cmdline_ns = NSString::from_str(command_line);
    unsafe { bl.setCommandLine(&cmdline_ns) };

    if let Some(initramfs_str) = initramfs_path_str {
        // SAFETY: `setInitialRamdiskURL:` is a copy-property setter
        // accepting an optional `NSURL`. We pass `Some(&url)` for a
        // fresh URL built from a validated UTF-8 path on the local
        // filesystem; Vz copies the URL before returning.
        let initramfs_url = NSURL::fileURLWithPath(&NSString::from_str(initramfs_str));
        unsafe { bl.setInitialRamdiskURL(Some(&initramfs_url)) };
    }

    Ok(bl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_fake_kernel(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("vmlinux");
        fs::write(&path, b"# placeholder kernel for tests\n").unwrap();
        path
    }

    fn write_fake_initramfs(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("initramfs.img");
        fs::write(&path, b"# placeholder initramfs for tests\n").unwrap();
        path
    }

    #[test]
    fn build_boot_loader_rejects_missing_kernel() {
        let err = build_boot_loader(
            Path::new("/nonexistent/kernel/path/vmlinux"),
            "console=hvc0 init=/init",
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("does not exist"),
            "expected missing-file error, got: {err}"
        );
    }

    #[test]
    fn build_boot_loader_accepts_existing_kernel_with_hvc0() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let bl =
            build_boot_loader(&kernel, "console=hvc0 reboot=k panic=1 init=/init", None).unwrap();
        // The constructor succeeded; Vz will validate kernel
        // contents in `VZVirtualMachineConfiguration::validate()`.
        // Sanity-check the command line round-tripped through the
        // NSString boundary.
        let got = unsafe { bl.commandLine() }.to_string();
        assert!(
            got.contains("console=hvc0"),
            "expected console=hvc0 in stored command line, got: {got}"
        );
        assert!(got.contains("init=/init"));
    }

    #[test]
    fn build_boot_loader_with_none_initramfs_leaves_ramdisk_unset() {
        // Guard against accidentally setting an empty / placeholder
        // initramfs URL — Apple treats nil as "no initramfs" and any
        // non-nil value as a real file path Vz will try to mmap.
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let bl = build_boot_loader(&kernel, "console=hvc0 init=/init", None).unwrap();
        let ramdisk = unsafe { bl.initialRamdiskURL() };
        assert!(
            ramdisk.is_none(),
            "expected nil initialRamdiskURL when None was passed; got {:?}",
            ramdisk.map(|u| u.path().map(|s| s.to_string()))
        );
    }

    #[test]
    fn build_boot_loader_attaches_initramfs_when_present() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let initramfs = write_fake_initramfs(tmp.path());
        let bl = build_boot_loader(&kernel, "console=hvc0 init=/init", Some(&initramfs)).unwrap();
        let ramdisk = unsafe { bl.initialRamdiskURL() }
            .expect("initialRamdiskURL must be set when initramfs path was supplied");
        let stored_path = ramdisk
            .path()
            .expect("file URL must round-trip back to a path")
            .to_string();
        assert!(
            stored_path.ends_with("initramfs.img"),
            "expected stored path to end with initramfs.img, got: {stored_path}"
        );
    }

    #[test]
    fn build_boot_loader_rejects_missing_initramfs_path_with_typed_error() {
        let tmp = TempDir::new().unwrap();
        let kernel = write_fake_kernel(tmp.path());
        let bogus = tmp.path().join("does-not-exist-initramfs.img");
        let err = build_boot_loader(&kernel, "console=hvc0 init=/init", Some(&bogus)).unwrap_err();
        assert!(
            err.contains("initramfs file does not exist"),
            "expected initramfs-not-found error shape, got: {err}"
        );
    }
}
