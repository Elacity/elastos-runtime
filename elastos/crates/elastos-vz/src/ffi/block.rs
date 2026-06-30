//! `VZVirtioBlockDeviceConfiguration` wrapper.
//!
//! Vz attaches raw disk images via
//! `VZDiskImageStorageDeviceAttachment`, and the
//! `cachingMode` / `synchronizationMode` knobs both matter:
//!
//! - `cachingMode = Cached` — the macOS unified buffer cache
//!   handles writes. Lima uses this in
//!   `pkg/hostagent/vmnet/vmnet_darwin.go` and so does UTM after
//!   they hit #4840. The alternative (`Uncached`) bypasses the
//!   page cache and produces tearing on host crash.
//! - `synchronizationMode = Fsync` — every guest flush translates
//!   to a host `fsync`. Without it, a host crash with un-flushed
//!   pages leaves the rootfs in an ambiguous state.
//!
//! The selected enum values are present in `objc2-virtualization 0.3`.

#![cfg(target_os = "macos")]

use std::path::Path;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::{
    VZDiskImageCachingMode, VZDiskImageStorageDeviceAttachment, VZDiskImageSynchronizationMode,
    VZVirtioBlockDeviceConfiguration,
};

use super::error::ns_error_to_string;

/// Build a virtio-block device from a raw disk image on disk.
///
/// Returns `Err(String)` if the file is missing, has an invalid
/// UTF-8 path, or Vz rejects the attachment (Vz documents that
/// the file length must be a 512-byte multiple; that failure
/// surfaces here as an NSError with `localizedDescription`).
pub(crate) fn build_block_device(
    disk_path: &Path,
    read_only: bool,
) -> Result<Retained<VZVirtioBlockDeviceConfiguration>, String> {
    if !disk_path.exists() {
        return Err(format!(
            "block device: disk image not found at {}",
            disk_path.display()
        ));
    }

    let path_str = disk_path.to_str().ok_or_else(|| {
        format!(
            "block device: disk path is not valid UTF-8 ({})",
            disk_path.display()
        )
    })?;

    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));

    // SAFETY: the URL was just constructed from a valid UTF-8 path
    // that points at an existing regular file. Apple's
    // `init…readOnly:cachingMode:synchronizationMode:error:` runs
    // entirely on the calling thread.
    let attachment = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            read_only,
            VZDiskImageCachingMode::Cached,
            VZDiskImageSynchronizationMode::Fsync,
        )
    }
  .map_err(|err| {
        format!(
            "block device: Vz rejected attachment for {} ({})",
            disk_path.display(),
            ns_error_to_string(&err)
        )
    })?;

    // SAFETY: `initWithAttachment` wraps the attachment in a
    // VZVirtioBlockDeviceConfiguration. Apple retains the
    // attachment internally so dropping our `Retained<>` is fine.
    let block = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        )
    };

    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use tempfile::TempDir;

    /// 1 MiB is the smallest Vz-acceptable raw image (size must be
    /// a multiple of 512; 1 MiB is comfortably above that). Used
    /// only for tests; production capsules ship pre-sized rootfs
    /// images.
    const TEST_IMAGE_SIZE: u64 = 1024 * 1024;

    fn write_fake_image(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("rootfs.img");
        let f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.set_len(TEST_IMAGE_SIZE).unwrap();
        path
    }

    #[test]
    fn rejects_missing_file_with_clear_message() {
        let err = build_block_device(Path::new("/nonexistent/disk.img"), false).unwrap_err();
        assert!(
            err.contains("not found"),
            "expected not-found error, got: {err}"
        );
        assert!(err.contains("/nonexistent/disk.img"));
    }

    #[test]
    fn accepts_512_aligned_file_read_write() {
        let tmp = TempDir::new().unwrap();
        let img = write_fake_image(tmp.path());
        // The Retained pointer being non-null is enough proof of
        // construction; the deeper Vz validation lives in
        // `VZVirtualMachineConfiguration::validateWithError`.
        let _block = build_block_device(&img, false).expect("attachment + block device construct");
    }

    #[test]
    fn accepts_512_aligned_file_read_only() {
        let tmp = TempDir::new().unwrap();
        let img = write_fake_image(tmp.path());
        let _block =
            build_block_device(&img, true).expect("read-only attachment + block device construct");
    }
}
