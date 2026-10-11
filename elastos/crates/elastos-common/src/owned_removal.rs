//! Removal of one exact path below a directory this code owns, never through a link.
//!
//! [`remove_owned`] opens the owned root with `O_DIRECTORY|O_NOFOLLOW` and
//! every folder below it with `openat(O_DIRECTORY|O_NOFOLLOW)`, so a folder
//! swapped for a link fails instead of redirecting the removal elsewhere. The
//! absolute root is also opened one component at a time. A relative root uses
//! the current directory descriptor as its trust anchor. The leaf is inspected
//! without following it: a link is removed itself, a folder is removed entry
//! by entry relative to open descriptors (never following a link inside it),
//! anything else is unlinked. When the caller recorded the leaf's device and
//! inode, expected files are captured in a private directory before their
//! identity is checked and they are deleted. A new file at the original name
//! survives. Changed captured files are restored without overwrite; conflicts
//! stay in the private directory for repair.
//!
//! `scripts/removal-guard.py` refuses raw deletion added to product code;
//! this is the Rust helper it names. Python and shell callers use
//! `scripts/lib/owned_removal.py`, Node callers `scripts/lib/owned-removal.mjs`.

use std::ffi::{CStr, CString};
use std::fs::Metadata;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const DIRECTORY: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Device and inode of a file, recorded when the caller created or verified it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn of_stat(stat: &libc::stat) -> Self {
        // dev_t is i32 on macOS; std's MetadataExt::dev widens it the same way.
        #[cfg(target_os = "macos")]
        let device = stat.st_dev as u64;
        #[cfg(not(target_os = "macos"))]
        let device = stat.st_dev;
        Self {
            device,
            inode: stat.st_ino,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemovalError {
    #[error("{0} is not an exact path below the owned folder")]
    NotExact(PathBuf),
    #[error("a folder on the path to {0} is a link or not a folder; nothing was removed")]
    LinkedParent(PathBuf),
    #[error("{0} is no longer the recorded file; it was kept")]
    IdentityChanged(PathBuf),
    #[error("removing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Removes `root/relative`, where `relative` is plain names only. Returns
/// `Ok(false)` when it or one of its folders does not exist.
pub fn remove_owned(
    root: &Path,
    relative: &Path,
    expected: Option<FileIdentity>,
) -> Result<bool, RemovalError> {
    let not_exact = || RemovalError::NotExact(relative.to_path_buf());
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(not_exact());
        };
        names.push(CString::new(name.as_bytes()).map_err(|_| not_exact())?);
    }
    let leaf = names.pop().ok_or_else(not_exact)?;
    let failed = |source| RemovalError::Io {
        path: root.join(relative),
        source,
    };
    let folder = |result: io::Result<OwnedFd>| match result {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            Err(RemovalError::LinkedParent(relative.to_path_buf()))
        }
        Err(error) => Err(failed(error)),
    };
    let Some(mut directory) = folder(open_root(root))? else {
        return Ok(false);
    };
    for name in &names {
        let Some(inner) = folder(open_directory(Some(&directory), name))? else {
            return Ok(false);
        };
        directory = inner;
    }
    let found = match lstat_at(&directory, &leaf) {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(failed(error)),
    };
    let identity = FileIdentity::of_stat(&found);
    if expected.is_some_and(|expected| expected != identity) {
        return Err(RemovalError::IdentityChanged(relative.to_path_buf()));
    }
    if let Some(expected) = expected.filter(|_| found.st_mode & libc::S_IFMT != libc::S_IFDIR) {
        return match remove_expected_file(&directory, &leaf, expected).map_err(failed)? {
            Some(true) => Ok(true),
            None => Ok(false),
            Some(false) => Err(RemovalError::IdentityChanged(relative.to_path_buf())),
        };
    }
    if found.st_mode & libc::S_IFMT == libc::S_IFDIR {
        remove_tree(&directory, &leaf, identity)
    } else {
        unlink_at(&directory, &leaf, 0)
    }
    .map_err(failed)?;
    Ok(true)
}

fn open_root(root: &Path) -> io::Result<OwnedFd> {
    let mut directory = open_directory(None, if root.is_absolute() { c"/" } else { c"." })?;
    for component in root.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                let name = CString::new(name.as_bytes())
                    .map_err(|_| io::Error::other("invalid removal root"))?;
                directory = open_directory(Some(&directory), &name)?;
            }
            _ => return Err(io::Error::other("invalid removal root")),
        }
    }
    Ok(directory)
}

/// Capture before checking: removal acts only on the private captured entry.
/// None means absent; false means a changed entry was restored without overwrite.
fn remove_expected_file(
    parent: &OwnedFd,
    leaf: &CStr,
    expected: FileIdentity,
) -> io::Result<Option<bool>> {
    let mut random = [0_u8; 16];
    if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let holding = CString::new(format!(".elastos-remove-{}", hex::encode(random))).unwrap();
    if unsafe { libc::mkdirat(parent.as_raw_fd(), holding.as_ptr(), 0o700) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let captured = open_directory(Some(parent), &holding)?;
    let result = (|| {
        if unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                captured.as_raw_fd(),
                c"entry".as_ptr(),
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let found = lstat_at(&captured, c"entry")?;
        if FileIdentity::of_stat(&found) != expected
            || found.st_mode & libc::S_IFMT == libc::S_IFDIR
        {
            // linkat leaves any new original name intact. A restore conflict
            // retains both entries and reports the holding path for repair.
            if unsafe {
                libc::linkat(
                    captured.as_raw_fd(),
                    c"entry".as_ptr(),
                    parent.as_raw_fd(),
                    leaf.as_ptr(),
                    0,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            unlink_at(&captured, c"entry", 0)?;
            return Ok(Some(false));
        }
        unlink_at(&captured, c"entry", 0)?;
        Ok(Some(true))
    })();
    match result {
        Ok(value) => {
            unlink_at(parent, &holding, libc::AT_REMOVEDIR)?;
            Ok(value)
        }
        Err(error) => Err(io::Error::other(format!(
            "captured entry kept in {}/entry for repair: {error}",
            holding.to_string_lossy()
        ))),
    }
}

fn open_directory(at: Option<&OwnedFd>, name: &CStr) -> io::Result<OwnedFd> {
    let at = at.map_or(libc::AT_FDCWD, AsRawFd::as_raw_fd);
    let fd = unsafe { libc::openat(at, name.as_ptr(), DIRECTORY) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn lstat_at(directory: &OwnedFd, name: &CStr) -> io::Result<libc::stat> {
    let mut found: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut found,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(found)
}

fn unlink_at(directory: &OwnedFd, name: &CStr, flags: libc::c_int) -> io::Result<()> {
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the folder `name` below `parent` when it is still `identity`. A
/// read error ends the listing early; the final rmdir then fails, so nothing
/// is reported removed that was not.
fn remove_tree(parent: &OwnedFd, name: &CStr, identity: FileIdentity) -> io::Result<()> {
    let directory = open_directory(Some(parent), name)?;
    let mut opened: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(directory.as_raw_fd(), &mut opened) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if FileIdentity::of_stat(&opened) != identity {
        return Err(io::Error::other(
            "the folder changed while it was being removed",
        ));
    }
    for child in entries(&directory)? {
        let found = lstat_at(&directory, &child)?;
        if found.st_mode & libc::S_IFMT == libc::S_IFDIR {
            remove_tree(&directory, &child, FileIdentity::of_stat(&found))?;
        } else {
            unlink_at(&directory, &child, 0)?;
        }
    }
    unlink_at(parent, name, libc::AT_REMOVEDIR)
}

fn entries(directory: &OwnedFd) -> io::Result<Vec<CString>> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(duplicate) };
        return Err(error);
    }
    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    unsafe { libc::closedir(stream) };
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tree() -> (Scratch, PathBuf, PathBuf) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let temp = Scratch(std::env::temp_dir().join(format!(
            "elastos-owned-removal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(temp.path()).unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (root, outside) = (base.join("root"), base.join("outside"));
        fs::create_dir_all(root.join("owned/nested")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("owned/file"), b"owned").unwrap();
        fs::write(root.join("owned/nested/file"), b"owned").unwrap();
        fs::write(outside.join("file"), b"outside").unwrap();
        (temp, root, outside)
    }

    #[test]
    fn removes_an_exact_file_and_reports_absence() {
        let (_temp, root, _) = tree();
        assert!(remove_owned(&root, Path::new("owned/file"), None).unwrap());
        assert!(!root.join("owned/file").exists());
        assert!(!remove_owned(&root, Path::new("owned/file"), None).unwrap());
        assert!(!remove_owned(&root, Path::new("missing/file"), None).unwrap());
    }

    #[test]
    fn never_removes_through_a_linked_parent_or_root() {
        let (temp, root, outside) = tree();
        symlink(&outside, root.join("linked")).unwrap();
        let error = remove_owned(&root, Path::new("linked/file"), None).unwrap_err();
        assert!(matches!(error, RemovalError::LinkedParent(_)), "{error}");
        let linked_root = temp.path().canonicalize().unwrap().join("linked-root");
        symlink(&outside, &linked_root).unwrap();
        let error = remove_owned(&linked_root, Path::new("file"), None).unwrap_err();
        assert!(matches!(error, RemovalError::LinkedParent(_)), "{error}");
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
    }

    #[test]
    fn a_linked_leaf_is_removed_itself_and_tree_links_are_not_followed() {
        let (_temp, root, outside) = tree();
        symlink(&outside, root.join("owned/nested/to-outside")).unwrap();
        symlink(&outside, root.join("leaf-link")).unwrap();
        assert!(remove_owned(&root, Path::new("leaf-link"), None).unwrap());
        assert!(remove_owned(&root, Path::new("owned"), None).unwrap());
        assert!(!root.join("owned").exists());
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
    }

    #[test]
    fn a_replaced_leaf_is_kept() {
        let (_temp, root, _) = tree();
        let recorded = FileIdentity::of(&fs::symlink_metadata(root.join("owned/file")).unwrap());
        // Renamed, not deleted, so the replacement cannot reuse the inode number.
        fs::rename(root.join("owned/file"), root.join("owned/moved")).unwrap();
        fs::write(root.join("owned/file"), b"replacement").unwrap();
        let error = remove_owned(&root, Path::new("owned/file"), Some(recorded)).unwrap_err();
        assert!(matches!(error, RemovalError::IdentityChanged(_)), "{error}");
        assert_eq!(fs::read(root.join("owned/file")).unwrap(), b"replacement");
        let current = FileIdentity::of(&fs::symlink_metadata(root.join("owned/file")).unwrap());
        assert!(remove_owned(&root, Path::new("owned/file"), Some(current)).unwrap());
    }

    #[test]
    fn linked_ancestor_above_a_real_root_is_refused() {
        let (temp, root, outside) = tree();
        let linked = temp.path().canonicalize().unwrap().join("linked-ancestor");
        symlink(&root, &linked).unwrap();
        let error = remove_owned(&linked.join("owned"), Path::new("file"), None).unwrap_err();
        assert!(matches!(error, RemovalError::LinkedParent(_)), "{error}");
        assert_eq!(fs::read(root.join("owned/file")).unwrap(), b"owned");
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
    }

    #[test]
    fn capture_restores_a_leaf_replaced_after_its_identity_was_checked() {
        let (_temp, root, _) = tree();
        let file = root.join("owned/file");
        let recorded = FileIdentity::of(&fs::symlink_metadata(&file).unwrap());
        let parent = open_root(&root.join("owned")).unwrap();
        fs::rename(&file, root.join("owned/old-file")).unwrap();
        fs::write(&file, b"replacement").unwrap();
        assert_eq!(
            remove_expected_file(&parent, c"file", recorded).unwrap(),
            Some(false)
        );
        assert_eq!(fs::read(file).unwrap(), b"replacement");
        assert!(!fs::read_dir(root.join("owned")).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".elastos-remove-")));
    }

    #[test]
    fn a_parent_swapped_for_a_link_keeps_removal_on_the_open_directory() {
        let (_temp, root, outside) = tree();
        let recorded = FileIdentity::of(&fs::symlink_metadata(root.join("owned/file")).unwrap());
        let parent = open_root(&root.join("owned")).unwrap();
        fs::rename(root.join("owned"), root.join("moved")).unwrap();
        symlink(&outside, root.join("owned")).unwrap();
        assert_eq!(
            remove_expected_file(&parent, c"file", recorded).unwrap(),
            Some(true)
        );
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
        assert!(!root.join("moved/file").exists());
    }

    #[test]
    fn only_plain_relative_names_are_accepted() {
        let (_temp, root, _) = tree();
        for relative in ["", "/etc/passwd", "../outside/file", "owned/../owned/file"] {
            let error = remove_owned(&root, Path::new(relative), None).unwrap_err();
            assert!(
                matches!(error, RemovalError::NotExact(_)),
                "{relative}: {error}"
            );
        }
        assert!(root.join("owned/file").exists());
    }
}
