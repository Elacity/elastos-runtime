//! Host resources shared by participating Runtime installations in one OS account.
use crate::local_llama::LocalLlamaFault;
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt},
    io::AsRawFd,
};
use std::path::{Path, PathBuf};

/// The account lock has no payload. Keep its inode after release: unlinking a
/// held lock would let another installation create a second ownership domain.
pub(crate) fn account_lease_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(format!("/tmp/elastos-local-model-{}.lock", unsafe {
            libc::geteuid()
        }))
    }
    #[cfg(not(unix))]
    {
        PathBuf::new()
    }
}

#[cfg(unix)]
pub(crate) fn acquire_lease(path: &Path) -> Result<File, LocalLlamaFault> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| LocalLlamaFault::Failed)?;
    let metadata = file.metadata().map_err(|_| LocalLlamaFault::Failed)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() != 0
    {
        return Err(LocalLlamaFault::Failed);
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
            LocalLlamaFault::Busy
        } else {
            LocalLlamaFault::Failed
        });
    }
    Ok(file)
}

#[cfg(not(unix))]
pub(crate) fn acquire_lease(_path: &Path) -> Result<File, LocalLlamaFault> {
    Err(LocalLlamaFault::Failed)
}

/// Only the spawned guard and its engine inherit the lease. Parent descriptors
/// retain CLOEXEC, so unrelated provider children cannot prolong ownership.
#[cfg(unix)]
pub(crate) fn inherit_lease(command: &mut std::process::Command, lease: Option<&File>) {
    use std::os::unix::process::CommandExt as _;
    if let Some(lease) = lease {
        let fd = lease.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn inherit_lease(_command: &mut std::process::Command, _lease: Option<&File>) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::{Command, Stdio};

    #[test]
    fn independent_roots_share_one_empty_account_lease() {
        assert_eq!(
            account_lease_path(),
            PathBuf::from(format!("/tmp/elastos-local-model-{}.lock", unsafe {
                libc::geteuid()
            }))
        );
        let root = crate::test_support::temp_root_path("model-provider-resources", "lease");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("account.lock");
        let first = acquire_lease(&path).unwrap();
        assert!(matches!(acquire_lease(&path), Err(LocalLlamaFault::Busy)));
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        drop(first);
        let second = acquire_lease(&path).unwrap();
        drop(second);
        assert!(path.exists());
    }

    #[test]
    fn inherited_engine_lease_survives_parent_descriptor_close() {
        let root = crate::test_support::temp_root_path("model-provider-resources", "inherit");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("account.lock");
        let lease = acquire_lease(&path).unwrap();
        let mut command = Command::new("/bin/cat");
        command.stdin(Stdio::piped()).stdout(Stdio::null());
        inherit_lease(&mut command, Some(&lease));
        let mut child = command.spawn().unwrap();
        drop(lease);
        let blocked = matches!(acquire_lease(&path), Err(LocalLlamaFault::Busy));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(blocked);
        assert!(acquire_lease(&path).is_ok());
    }

    #[test]
    fn lease_rejects_symlinks_hardlinks_and_shared_permissions() {
        let root = crate::test_support::temp_root_path("model-provider-resources", "unsafe");
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        drop(acquire_lease(&target).unwrap());
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(acquire_lease(&link), Err(LocalLlamaFault::Failed)));
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&target, &link).unwrap();
        assert!(matches!(acquire_lease(&link), Err(LocalLlamaFault::Failed)));
        std::fs::remove_file(&link).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            acquire_lease(&target),
            Err(LocalLlamaFault::Failed)
        ));
    }
}
