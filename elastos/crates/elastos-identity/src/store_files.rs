//! Identity files below the operator-selected, trusted Runtime data root.
//! Unix uses descriptor-bound paths. Other hosts retain the existing path/ACL
//! model; that adapter is not sufficient for secure public owner enrollment.
//! The stable identity lock orders key creation and credential mutations. Callers
//! must release it before taking an auth-state lock, or acquire auth before identity.

#[cfg(unix)]
use anyhow::Context;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::{MetadataExt, OpenOptionsExt},
};

#[cfg(unix)]
pub(super) struct IdentityFiles {
    root: File,
    directory: File,
    _lock: File,
}

#[cfg(not(unix))]
pub(super) use portable::IdentityFiles;

#[cfg(unix)]
impl IdentityFiles {
    pub(super) fn open(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let root = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(data_dir)?;
        validate_owner(&root.metadata()?)?;
        if unsafe { libc::mkdirat(root.as_raw_fd(), c"identity".as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        } else {
            root.sync_all()?;
        }
        let directory = open_at(&root, c"identity", libc::O_RDONLY | libc::O_DIRECTORY)?;
        validate_owner(&directory.metadata()?)?;
        let lock = match open_at(
            &directory,
            c"identity.lock",
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                open_at(&directory, c"identity.lock", libc::O_RDWR)
                    .context("open existing identity lock")?
            }
            Err(error) => return Err(error).context("create identity lock"),
        };
        validate_file(&lock)?;
        lock.lock()?;
        let files = Self {
            root,
            directory,
            _lock: lock,
        };
        files.ensure_attached()?;
        // A replaced lock must not create two independent writer groups.
        let current = open_at(&files.directory, c"identity.lock", libc::O_RDWR)?;
        if !same_file(&current, &files._lock)? {
            anyhow::bail!("identity lock changed while being acquired");
        }
        Ok(files)
    }

    fn ensure_attached(&self) -> anyhow::Result<()> {
        let current = open_at(&self.root, c"identity", libc::O_RDONLY | libc::O_DIRECTORY)?;
        if !same_file(&current, &self.directory)? {
            anyhow::bail!("identity directory changed during persistence");
        }
        validate_owner(&current.metadata()?)
    }

    pub(super) fn sync(&self) -> anyhow::Result<()> {
        self.ensure_attached()?;
        self.directory
            .sync_all()
            .map_err(|_| anyhow::anyhow!("identity replacement durability indeterminate"))?;
        Ok(())
    }

    pub(super) fn read(&self, name: &std::ffi::CStr) -> anyhow::Result<Option<Vec<u8>>> {
        self.ensure_attached()?;
        let mut file = match open_at(&self.directory, name, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        validate_file(&file)?;
        if name == c"device.key" && file.metadata()?.mode() & 0o077 != 0 {
            anyhow::bail!("identity device.key must be owner-only");
        }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        if name == c"device.key" {
            file.take(33).read_to_end(&mut bytes)?;
        } else {
            file.read_to_end(&mut bytes)?;
        }
        self.ensure_attached()?;
        Ok(Some(std::mem::take(&mut *bytes)))
    }

    /// Before rename, errors preserve the old file. After rename, errors mean
    /// durability is indeterminate; the caller must reload, never roll back.
    pub(super) fn replace(
        &self,
        name: &std::ffi::CStr,
        bytes: &[u8],
        #[cfg(test)] fault: Option<SaveFault>,
    ) -> anyhow::Result<()> {
        self.ensure_attached()?;
        // Validate the existing destination without following it.
        if let Some(bytes) = self.read(name)? {
            let _bytes = zeroize::Zeroizing::new(bytes);
        }
        let temp = std::ffi::CString::new(format!(".identity-{}.tmp", uuid::Uuid::new_v4()))?;
        let mut file = open_at(
            &self.directory,
            &temp,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(test)]
            if fault == Some(SaveFault::BeforeReplace) {
                anyhow::bail!("injected pre-replacement failure");
            }
            self.ensure_attached()?;
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temp.as_ptr(),
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            #[cfg(test)]
            if fault == Some(SaveFault::AfterReplace) {
                anyhow::bail!("identity replacement durability indeterminate (injected)");
            }
            self.directory
                .sync_all()
                .map_err(|_| anyhow::anyhow!("identity replacement durability indeterminate"))?;
            self.ensure_attached().map_err(|_| {
                anyhow::anyhow!("identity replacement durability indeterminate: directory changed")
            })?;
            Ok(())
        })();
        // Only this operation's exclusively created temporary name is removed.
        if result.is_err() {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temp.as_ptr(), 0);
            }
        }
        result
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveFault {
    BeforeReplace,
    AfterReplace,
}

// Compile-time adapter for existing non-Unix identity use. This intentionally
// makes no descriptor-relative path or parent-directory durability guarantee.
#[cfg(any(not(unix), test))]
mod portable {
    #[cfg(test)]
    use super::SaveFault;
    use std::{
        ffi::CStr,
        fs::File,
        io::{Read, Write},
        path::{Path, PathBuf},
    };

    pub(crate) struct IdentityFiles {
        directory: PathBuf,
        _lock: File,
    }

    impl IdentityFiles {
        pub(crate) fn open(data_dir: &Path) -> anyhow::Result<Self> {
            let directory = data_dir.join("identity");
            std::fs::create_dir_all(&directory)?;
            let path = directory.join("identity.lock");
            let lock = match File::options()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    File::options().read(true).write(true).open(&path)?
                }
                Err(error) => return Err(error.into()),
            };
            lock.lock()?;
            Ok(Self {
                directory,
                _lock: lock,
            })
        }

        pub(crate) fn read(&self, name: &CStr) -> anyhow::Result<Option<Vec<u8>>> {
            let mut file = match File::open(self.directory.join(name.to_str()?)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            if name == c"device.key" {
                file.take(33).read_to_end(&mut bytes)?;
            } else {
                file.read_to_end(&mut bytes)?;
            }
            Ok(Some(std::mem::take(&mut *bytes)))
        }

        pub(crate) fn sync(&self) -> anyhow::Result<()> {
            // std does not expose a portable parent-directory sync operation.
            std::fs::metadata(&self.directory)?;
            Ok(())
        }

        pub(crate) fn replace(
            &self,
            name: &CStr,
            bytes: &[u8],
            #[cfg(test)] fault: Option<SaveFault>,
        ) -> anyhow::Result<()> {
            let temp = self
                .directory
                .join(format!(".identity-{}.tmp", uuid::Uuid::new_v4()));
            let mut file = File::options().create_new(true).write(true).open(&temp)?;
            let result = (|| {
                file.write_all(bytes)?;
                file.sync_all()?;
                drop(file);
                #[cfg(test)]
                if fault == Some(SaveFault::BeforeReplace) {
                    anyhow::bail!("injected pre-replacement failure");
                }
                std::fs::rename(&temp, self.directory.join(name.to_str()?))?;
                #[cfg(test)]
                if fault == Some(SaveFault::AfterReplace) {
                    anyhow::bail!("identity replacement durability indeterminate (injected)");
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result
        }
    }

    #[test]
    fn portable_adapter_locks_and_replaces_without_changing_format() {
        let root = tempfile::tempdir().unwrap();
        let files = IdentityFiles::open(root.path()).unwrap();
        let other = File::options()
            .read(true)
            .write(true)
            .open(root.path().join("identity/identity.lock"))
            .unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        files.replace(c"credentials.json", b"old", None).unwrap();
        assert!(files
            .replace(c"credentials.json", b"new", Some(SaveFault::BeforeReplace))
            .is_err());
        assert_eq!(files.read(c"credentials.json").unwrap().unwrap(), b"old");
        assert!(files
            .replace(c"credentials.json", b"new", Some(SaveFault::AfterReplace))
            .is_err());
        assert_eq!(files.read(c"credentials.json").unwrap().unwrap(), b"new");
        files.replace(c"device.key", &[7; 64], None).unwrap();
        assert_eq!(files.read(c"device.key").unwrap().unwrap().len(), 33);
        files.sync().unwrap();
        assert!(std::fs::read_dir(root.path().join("identity"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        drop(files);
        other.try_lock().unwrap();
    }
}

#[cfg(unix)]
fn open_at(directory: &File, name: &std::ffi::CStr, flags: libc::c_int) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn validate_owner(metadata: &std::fs::Metadata) -> anyhow::Result<()> {
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        anyhow::bail!("identity path must be owned by the Runtime user and not writable by others");
    }
    Ok(())
}

#[cfg(unix)]
fn validate_file(file: &File) -> anyhow::Result<()> {
    let metadata = file.metadata()?;
    validate_owner(&metadata)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        anyhow::bail!("identity file must be regular with one link");
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn fresh_identity_lock_creation_is_safe_with_concurrent_openers() {
        let root = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    let files = IdentityFiles::open(root.path()).unwrap();
                    assert!(files.read(c"device.key").unwrap().is_none());
                });
            }
        });
        assert_eq!(
            std::fs::read_dir(root.path().join("identity"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn identity_files_are_private_and_parent_or_file_redirection_is_rejected() {
        for name in [
            "identity",
            "device.key",
            "credentials.json",
            "identity.lock",
        ] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let sentinel = outside.path().join("sentinel");
            std::fs::write(&sentinel, b"preserved").unwrap();
            if name == "identity" {
                symlink(outside.path(), root.path().join(name)).unwrap();
                assert!(IdentityFiles::open(root.path()).is_err());
            } else {
                drop(IdentityFiles::open(root.path()).unwrap());
                let target = root.path().join("identity").join(name);
                if target.exists() {
                    std::fs::remove_file(&target).unwrap();
                }
                symlink(&sentinel, &target).unwrap();
                if name == "identity.lock" {
                    assert!(IdentityFiles::open(root.path()).is_err());
                } else {
                    let files = IdentityFiles::open(root.path()).unwrap();
                    let name = std::ffi::CString::new(name).unwrap();
                    assert!(files.read(&name).is_err());
                    assert!(files.replace(&name, b"replacement", None).is_err());
                }
            }
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserved");
        }
        let root = tempfile::tempdir().unwrap();
        let files = IdentityFiles::open(root.path()).unwrap();
        files.replace(c"device.key", &[7; 32], None).unwrap();
        files
            .replace(c"credentials.json", b"encrypted fixture", None)
            .unwrap();
        for name in ["device.key", "credentials.json", "identity.lock"] {
            let mode = std::fs::metadata(root.path().join("identity").join(name))
                .unwrap()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(files.directory.metadata().unwrap().mode() & 0o777, 0o700);
        files.replace(c"device.key", &[7; 64], None).unwrap();
        assert_eq!(files.read(c"device.key").unwrap().unwrap().len(), 33);
    }

    #[test]
    fn identity_descriptor_refuses_replaced_parent_without_touching_redirect() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let files = IdentityFiles::open(root.path()).unwrap();
        files.replace(c"credentials.json", b"old", None).unwrap();
        std::fs::rename(root.path().join("identity"), root.path().join("retained")).unwrap();
        symlink(outside.path(), root.path().join("identity")).unwrap();
        assert!(files.replace(c"credentials.json", b"new", None).is_err());
        assert!(files.read(c"credentials.json").is_err());
        assert_eq!(
            std::fs::read(root.path().join("retained/credentials.json")).unwrap(),
            b"old"
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn identity_files_reject_hard_links_non_files_and_unsafe_permissions() {
        let root = tempfile::tempdir().unwrap();
        let files = IdentityFiles::open(root.path()).unwrap();
        let target = root.path().join("identity/credentials.json");
        files.replace(c"credentials.json", b"old", None).unwrap();
        let link = root.path().join("alias");
        std::fs::hard_link(&target, &link).unwrap();
        assert!(files.replace(c"credentials.json", b"new", None).is_err());
        assert_eq!(std::fs::read(&link).unwrap(), b"old");
        std::fs::remove_file(&link).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(files.read(c"credentials.json").is_err());
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        assert!(files.read(c"credentials.json").is_err());
        std::fs::set_permissions(
            root.path().join("identity"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(files.read(c"device.key").is_err());
        assert!(IdentityFiles::open(root.path()).is_err());
    }

    #[test]
    fn identity_key_replacement_faults_leave_complete_old_or_new_bytes() {
        for fault in [SaveFault::BeforeReplace, SaveFault::AfterReplace] {
            let root = tempfile::tempdir().unwrap();
            let files = IdentityFiles::open(root.path()).unwrap();
            assert!(files.replace(c"device.key", &[7; 32], Some(fault)).is_err());
            let key = files.read(c"device.key").unwrap();
            assert_eq!(
                key,
                if fault == SaveFault::BeforeReplace {
                    None
                } else {
                    Some(vec![7; 32])
                }
            );
            assert!(std::fs::read_dir(root.path().join("identity"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")));
        }
    }
}
