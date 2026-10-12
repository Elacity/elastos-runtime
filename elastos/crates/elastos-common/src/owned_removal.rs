//! Removal of one exact path below a directory this code owns, never through a link.
//!
//! [`remove_owned`] opens the root's parent (the caller's trust anchor), then
//! the root's last name with `openat(O_DIRECTORY|O_NOFOLLOW)`, however the
//! root is spelled (`data`, `data/` and `data/.` all name `data` itself), then
//! every folder below it the same way, so a folder that is a link fails
//! instead of redirecting the removal elsewhere. The leaf is inspected with
//! `fstatat(AT_SYMLINK_NOFOLLOW)` relative to its parent's descriptor: a link
//! is removed itself, a folder is emptied through descriptors opened the same
//! way (never following a link inside it), anything else is unlinked. Every
//! `unlinkat` is relative to a held directory descriptor, so renaming or
//! linking a folder on the path after the walk cannot redirect it. When the
//! caller recorded the leaf's device and inode, a leaf with another identity
//! is kept.
//!
//! Accepted limit: POSIX removes a name, not an inode. Between the identity
//! check and `unlinkat`, and while a folder is being emptied, a process
//! running as the same user can still rename entries inside the owned folder.
//! The owned folder is that user's own private (0700) directory, so no other
//! account can; this is the same limit recorded for #183.
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
    #[error("{0} is not an exact path")]
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

#[cfg(test)]
thread_local! {
    static BEFORE_LEAF_TEST_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Removes `root/relative`, where `relative` is plain names only. Returns
/// `Ok(false)` when it or one of its folders does not exist.
pub fn remove_owned(
    root: &Path,
    relative: &Path,
    expected: Option<FileIdentity>,
) -> Result<bool, RemovalError> {
    let not_exact = |path: &Path| RemovalError::NotExact(path.to_path_buf());
    let c_name = |name: &std::ffi::OsStr, path: &Path| {
        CString::new(name.as_bytes()).map_err(|_| not_exact(path))
    };
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(not_exact(relative));
        };
        names.push(c_name(name, relative)?);
    }
    let leaf = names.pop().ok_or_else(|| not_exact(relative))?;
    // `linked/` or `linked/.` would let the kernel follow `linked` before
    // O_NOFOLLOW sees it, so the root's last name is opened on its own.
    let mut root_components = root.components();
    let Some(Component::Normal(root_name)) = root_components.next_back() else {
        return Err(not_exact(root));
    };
    let anchor = root_components.as_path();
    let anchor = if anchor.as_os_str().is_empty() {
        Path::new(".")
    } else {
        anchor
    };
    let (anchor, root_name) = (c_name(anchor.as_os_str(), root)?, c_name(root_name, root)?);
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
    let anchor = match open_directory(
        None,
        &anchor,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
    ) {
        Ok(anchor) => anchor,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(failed(error)),
    };
    let Some(mut directory) = folder(open_directory(Some(&anchor), &root_name, DIRECTORY))? else {
        return Ok(false);
    };
    for name in &names {
        let Some(inner) = folder(open_directory(Some(&directory), name, DIRECTORY))? else {
            return Ok(false);
        };
        directory = inner;
    }
    #[cfg(test)]
    if let Some(hook) = BEFORE_LEAF_TEST_HOOK.with(|hook| hook.borrow_mut().take()) {
        hook();
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
    if found.st_mode & libc::S_IFMT == libc::S_IFDIR {
        remove_tree(&directory, &leaf, identity)
    } else {
        unlink_at(&directory, &leaf, 0)
    }
    .map_err(failed)?;
    Ok(true)
}

fn open_directory(at: Option<&OwnedFd>, name: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    let at = at.map_or(libc::AT_FDCWD, AsRawFd::as_raw_fd);
    let fd = unsafe { libc::openat(at, name.as_ptr(), flags) };
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
    let directory = open_directory(Some(parent), name, DIRECTORY)?;
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
        let (root, outside) = (temp.path().join("root"), temp.path().join("outside"));
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
    fn never_removes_through_a_linked_parent_or_root_however_spelled() {
        let (temp, root, outside) = tree();
        symlink(&outside, root.join("linked")).unwrap();
        let error = remove_owned(&root, Path::new("linked/file"), None).unwrap_err();
        assert!(matches!(error, RemovalError::LinkedParent(_)), "{error}");
        let linked = temp.path().join("linked-root");
        symlink(&outside, &linked).unwrap();
        let linked = linked.display().to_string();
        for spelling in ["", "/", "/.", "/./", "//"] {
            let root = PathBuf::from(format!("{linked}{spelling}"));
            let error = remove_owned(&root, Path::new("file"), None).unwrap_err();
            assert!(
                matches!(error, RemovalError::LinkedParent(_)),
                "{spelling}: {error}"
            );
        }
        let error = remove_owned(
            &PathBuf::from(format!("{linked}/..")),
            Path::new("file"),
            None,
        );
        assert!(matches!(error, Err(RemovalError::NotExact(_))));
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
        let dotted = PathBuf::from(format!("{}/.", root.display()));
        assert!(remove_owned(&dotted, Path::new("owned/file"), None).unwrap());
    }

    #[test]
    fn a_folder_swapped_for_a_link_after_the_walk_is_not_followed() {
        let (temp, root, outside) = tree();
        let (owned, moved) = (root.join("owned"), temp.path().join("moved"));
        let (hook_owned, hook_moved, hook_outside) =
            (owned.clone(), moved.clone(), outside.clone());
        BEFORE_LEAF_TEST_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook_owned, &hook_moved).unwrap();
                symlink(&hook_outside, &hook_owned).unwrap();
            }))
        });
        assert!(remove_owned(&root, Path::new("owned/file"), None).unwrap());
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
        assert!(!moved.join("file").exists());
        assert!(fs::symlink_metadata(&owned).unwrap().is_symlink());
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
    fn a_leaf_replaced_before_the_identity_check_is_kept() {
        let (_temp, root, outside) = tree();
        let identity = |path: &Path| FileIdentity::of(&fs::symlink_metadata(path).unwrap());
        let (file, nested) = (root.join("owned/file"), root.join("owned/nested"));
        let recorded = identity(&file);
        // Renamed, not deleted, so the replacement cannot reuse the inode number.
        fs::rename(&file, root.join("owned/moved")).unwrap();
        fs::write(&file, b"replacement").unwrap();
        let error = remove_owned(&root, Path::new("owned/file"), Some(recorded)).unwrap_err();
        assert!(matches!(error, RemovalError::IdentityChanged(_)), "{error}");
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
        fs::remove_file(&file).unwrap();
        symlink(outside.join("file"), &file).unwrap();
        let error = remove_owned(&root, Path::new("owned/file"), Some(recorded)).unwrap_err();
        assert!(matches!(error, RemovalError::IdentityChanged(_)), "{error}");
        let folder = identity(&nested);
        fs::rename(&nested, root.join("owned/nested-moved")).unwrap();
        fs::create_dir(&nested).unwrap();
        let error = remove_owned(&root, Path::new("owned/nested"), Some(folder)).unwrap_err();
        assert!(matches!(error, RemovalError::IdentityChanged(_)), "{error}");
        assert!(nested.is_dir());
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
        assert!(remove_owned(&root, Path::new("owned/file"), Some(identity(&file))).unwrap());
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
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
