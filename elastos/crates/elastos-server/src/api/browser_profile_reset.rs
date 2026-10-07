//! Local profile Reset shares the Mac VZ writer's stable disk-sidecar lease.
//! Runtime supplies the active-principal disk path and checks pending ownership
//! while this guard is held. The removal worker owns the guard through unlink.

use std::fmt;
use std::path::PathBuf;

#[cfg(unix)]
use std::ffi::{CStr, CString, OsString};
#[cfg(unix)]
use std::fs::{self, File, Metadata, OpenOptions};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

#[cfg(unix)]
const DISK_LEAF: &CStr = c"profile.ext4";
#[cfg(unix)]
const LOCK_LEAF: &CStr = c"profile.ext4.lifetime.lock";

#[derive(Debug)]
pub(super) enum ProfileResetError {
    Busy,
    Unsafe(&'static str),
    UnsupportedHost,
    Io(std::io::Error),
    WorkerFailed,
}

impl fmt::Display for ProfileResetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter
                .write_str("Browser profile is in use; close its Browser pages before Reset"),
            Self::Unsafe(reason) => write!(formatter, "Browser profile reset refused: {reason}"),
            Self::UnsupportedHost => {
                formatter.write_str("Browser profile reset requires a supported local disk lease")
            }
            Self::Io(error) => write!(formatter, "Browser profile reset failed: {error}"),
            Self::WorkerFailed => formatter.write_str("Browser profile reset worker failed"),
        }
    }
}

impl std::error::Error for ProfileResetError {}

impl From<std::io::Error> for ProfileResetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
pub(super) struct ProfileResetGuard {
    #[cfg(unix)]
    data_dir: PathBuf,
    #[cfg(unix)]
    root_directory: File,
    #[cfg(unix)]
    directory_chain: Vec<OsString>,
    #[cfg(unix)]
    directory_identities: Vec<FileIdentity>,
    #[cfg(unix)]
    profile_directory: File,
    #[cfg(unix)]
    lock_file: crate::host_lock::FileLock,
    #[cfg(unix)]
    disk_identity: Option<FileIdentity>,
}

pub(super) async fn acquire_profile_reset(
    data_dir: PathBuf,
    disk_path: PathBuf,
) -> Result<ProfileResetGuard, ProfileResetError> {
    #[cfg(unix)]
    {
        tokio::task::spawn_blocking(move || ProfileResetGuard::acquire(data_dir, disk_path))
            .await
            .map_err(|_| ProfileResetError::WorkerFailed)?
    }
    #[cfg(not(unix))]
    {
        let _ = (data_dir, disk_path);
        Err(ProfileResetError::UnsupportedHost)
    }
}

impl ProfileResetGuard {
    #[cfg(unix)]
    fn acquire(data_dir: PathBuf, disk_path: PathBuf) -> Result<Self, ProfileResetError> {
        if !data_dir.is_absolute()
            || !disk_path.is_absolute()
            || disk_path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            || disk_path.to_string_lossy().contains(['\0', '\r', '\n'])
            || !disk_path.ends_with("BrowserProfiles/default/profile.ext4")
        {
            return Err(ProfileResetError::Unsafe(
                "invalid principal profile disk binding",
            ));
        }
        let relative = disk_path.strip_prefix(&data_dir).map_err(|_| {
            ProfileResetError::Unsafe("profile disk is outside the Runtime data root")
        })?;
        let mut directory_chain = Vec::new();
        for component in relative
            .parent()
            .ok_or(ProfileResetError::Unsafe("missing profile disk root"))?
            .components()
        {
            let std::path::Component::Normal(name) = component else {
                return Err(ProfileResetError::Unsafe(
                    "invalid rooted profile directory",
                ));
            };
            directory_chain.push(name.to_os_string());
        }
        // The configured data root is the trusted boundary. OS aliases before
        // that boundary may resolve normally; every component inside it uses
        // descriptor-relative nofollow operations.
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&data_dir)?;
        directory_identity(&root_directory.metadata()?)?;
        let (profile_directory, directory_identities) =
            open_directory_chain(&root_directory, &directory_chain, true)?;
        let lock_file = open_at(
            &profile_directory,
            LOCK_LEAF,
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )?;
        validate_opened_identity(&lock_file, &profile_directory, LOCK_LEAF)?;
        let lock_file = crate::host_lock::FileLock::try_exclusive(lock_file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                ProfileResetError::Busy
            } else {
                ProfileResetError::Io(error)
            }
        })?;
        let mut guard = Self {
            data_dir,
            root_directory,
            directory_chain,
            directory_identities,
            profile_directory,
            lock_file,
            disk_identity: None,
        };
        guard.validate_directory_binding()?;
        validate_opened_identity(&guard.lock_file, &guard.profile_directory, LOCK_LEAF)?;
        guard
            .lock_file
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        guard.disk_identity = inspect_disk(&guard.profile_directory)?;
        Ok(guard)
    }

    pub(super) async fn remove(self) -> Result<bool, ProfileResetError> {
        #[cfg(unix)]
        {
            self.remove_with_hook(|| {}).await
        }
        #[cfg(not(unix))]
        {
            Err(ProfileResetError::UnsupportedHost)
        }
    }

    #[cfg(unix)]
    async fn remove_with_hook(
        self,
        before_remove: impl FnOnce() + Send + 'static,
    ) -> Result<bool, ProfileResetError> {
        // Dropping the HTTP future cannot release this lease while the worker
        // still has an unlink to perform. The worker owns self until it ends.
        tokio::task::spawn_blocking(move || {
            before_remove();
            self.validate_directory_binding()?;
            validate_opened_identity(&self.lock_file, &self.profile_directory, LOCK_LEAF)?;
            let current = inspect_disk(&self.profile_directory)?;
            if current.is_none() {
                return Ok(false);
            }
            if current != self.disk_identity {
                return Err(ProfileResetError::Unsafe(
                    "profile disk identity changed during Reset",
                ));
            }
            if unsafe { libc::unlinkat(self.profile_directory.as_raw_fd(), DISK_LEAF.as_ptr(), 0) }
                == 0
            {
                Ok(true)
            } else {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(ProfileResetError::Io(error))
                }
            }
        })
        .await
        .map_err(|_| ProfileResetError::WorkerFailed)?
    }

    #[cfg(unix)]
    fn validate_directory_binding(&self) -> Result<(), ProfileResetError> {
        if directory_identity(&fs::metadata(&self.data_dir)?)?
            != directory_identity(&self.root_directory.metadata()?)?
        {
            return Err(ProfileResetError::Unsafe(
                "Runtime data root identity changed during Reset",
            ));
        }
        let (current, identities) =
            open_directory_chain(&self.root_directory, &self.directory_chain, false)?;
        if identities != self.directory_identities
            || directory_identity(&current.metadata()?)?
                != directory_identity(&self.profile_directory.metadata()?)?
        {
            return Err(ProfileResetError::Unsafe(
                "profile directory identity changed during Reset",
            ));
        }
        Ok(())
    }

    #[cfg(all(unix, test))]
    pub(super) async fn remove_after_barrier(
        self,
        entered: std::sync::mpsc::Sender<()>,
        proceed: std::sync::mpsc::Receiver<()>,
    ) -> Result<bool, ProfileResetError> {
        self.remove_with_hook(move || {
            entered.send(()).expect("removal barrier observer");
            proceed.recv().expect("removal barrier release");
        })
        .await
    }
}

#[cfg(unix)]
fn regular_identity(metadata: &Metadata) -> Result<FileIdentity, ProfileResetError> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(ProfileResetError::Unsafe(
            "profile disk and lease require regular files with one link",
        ));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(unix)]
fn validate_opened_identity(
    file: &File,
    directory: &File,
    name: &CStr,
) -> Result<FileIdentity, ProfileResetError> {
    let opened = regular_identity(&file.metadata()?)?;
    let stat = stat_at(directory, name)?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_nlink != 1 {
        return Err(ProfileResetError::Unsafe(
            "profile disk and lease require regular files with one link",
        ));
    }
    if (FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }) != opened
    {
        return Err(ProfileResetError::Unsafe(
            "profile disk or lease identity changed during Reset",
        ));
    }
    Ok(opened)
}

#[cfg(unix)]
fn inspect_disk(directory: &File) -> Result<Option<FileIdentity>, ProfileResetError> {
    match stat_at(directory, DISK_LEAF) {
        Ok(stat) => {
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_nlink != 1 {
                return Err(ProfileResetError::Unsafe(
                    "profile disk requires a regular file with one link",
                ));
            }
            let file = open_at(
                directory,
                DISK_LEAF,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                0,
            )?;
            Ok(Some(validate_opened_identity(&file, directory, DISK_LEAF)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProfileResetError::Io(error)),
    }
}

#[cfg(unix)]
fn directory_identity(metadata: &Metadata) -> Result<FileIdentity, ProfileResetError> {
    if !metadata.is_dir() {
        return Err(ProfileResetError::Unsafe(
            "profile directory binding requires a directory",
        ));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(unix)]
fn open_directory_chain(
    root: &File,
    names: &[OsString],
    create: bool,
) -> Result<(File, Vec<FileIdentity>), ProfileResetError> {
    let mut directory = root.try_clone()?;
    let mut identities = Vec::with_capacity(names.len());
    for name in names {
        let name = CString::new(name.as_bytes())
            .map_err(|_| ProfileResetError::Unsafe("invalid rooted profile directory"))?;
        let flags = libc::O_RDONLY
            | libc::O_DIRECTORY
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC;
        let next = match open_at(&directory, &name, flags, 0) {
            Ok(next) => next,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(ProfileResetError::Io(error));
                    }
                }
                open_at(&directory, &name, flags, 0)?
            }
            Err(error) => return Err(ProfileResetError::Io(error)),
        };
        identities.push(directory_identity(&next.metadata()?)?);
        directory = next;
    }
    Ok((directory, identities))
}

#[cfg(unix)]
fn open_at(
    directory: &File,
    name: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(unix)]
fn stat_at(directory: &File, name: &CStr) -> std::io::Result<libc::stat> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { stat.assume_init() })
    }
}
