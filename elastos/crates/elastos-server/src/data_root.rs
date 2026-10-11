//! The Runtime data root holds keys, attach secrets and owner data, so it is
//! private to the account that runs the Runtime.
//!
//! Startup calls [`require_private_data_root`] before anything else touches the
//! root. It repairs what it safely can: a missing root is created 0700, and a
//! root this account owns that others can read (0755 from older releases or a
//! loose umask) is narrowed to 0700 on every start. It refuses only what a
//! mode change cannot make safe: a symlink, a folder owned by another account,
//! or a folder every account could write to.

use std::path::Path;

/// Create `path` and every missing parent with mode 0700 (umask permitting).
pub fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Make the data root private (0700), creating it when missing, and remove
/// files earlier releases left that nothing uses. Refuses a root that is a
/// symlink, belongs to another account, or that every account could write to.
pub fn require_private_data_root(data_dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        require_private_data_root_unix(data_dir)
    }
    #[cfg(not(unix))]
    {
        create_private_dir_all(data_dir)?;
        Ok(())
    }
}

#[cfg(unix)]
fn require_private_data_root_unix(data_dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let shown = data_dir.display();
    let fix = crate::install_transaction::shell_quote_path(data_dir);
    let linked = match std::fs::symlink_metadata(data_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir_all(data_dir).map_err(|error| {
                anyhow::anyhow!("ElastOS cannot create its data folder {shown} ({error}); check that this account can write to the folder that contains it.")
            })?;
            std::fs::symlink_metadata(data_dir)?
        }
        Err(error) => anyhow::bail!(
            "ElastOS cannot read its data folder {shown} ({error}); check that this account owns it."
        ),
    };
    anyhow::ensure!(
        linked.is_dir() && !linked.file_type().is_symlink(),
        "ElastOS data folder {shown} is a link or a file, not a folder; move it aside and start ElastOS again to create a new one."
    );

    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(data_dir)
        .map_err(|error| {
            anyhow::anyhow!(
                "ElastOS cannot open its data folder {shown} ({error}); check that this account owns it."
            )
        })?;
    let opened = directory.metadata()?;
    anyhow::ensure!(
        opened.dev() == linked.dev() && opened.ino() == linked.ino(),
        "ElastOS data folder {shown} changed while it was checked; start ElastOS again."
    );
    let euid = unsafe { libc::geteuid() };
    anyhow::ensure!(
        opened.uid() == euid,
        "ElastOS data folder {shown} belongs to another account; start ElastOS as that account, or as root run: chown -R {euid} {fix}"
    );

    let mode = opened.mode() & 0o7777;
    anyhow::ensure!(
        mode & 0o002 == 0,
        "ElastOS data folder {shown} can be changed by every account on this computer, so ElastOS will not trust it; check its contents, then run: chmod 700 {fix}"
    );
    if mode & 0o077 != 0 {
        if unsafe { libc::fchmod(std::os::fd::AsRawFd::as_raw_fd(&directory), 0o700) } != 0 {
            anyhow::bail!(
                "ElastOS could not make its data folder {shown} private ({}); run: chmod 700 {fix}",
                std::io::Error::last_os_error()
            );
        }
        tracing::info!("Made the ElastOS data folder {shown} private (mode 0700)");
    }
    remove_retired_local_ca(data_dir)
}

/// Files of the local certificate authority that earlier releases created in
/// the data root. Nothing uses them now; the private key is removed so no
/// certificate can be issued from it.
#[cfg(unix)]
const RETIRED_LOCAL_CA_FILES: &[&str] = &["ca.key", "ca.pem", "tls.key", "tls.pem", "tls.ips"];

#[cfg(unix)]
fn remove_retired_local_ca(data_dir: &Path) -> anyhow::Result<()> {
    for name in RETIRED_LOCAL_CA_FILES {
        let path = data_dir.join(name);
        match std::fs::symlink_metadata(&path) {
            // unlink never follows a symlink, so a link is removed, not its target.
            Ok(metadata) if !metadata.is_dir() => std::fs::remove_file(&path)
                .map_err(|error| anyhow::anyhow!("cannot remove {}: {error}", path.display()))?,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => anyhow::bail!("cannot inspect {}: {error}", path.display()),
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn readable_root_is_made_private_on_every_start() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("elastos");
        std::fs::create_dir(&root).unwrap();
        for loose in [0o755, 0o775, 0o750, 0o711] {
            chmod(&root, loose);
            require_private_data_root(&root).unwrap();
            assert_eq!(mode(&root), 0o700, "from {loose:o}");
        }
    }

    #[test]
    fn world_writable_root_refuses_with_the_fix_and_is_left_alone() {
        for loose in [0o777, 0o703, 0o1777] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("elastos");
            std::fs::create_dir(&root).unwrap();
            chmod(&root, loose);
            let error = require_private_data_root(&root).unwrap_err().to_string();
            assert!(
                error.contains(&format!("chmod 700 '{}'", root.display())),
                "{error}"
            );
            assert_eq!(mode(&root), loose);
        }
    }

    #[test]
    fn missing_root_is_created_private_under_a_permissive_umask() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("a/b/elastos");
        let previous = unsafe { libc::umask(0o022) };
        let result = require_private_data_root(&root);
        unsafe { libc::umask(previous) };
        result.unwrap();
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&temp.path().join("a/b")), 0o700);
    }

    #[test]
    fn symlinked_root_refuses_and_target_is_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        chmod(&target, 0o755);
        let root = temp.path().join("elastos");
        std::os::unix::fs::symlink(&target, &root).unwrap();

        let error = require_private_data_root(&root).unwrap_err().to_string();
        assert!(error.contains("not a folder"), "{error}");
        assert_eq!(mode(&target), 0o755);
    }

    #[test]
    fn startup_removes_the_retired_local_ca_without_following_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("elastos");
        create_private_dir_all(&root).unwrap();
        for name in RETIRED_LOCAL_CA_FILES {
            std::fs::write(root.join(name), b"old").unwrap();
        }
        let outside = temp.path().join("outside.pem");
        std::fs::write(&outside, b"keep").unwrap();
        std::fs::remove_file(root.join("ca.pem")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("ca.pem")).unwrap();

        require_private_data_root(&root).unwrap();

        for name in RETIRED_LOCAL_CA_FILES {
            assert!(
                std::fs::symlink_metadata(root.join(name)).is_err(),
                "{name}"
            );
        }
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }
}
