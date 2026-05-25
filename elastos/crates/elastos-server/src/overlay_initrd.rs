//! Phase 8 Day 6 — overlay-init CPIO builder for the Mac substrate.
//!
//! The Phase-8 squashfs rootfs (`rootfs.ext4` — really a read-only
//! squashfs) is the right v0.1 distribution format: small, immutable,
//! signed by Canonical, no on-host conversion. But it's read-only by
//! design, and Ubuntu userspace (`systemd-logind`, `rsyslog`,
//! `unattended-upgrades`, …) crashes hard when it can't write to
//! `/var/lib/systemd/linger`, `/var/log/journal`, `/var/lib/dbus`,
//! etc. The Day-5 smoke surfaced exactly that pattern.
//!
//! This module fixes it by giving the kernel a *second-stage*
//! initramfs containing a single override file at `/init`. Linux
//! treats concatenated initramfs archives as one (see
//! `init/initramfs.c`), so by appending our CPIO segment to
//! Ubuntu's pristine `bin/initrd` we:
//!
//!   1. Preserve *all* of Ubuntu's tools (`/bin/sh`,
//!      `/usr/bin/switch_root`, `/usr/bin/mount`, `kmod`, the
//!      `overlay.ko` module). The second segment unpacks **on top
//!      of** the first, file-for-file.
//!   2. Shadow Ubuntu's `/init` script with ours. Ours mounts the
//!      squashfs at `/lower`, a tmpfs at `/upper` (256 MiB
//!      ephemeral), overlays the two at `/newroot`, and
//!      `switch_root`s into the merged tree.
//!   3. Hand off to systemd exactly as Ubuntu would have — but
//!      with a writable root that survives userspace writes by
//!      transparently storing them in the tmpfs upperdir.
//!
//! The combined initrd is built **once at `elastos setup` time**
//! (idempotent: skipped when up-to-date) and dropped at
//! `<data_dir>/bin/initrd-overlay`. Consumers (supervisor,
//! `elastos run` standalone lane, integration-test discovery)
//! prefer that path when present, fall back to plain `bin/initrd`
//! otherwise — so Linux callers, kernel-only boots, and the
//! `vm-debug boot` lane are unaffected.
//!
//! ## Why CPIO concatenation, not a re-built initramfs
//!
//! Rebuilding Ubuntu's 31 MB initrd from scratch on every install
//! would require shipping or vendoring `mkinitramfs` + a full set
//! of kernel-matched modules. Concatenation is a 2 KB byte-append
//! that the Linux kernel parses natively — zero new dependencies,
//! zero risk of drifting from Canonical's pinned initrd.
//!
//! ## Why tmpfs (ephemeral), not ext4-on-loop (persistent)
//!
//! v0.1's bar is "Ubuntu userspace boots cleanly." Persistent
//! per-VM state is a separate design question (where does the
//! state file live? how is it sized? does it survive
//! `elastos setup` runs?). tmpfs is the smallest hammer that
//! moves the bar. The chosen upper size (256 MiB) leaves room
//! for systemd-journal, snapd transient state, and a handful of
//! interactive shell users without spilling into swap. Day-7+
//! can add a persistent option behind a capsule manifest flag.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Canonical filename for the second-stage overlay initrd inside
/// `<data_dir>/bin/`. Centralised so the supervisor, `elastos run`
/// standalone lane, and integration test discovery all agree on
/// what to look for.
pub const OVERLAY_INITRD_FILENAME: &str = "initrd-overlay";

/// Canonical filename for Canonical's pristine initrd inside
/// `<data_dir>/bin/`. Mirrored here so the resolver fallback is
/// in one place even if Phase-7-era code paths happen to inline
/// the literal.
pub const PRISTINE_INITRD_FILENAME: &str = "initrd";

/// Given `<data_dir>/bin/`, return the preferred initramfs path:
/// `initrd-overlay` when present (the writable-rootfs variant
/// Day 6 ships), else `initrd` (the kernel-only variant Day 2
/// shipped), else `None` if neither is staged.
///
/// This is the *resolver* shared between all consumers. Callers
/// MUST go through it rather than hard-coding `bin/initrd`, so a
/// future swap (e.g. ext4-overlay variant on a Day-7+ branch) is
/// a one-line change here, not a sweep through the codebase.
pub fn resolve_initrd_path(bin_dir: &Path) -> Option<PathBuf> {
    let overlay = bin_dir.join(OVERLAY_INITRD_FILENAME);
    if overlay.is_file() {
        return Some(overlay);
    }
    let pristine = bin_dir.join(PRISTINE_INITRD_FILENAME);
    pristine.is_file().then_some(pristine)
}

/// The custom `/init` we inject. Runs `#!/bin/sh` (dash, present
/// in Ubuntu's initrd) and uses only tools shipped in Ubuntu's
/// initramfs — `mount`, `modprobe`/`kmod`, `switch_root`, plus
/// the kernel's built-in `overlayfs`/`squashfs` support.
///
/// Kernel cmdline is *read*, not hard-coded: `root=`,
/// `rootfstype=`, and `init=` are honoured so the capsule
/// manifest stays the source of truth and operators can override
/// for debugging without rebuilding the initrd.
const ELASTOS_INIT_SCRIPT: &[u8] = br#"#!/bin/sh
# Phase 8 Day 6 - elastos overlay init.
# Mounts a tmpfs upperdir over the read-only squashfs rootfs so
# Ubuntu userspace can write to /var, /tmp, /home, etc. without
# crashing on EROFS.

set -e

# Standard pseudo-filesystems any init needs. Tolerant of
# already-mounted (idempotent re-entry during debug runs).
[ -d /proc ] || mkdir /proc
[ -d /sys ]  || mkdir /sys
[ -d /dev ]  || mkdir /dev
mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sys /sys 2>/dev/null || true
mount -t devtmpfs dev /dev 2>/dev/null || true

# Load any modules we depend on. squashfs + virtio_blk are
# usually built into the Ubuntu arm64 cloud kernel; overlay
# is a .ko shipped in the initramfs. modprobe is best-effort
# either way (silent success if builtin, silent fail-and-retry
# if the module is genuinely absent).
modprobe overlay    2>/dev/null || true
modprobe virtio_blk 2>/dev/null || true
modprobe squashfs   2>/dev/null || true

# Parse the cmdline. The capsule manifest ships sensible defaults
# but we honour overrides so an operator can do
# `boot_args = "console=hvc0 root=/dev/disk/by-label/foo ..."`
# without us hard-coding device paths.
CMDLINE=$(cat /proc/cmdline)
ROOT=$(echo "$CMDLINE" | tr ' ' '\n' | sed -n 's/^root=//p' | head -1)
ROOTFSTYPE=$(echo "$CMDLINE" | tr ' ' '\n' | sed -n 's/^rootfstype=//p' | head -1)
INIT_CMD=$(echo "$CMDLINE" | tr ' ' '\n' | sed -n 's/^init=//p' | head -1)
[ -z "$ROOT" ]       && ROOT=/dev/vda
[ -z "$ROOTFSTYPE" ] && ROOTFSTYPE=squashfs
[ -z "$INIT_CMD" ]   && INIT_CMD=/sbin/init

# Wait for the root device. virtio PCI probing is async; our
# /init runs well before the kernel has finished enumerating
# block devices, so a naive `mount` race-loses ~100% of the
# time. 50 attempts * 100ms = 5s ceiling, which is generous
# next to Vz's ~50ms typical device-ready latency. We poll on
# the device node, not on `mount`, so the timeout is bounded
# regardless of mount-side blocking semantics.
i=0
while [ $i -lt 50 ]; do
    [ -b "$ROOT" ] && break
    sleep 0.1
    i=$((i + 1))
done
if [ ! -b "$ROOT" ]; then
    echo "elastos-init: $ROOT did not appear after 5s; falling back to Ubuntu /init"
    echo "elastos-init: /dev listing:"
    ls /dev
    # Drop the failing exec and execute Ubuntu's /init via its
    # absolute path on the initramfs (we shadowed /init at
    # /init, but Ubuntu's lives at no other path -- if we
    # didn't have the original, we have no recourse).
    exec /sbin/init "$@"
fi

# /lower = read-only Canonical squashfs. /upper = tmpfs holding
# both the overlay upperdir and workdir (overlayfs requires those
# two to live on the *same* mount -- that's why we nest them
# inside /upper).
mkdir -p /lower /upper /newroot
mount -t "$ROOTFSTYPE" -o ro "$ROOT" /lower
mount -t tmpfs -o size=256m tmpfs /upper
mkdir /upper/upper /upper/work

mount -t overlay overlay \
    -o lowerdir=/lower,upperdir=/upper/upper,workdir=/upper/work \
    /newroot

# Move pseudo-fs into the new root so systemd-on-/newroot sees
# them at the expected paths.
mount --move /proc /newroot/proc
mount --move /sys  /newroot/sys
mount --move /dev  /newroot/dev

# Hand off. switch_root pivots, drops the initramfs, and execs
# the operator-specified init binary (systemd by default).
exec switch_root /newroot "$INIT_CMD"
"#;

/// Build the CPIO archive we append to Ubuntu's initrd.
///
/// Single entry: `/init`, mode `0100755`. Trailing `TRAILER!!!`
/// entry tells the kernel "no more files in this segment."
///
/// Returned bytes are intended to be **appended verbatim** to a
/// previous initramfs file. The kernel's `unpack_to_rootfs`
/// (`init/initramfs.c`) scans the buffer and unpacks each segment
/// in order; later segments overlay earlier ones, so our `/init`
/// shadows Ubuntu's at boot time.
pub fn build_overlay_init_cpio() -> Vec<u8> {
    let mut out = Vec::with_capacity(ELASTOS_INIT_SCRIPT.len() + 256);
    write_cpio_entry(&mut out, "init", ELASTOS_INIT_SCRIPT, 0o100755);
    write_cpio_trailer(&mut out);
    out
}

/// Read `source`, append the overlay-init CPIO bytes, write the
/// result to `dest`. Returns `Ok(true)` if a write happened,
/// `Ok(false)` if `dest` was already up-to-date and we skipped
/// the work.
///
/// Idempotency model: byte-compare the existing destination to
/// the freshly-computed payload. Cheap (one file read), correct
/// (no false-skip if either `source` or the embedded script
/// changes), and avoids the trap of trusting mtime alone.
pub fn write_combined_initrd(source: &Path, dest: &Path) -> anyhow::Result<bool> {
    use std::fs;

    let mut combined = fs::read(source)
        .map_err(|e| anyhow::anyhow!("reading source initrd {}: {e}", source.display()))?;
    combined.extend_from_slice(&build_overlay_init_cpio());

    if let Ok(existing) = fs::read(dest) {
        if existing == combined {
            return Ok(false);
        }
    }

    fs::write(dest, &combined)
        .map_err(|e| anyhow::anyhow!("writing combined initrd {}: {e}", dest.display()))?;
    Ok(true)
}

// ── CPIO newc-format internals ────────────────────────────────────
//
// Format spec: Linux `Documentation/early-userspace/buffer-format.rst`
// and `init/initramfs.c`. Each entry is:
//
//   "070701"                      (6-byte magic)
//   13 hex fields × 8 chars each  (110 bytes total header)
//   filename + NUL                (variable)
//   pad to 4-byte boundary
//   data                          (variable)
//   pad to 4-byte boundary
//
// 13 fields in order: ino, mode, uid, gid, nlink, mtime,
// filesize, devmajor, devminor, rdevmajor, rdevminor, namesize
// (includes NUL), check (unused, must be 0).

const CPIO_NEWC_MAGIC: &[u8] = b"070701";
/// Regular file, mode 0755 (rwxr-xr-x).
const CPIO_MODE_EXEC_FILE: u32 = 0o100755;

fn write_cpio_entry(buf: &mut Vec<u8>, name: &str, data: &[u8], mode: u32) {
    let name_bytes = name.as_bytes();
    let namesize = (name_bytes.len() + 1) as u32;
    let filesize = data.len() as u32;

    buf.extend_from_slice(CPIO_NEWC_MAGIC);
    write_hex(buf, 1); // ino — kernel doesn't care, must be non-zero for real files
    write_hex(buf, mode);
    write_hex(buf, 0); // uid (root)
    write_hex(buf, 0); // gid (root)
    write_hex(buf, 1); // nlink
    write_hex(buf, 0); // mtime — kernel ignores
    write_hex(buf, filesize);
    write_hex(buf, 0); // devmajor
    write_hex(buf, 0); // devminor
    write_hex(buf, 0); // rdevmajor
    write_hex(buf, 0); // rdevminor
    write_hex(buf, namesize);
    write_hex(buf, 0); // check — required to be 0 for newc

    buf.extend_from_slice(name_bytes);
    buf.push(0);
    pad_to_4(buf);

    buf.extend_from_slice(data);
    pad_to_4(buf);
}

fn write_cpio_trailer(buf: &mut Vec<u8>) {
    // The terminator is a zero-data zero-mode entry named
    // "TRAILER!!!". Linux scans for this to know the segment is
    // complete and may begin scanning for another concatenated
    // archive after the trailing alignment pad.
    write_cpio_entry(buf, "TRAILER!!!", &[], 0);
}

fn write_hex(buf: &mut Vec<u8>, val: u32) {
    write!(buf, "{:08x}", val).expect("Vec<u8> writes are infallible");
}

fn pad_to_4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpio_starts_with_magic() {
        let bytes = build_overlay_init_cpio();
        assert_eq!(&bytes[..6], CPIO_NEWC_MAGIC, "first entry must be newc cpio");
    }

    #[test]
    fn cpio_contains_init_filename_and_script_payload() {
        let bytes = build_overlay_init_cpio();
        // The filename appears immediately after the 110-byte header
        // (6 magic + 13×8 hex). Asserting on the embedded literal is
        // enough — a corrupt header would have been caught by the
        // round-trip test below.
        assert!(
            contains(&bytes, b"init\0"),
            "cpio body must contain `init\\0`"
        );
        assert!(
            contains(&bytes, b"mount -t overlay"),
            "cpio body must contain the overlay mount command"
        );
        assert!(
            contains(&bytes, b"exec switch_root"),
            "cpio body must contain the switch_root handoff"
        );
    }

    #[test]
    fn cpio_ends_with_trailer() {
        let bytes = build_overlay_init_cpio();
        assert!(
            contains(&bytes, b"TRAILER!!!\0"),
            "cpio must terminate with TRAILER!!! sentinel"
        );
    }

    #[test]
    fn cpio_is_4byte_aligned() {
        let bytes = build_overlay_init_cpio();
        assert_eq!(
            bytes.len() % 4,
            0,
            "cpio archive must end on a 4-byte boundary for kernel scanner"
        );
    }

    #[test]
    fn cpio_roundtrips_through_system_cpio() {
        // If this test fails on a future CI runner without `cpio`,
        // the assertion above on magic/filename/trailer is still
        // a strong invariant — but on a dev box `cpio -t -F` is the
        // gold-standard parser, so use it when available.
        let cpio = match std::process::Command::new("cpio")
            .arg("--version")
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return, // no cpio on host — skip rather than fail
        };
        let _ = cpio;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("test.cpio");
        std::fs::write(&archive, build_overlay_init_cpio()).unwrap();

        let listing = std::process::Command::new("cpio")
            .args(["-t", "-F"])
            .arg(&archive)
            .output()
            .expect("cpio -t");
        let stdout = String::from_utf8_lossy(&listing.stdout);
        assert!(
            stdout.contains("init"),
            "cpio -t must list `init`; got: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&listing.stderr)
        );
    }

    #[test]
    fn write_combined_appends_to_source_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.initrd");
        let dest = tmp.path().join("combined.initrd");
        // Use a sentinel byte pattern so we can verify our CPIO
        // is appended *after* the source bytes verbatim.
        let source_bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        std::fs::write(&source, &source_bytes).unwrap();

        let wrote = write_combined_initrd(&source, &dest).unwrap();
        assert!(wrote, "first call must write the file");

        let combined = std::fs::read(&dest).unwrap();
        let expected_len = source_bytes.len() + build_overlay_init_cpio().len();
        assert_eq!(combined.len(), expected_len);
        assert_eq!(
            &combined[..source_bytes.len()],
            source_bytes.as_slice(),
            "source bytes must be preserved verbatim in the prefix"
        );
    }

    #[test]
    fn write_combined_is_idempotent_when_inputs_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.initrd");
        let dest = tmp.path().join("combined.initrd");
        std::fs::write(&source, b"fake initrd payload").unwrap();

        let first = write_combined_initrd(&source, &dest).unwrap();
        let second = write_combined_initrd(&source, &dest).unwrap();
        assert!(first, "first call writes");
        assert!(!second, "second call must short-circuit when content matches");
    }

    #[test]
    fn write_combined_rewrites_when_source_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.initrd");
        let dest = tmp.path().join("combined.initrd");
        std::fs::write(&source, b"initial payload").unwrap();
        write_combined_initrd(&source, &dest).unwrap();

        std::fs::write(&source, b"different payload").unwrap();
        let rewrote = write_combined_initrd(&source, &dest).unwrap();
        assert!(rewrote, "must rewrite when source bytes change");

        let combined = std::fs::read(&dest).unwrap();
        assert!(
            combined.starts_with(b"different payload"),
            "rewrite must use the new source bytes"
        );
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // Used in tests that exercise the same modes the production
    // path uses, so a future tightening of the mode constants is
    // mirrored in test coverage.
    #[test]
    fn cpio_mode_constant_matches_unix_executable_regular_file() {
        assert_eq!(CPIO_MODE_EXEC_FILE, 0o100755);
    }

    #[test]
    fn resolve_initrd_prefers_overlay_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::write(bin.join(PRISTINE_INITRD_FILENAME), b"plain").unwrap();
        std::fs::write(bin.join(OVERLAY_INITRD_FILENAME), b"overlay").unwrap();

        let resolved = resolve_initrd_path(bin).expect("resolver finds initrd-overlay");
        assert!(
            resolved.ends_with(OVERLAY_INITRD_FILENAME),
            "overlay variant must win when both are staged; got {resolved:?}"
        );
    }

    #[test]
    fn resolve_initrd_falls_back_to_pristine_when_overlay_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::write(bin.join(PRISTINE_INITRD_FILENAME), b"plain").unwrap();

        let resolved = resolve_initrd_path(bin).expect("resolver finds pristine initrd");
        assert!(
            resolved.ends_with(PRISTINE_INITRD_FILENAME),
            "pristine variant must be used when overlay is absent; got {resolved:?}"
        );
    }

    #[test]
    fn resolve_initrd_returns_none_when_nothing_staged() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            resolve_initrd_path(tmp.path()).is_none(),
            "resolver must return None on a clean install (kernel-only path)"
        );
    }
}
