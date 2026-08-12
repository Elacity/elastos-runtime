//! Secure, symlink-resistant, atomic file persistence for the node's operator-owned state root
//! (DKMS-7). The node's master-seed store — and, from Stage 6, its durable revocation journal — are
//! secrets whose first-write and update must never (a) follow an attacker-planted symlink, (b) leave
//! a world-readable intermediate, or (c) silently overwrite a competing first writer. This module is
//! the single reviewed primitive both callers share.
//!
//! ## Threat model and the guarantees relied on (Linux + macOS)
//!
//! An adversary who can create names in the state directory (but is NOT its owner) must not be able
//! to redirect a write or hand the node a forged identity. We do NOT rely on `openat`/`renameat2`
//! (not portable to macOS); instead:
//!
//!   * **Temp file** — created with a RANDOM same-directory name and `create_new(true)`
//!     (`O_CREAT | O_EXCL`) at mode `0600` AT OPEN TIME. `O_EXCL` refuses to open through a symlink
//!     planted at that exact name, and the name is unpredictable, so a pre-plant cannot win. Mode is
//!     set in the open flags, so the file is never momentarily world-readable.
//!   * **Exclusive install** — [`create_new_durable`] installs via `hard_link(temp, final)`, which
//!     fails with `AlreadyExists` if `final` already exists. On both Linux and macOS this is an
//!     atomic no-overwrite: a race loser (or a pre-planted `final` symlink occupying the name) never
//!     clobbers the winner, and the link never writes THROUGH a symlink.
//!   * **Atomic replace** — [`write_atomic_durable`] installs via `rename(temp, final)`, which
//!     atomically replaces `final`. Renaming over a symlink swaps the link itself; it never writes
//!     through it. This is the update primitive for state that is rewritten over time (revocation).
//!   * **Read** — [`read_no_follow`] refuses a symlink at the final component (`lstat` pre-check) and
//!     re-verifies the opened inode against that `lstat` (open-then-fstat identity check), so a
//!     TOCTOU swap of a regular file for a symlink between the check and the open is detected and
//!     fails closed rather than following the link.
//!   * **Durability** — the temp is `fsync`ed before install and the containing directory is
//!     `fsync`ed after, so a crash cannot leave the identity half-committed.
//!   * **Parent gate** — [`validate_parent_dir`] additionally requires the containing directory to be
//!     a real directory (not a symlink), free of `..` traversal, and NOT group/world-writable, so a
//!     non-owner cannot plant names there at all. Operator-ownership of the directory itself is a
//!     deployment guarantee (systemd `StateDirectory`, mode `0700`); the mode gate enforces the part
//!     that is checkable in-process without a `libc` dependency.

use std::io;
use std::path::Path;

/// Restrictive owner-only mode for a freshly created secret file / its intermediate temp.
const SECRET_MODE: u32 = 0o600;

/// Outcome of an exclusive, no-overwrite [`create_new_durable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    /// This caller created `final_path` and durably installed the supplied bytes.
    Created,
    /// `final_path` already existed (a competing writer won the race, or a prior run created it).
    /// The caller MUST now read the existing file to obtain the durable content — the bytes it
    /// offered were NOT installed.
    AlreadyExisted,
}

/// A fault-injection seam for the "a simulated write/fsync/rename failure never reports success"
/// regression. Test-only; the daemon never arms it, and the injected error takes the same
/// fail-closed `Err` path a real I/O error would.
#[cfg(test)]
pub(crate) mod fault {
    use std::cell::Cell;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum FailPoint {
        /// Fail right after the temp bytes are written, before `fsync`.
        AfterTempWrite,
        /// Fail at the temp `fsync`.
        AtTempFsync,
        /// Fail at the final install (`hard_link` / `rename`).
        AtInstall,
    }

    thread_local! {
        static ARMED: Cell<Option<FailPoint>> = const { Cell::new(None) };
    }

    /// Arm (or with `None`, disarm) the fault point for the CURRENT thread only.
    pub fn arm(fp: Option<FailPoint>) {
        ARMED.with(|c| c.set(fp));
    }

    pub(super) fn tripped(fp: FailPoint) -> bool {
        ARMED.with(|c| c.get()) == Some(fp)
    }
}

#[cfg(test)]
fn injected(fp: fault::FailPoint) -> io::Result<()> {
    if fault::tripped(fp) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "injected persistence fault",
        ));
    }
    Ok(())
}

/// Exclusive, no-overwrite, symlink-resistant durable create. Writes `bytes` to a random
/// same-directory temp (`O_EXCL | O_CREAT`, mode `0600` at open), `fsync`s it, installs it at
/// `final_path` via `hard_link` (no-overwrite), removes the temp, and `fsync`s the directory.
///
/// Returns [`CreateOutcome::Created`] when this call installed the file, or
/// [`CreateOutcome::AlreadyExisted`] when `final_path` already existed (the offered bytes were NOT
/// installed and the caller must read the winner). Any other error is a hard failure — the caller
/// must treat it as "not persisted" and never report success.
pub fn create_new_durable(final_path: &Path, bytes: &[u8]) -> io::Result<CreateOutcome> {
    let dir = parent_dir(final_path)?;
    let (temp_path, file) = create_secret_temp(dir)?;
    // Best-effort temp cleanup on any early return.
    let guard = TempGuard {
        path: Some(temp_path.clone()),
    };

    write_and_sync_temp(file, bytes)?;

    #[cfg(test)]
    injected(fault::FailPoint::AtInstall)?;

    match std::fs::hard_link(&temp_path, final_path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            // A competing writer (or a pre-planted name) already occupies `final_path`. We did NOT
            // install; the temp is removed by the guard. The caller reads the winner.
            return Ok(CreateOutcome::AlreadyExisted);
        }
        Err(e) => return Err(e),
    }

    // Installed: `final_path` and the temp now name the same inode. Drop the guard to unlink the
    // temp (leaving `final_path`), then fsync the directory so the new link is durable across a crash.
    drop(guard);
    fsync_dir(dir)?;
    Ok(CreateOutcome::Created)
}

/// Atomic, symlink-resistant durable REPLACE. Writes `bytes` to a random same-directory temp
/// (`O_EXCL | O_CREAT`, mode `0600`), `fsync`s it, atomically `rename`s it over `final_path`
/// (replacing any existing regular file OR symlink NAME — never writing through a symlink target),
/// and `fsync`s the directory. This is the update primitive for state rewritten over time (the
/// Stage 6 revocation journal); use [`create_new_durable`] for a one-time exclusive first create.
pub fn write_atomic_durable(final_path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = parent_dir(final_path)?;
    let (temp_path, file) = create_secret_temp(dir)?;
    let guard = TempGuard {
        path: Some(temp_path.clone()),
    };

    write_and_sync_temp(file, bytes)?;

    #[cfg(test)]
    injected(fault::FailPoint::AtInstall)?;

    std::fs::rename(&temp_path, final_path)?;
    // Rename consumed the temp name; nothing to clean up.
    guard.disarm();
    fsync_dir(dir)?;
    Ok(())
}

/// Read a file, refusing to follow a symlink at the final component (fail closed). Returns a
/// `NotFound` error when the path does not exist (so callers can distinguish "create it" from a
/// hard failure).
pub fn read_no_follow(path: &Path) -> io::Result<Vec<u8>> {
    let pre = std::fs::symlink_metadata(path)?; // NotFound propagates
    if pre.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state file is a symlink — refusing to follow it (fail closed)",
        ));
    }
    let file = std::fs::File::open(path)?;
    // TOCTOU guard: verify the inode we actually opened is the regular file we `lstat`ed, not a
    // symlink swapped in between the check and the open.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let post = file.metadata()?;
        if post.dev() != pre.dev() || post.ino() != pre.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "state file changed identity during open (possible symlink race) — fail closed",
            ));
        }
    }
    read_all(file)
}

fn read_all(mut file: std::fs::File) -> io::Result<Vec<u8>> {
    use io::Read as _;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Validate the directory that will hold a secret state file: it must be a real directory (not a
/// symlink), the path must be free of `..` traversal, and — on Unix — the directory must NOT be
/// group- or world-writable, so a non-owner cannot plant names (symlinks) in it. Operator-ownership
/// of the directory is a deployment guarantee (systemd `StateDirectory`); this is the in-process,
/// dependency-free half of "constrain the keystore under an operator-owned state root".
pub fn validate_parent_dir(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("state path must not contain `..` (path traversal)".to_string());
    }
    if path.file_name().is_none() {
        return Err("state path has no file name".to_string());
    }
    let parent = parent_dir(path).map_err(|_| "state path has no parent directory".to_string())?;
    let md = std::fs::symlink_metadata(parent)
        .map_err(|e| format!("state directory {}: {e}", parent.display()))?;
    if md.file_type().is_symlink() {
        return Err(format!(
            "state directory {} is a symlink — refusing (fail closed)",
            parent.display()
        ));
    }
    if !md.is_dir() {
        return Err(format!(
            "state directory {} is not a directory",
            parent.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = md.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(format!(
                "state directory {} is group/world-writable (mode {:o}) — a non-owner could plant a \
                 symlink; provision it 0700",
                parent.display(),
                mode & 0o777
            ));
        }
    }
    Ok(())
}

fn parent_dir(path: &Path) -> io::Result<&Path> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => Ok(p),
        // A bare filename resolves against the current directory.
        _ => Ok(Path::new(".")),
    }
}

/// Create a random-named, owner-only (mode 0600 at open), exclusively-created temp in `dir`.
fn create_secret_temp(dir: &Path) -> io::Result<(std::path::PathBuf, std::fs::File)> {
    // A few attempts in the astronomically unlikely event of a name collision.
    let mut last_err: Option<io::Error> = None;
    for _ in 0..8 {
        let name = random_temp_name();
        let temp_path = dir.join(name);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true); // O_CREAT | O_EXCL — refuses a planted symlink at this name
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(SECRET_MODE); // 0600 in the open flags — never momentarily world-readable
        }
        match opts.open(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::Other, "could not create temp file")))
}

fn write_and_sync_temp(mut file: std::fs::File, bytes: &[u8]) -> io::Result<()> {
    use io::Write as _;
    file.write_all(bytes)?;
    #[cfg(test)]
    injected(fault::FailPoint::AfterTempWrite)?;
    file.flush()?;
    #[cfg(test)]
    injected(fault::FailPoint::AtTempFsync)?;
    file.sync_all()?; // fsync the temp before install
    Ok(())
}

/// `fsync` a directory so a freshly created/renamed entry is durable. Best-effort on platforms that
/// refuse to open a directory for this purpose; a failure to open the dir is not fatal to the write
/// (the file itself is already fsynced), but a failed fsync of a successfully opened dir is reported.
fn fsync_dir(dir: &Path) -> io::Result<()> {
    match std::fs::File::open(dir) {
        Ok(f) => match f.sync_all() {
            Ok(()) => Ok(()),
            // Some platforms/filesystems reject fsync on a directory handle; the entry is still
            // durable via the file fsync. Only surface an unexpected error.
            Err(e) if e.raw_os_error() == Some(22) /* EINVAL */ => Ok(()),
            Err(e) => Err(e),
        },
        Err(_) => Ok(()),
    }
}

/// A 32-hex-char random temp basename, unpredictable so a pre-plant cannot guess it.
fn random_temp_name() -> String {
    let r = ddrm_envelope::random_seed(); // 32 bytes of CSPRNG output
    let mut s = String::with_capacity(4 + 32);
    s.push_str(".tmp");
    for b in &r[..16] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// RAII cleanup for the intermediate temp: removes it unless the install consumed it (via rename) or
/// the caller explicitly disarmed after a successful hard-link install.
struct TempGuard {
    path: Option<std::path::PathBuf>,
}

impl TempGuard {
    // Used by `write_atomic_durable` (rename consumes the temp).
    fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}
