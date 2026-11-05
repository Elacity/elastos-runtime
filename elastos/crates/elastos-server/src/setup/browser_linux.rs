//! The Linux owner explicitly grants Browser host access before image acquisition.
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) const ENABLE_COMMAND: &str = "elastos setup --browser";
pub(crate) const SETUP_MESSAGE: &str =
    "Browser is available on this computer. To turn it on, run: elastos setup --browser";
pub(crate) const KVM_MESSAGE: &str =
    "Browser needs KVM on this computer. After KVM is available, run: elastos setup --browser";
const ROOT_HELPER: &str = "/usr/local/lib/elastos/browser-vm-linux-network.py";

/// Keep ordinary Home setup and Browser open free of administrator operations.
pub(super) async fn enable(data: &Path, platform: &str) -> anyhow::Result<()> {
    validate_host(platform, kvm_present(Path::new("/dev/kvm")))?;
    let _writer = crate::install_transaction::acquire_installed_writer(data)?;
    let _fresh = if _writer.is_none() {
        Some(crate::install_transaction::InstallationGuard::acquire(
            &std::fs::canonicalize(data)?,
        )?)
    } else {
        None
    };
    let manifest = super::load_manifest_from_path(&data.join("components.json"))?;
    anyhow::ensure!(
        super::browser_config::local_vm_selected(data, platform)?,
        "Browser uses the selected remote Engine. Select the local Engine before running {ENABLE_COMMAND}"
    );
    retire_rootfs_pool(data)?;
    println!("Preparing Browser Engine helpers...");
    super::browser_config::ensure_host_components(
        data,
        &manifest,
        platform,
        super::FirstPartyCarrierContext::Setup,
    )
    .await?;
    let helper = data.join("scripts/browser-vm-linux-network.py");
    super::verify_installed_component_binary(data, "browser-vm-linux-network", &helper)?;
    let user = Command::new("id").arg("-un").output()?;
    anyhow::ensure!(user.status.success(), "Could not identify the Home user");
    let user = String::from_utf8(user.stdout)?;
    enable_with(
        || {
            setup_host_with(data, user.trim(), |program, args, quiet| {
                let mut command = Command::new(program);
                command.args(args);
                if quiet {
                    command
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                }
                Ok(command.status()?.success())
            })
        },
        || async {
            super::browser_config::require_linux_host(data)?;
            println!("Preparing Browser Engine image...");
            super::ensure_browser_vm_image_in_context(data, super::FirstPartyCarrierContext::Setup)
                .await
        },
    )
    .await?;
    println!("Browser is ready. Close Home, then start it with elastos home --browser.");
    println!("To remove Browser host access after closing Browser:");
    println!(
        "  sudo /usr/bin/python3 -I {ROOT_HELPER} remove --user {}",
        user.trim()
    );
    Ok(())
}

fn retire_rootfs_pool(data: &Path) -> anyhow::Result<()> {
    retire_rootfs_pool_for_owner(data, unsafe { libc::geteuid() })
}

fn retire_rootfs_pool_for_owner(data: &Path, owner: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let relative = Path::new("browser-vm/rootfs-pool");
    let mut expected = None;
    for name in [Path::new(""), Path::new("browser-vm"), relative] {
        let path = data.join(name);
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            metadata.uid() == owner,
            "Browser pool cleanup requires the Home user's files"
        );
        if name != relative {
            anyhow::ensure!(
                metadata.is_dir() && !metadata.is_symlink(),
                "Browser pool cleanup requires plain parent directories"
            );
        } else {
            if !metadata.is_dir() && !metadata.is_symlink() {
                return Ok(());
            }
            expected = Some(elastos_common::owned_removal::FileIdentity::of(&metadata));
        }
    }
    if elastos_common::owned_removal::remove_owned(data, relative, expected)? {
        println!(
            "Removed retired Browser rootfs pool: {}",
            data.join("browser-vm/rootfs-pool").display()
        );
    }
    Ok(())
}

pub(super) fn kvm_present(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_char_device())
}

fn validate_host(platform: &str, kvm_present: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        platform == "linux-arm64",
        "{ENABLE_COMMAND} requires a supported Linux ARM64 Browser host"
    );
    anyhow::ensure!(kvm_present, "{KVM_MESSAGE}");
    Ok(())
}

async fn enable_with<F, P, Fut>(setup_host: F, prepare_image: P) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
    P: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    setup_host()?;
    prepare_image().await
}

fn setup_host_with(
    data: &Path,
    user: &str,
    mut run: impl FnMut(&str, &[&std::ffi::OsStr], bool) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    use std::ffi::OsStr;
    let helper = data.join("scripts/browser-vm-linux-network.py");
    let check = [
        OsStr::new("-I"),
        helper.as_os_str(),
        OsStr::new("check"),
        OsStr::new("--data-dir"),
        data.as_os_str(),
    ];
    if run("/usr/bin/python3", &check, true)? {
        return Ok(());
    }
    println!("Browser needs one-time administrator setup for KVM and its private network devices.");
    println!(
        "The setup shows its changes and asks for confirmation. Runtime then runs as your user."
    );
    let install = [
        OsStr::new("install"),
        OsStr::new("-D"),
        OsStr::new("-o"),
        OsStr::new("root"),
        OsStr::new("-g"),
        OsStr::new("root"),
        OsStr::new("-m"),
        OsStr::new("755"),
        helper.as_os_str(),
        OsStr::new(ROOT_HELPER),
    ];
    anyhow::ensure!(
        run("sudo", &install, false)?,
        "Browser helper installation stopped. Retry with {ENABLE_COMMAND}"
    );
    let setup = [
        OsStr::new("/usr/bin/python3"),
        OsStr::new("-I"),
        OsStr::new(ROOT_HELPER),
        OsStr::new("setup"),
        OsStr::new("--user"),
        OsStr::new(user),
        OsStr::new("--data-dir"),
        data.as_os_str(),
    ];
    anyhow::ensure!(
        run("sudo", &setup, false)?,
        "Browser administrator setup stopped. Retry with {ENABLE_COMMAND}"
    );
    // The helper returns success when confirmation is declined. Prove access
    // before downloading an image, including that declined case.
    anyhow::ensure!(
        run("/usr/bin/python3", &check, true)?,
        "Browser host access still needs setup. Run {ENABLE_COMMAND} and confirm its changes"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn linux_browser_opt_in_retires_only_owned_pool_without_following_links() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let data = base.join("data");
        let outside = base.join("outside");
        let pool = data.join("browser-vm/rootfs-pool");
        std::fs::create_dir_all(&pool).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(pool.join("old.raw"), b"retired").unwrap();
        std::fs::write(data.join("browser-vm/rootfs.ext4"), b"image").unwrap();
        std::fs::write(outside.join("keep"), b"profile").unwrap();
        assert!(
            retire_rootfs_pool_for_owner(&data, unsafe { libc::geteuid() }.wrapping_add(1))
                .is_err()
        );
        assert!(pool.join("old.raw").is_file());
        retire_rootfs_pool(&data).unwrap();
        assert!(!pool.exists());
        assert_eq!(
            std::fs::read(data.join("browser-vm/rootfs.ext4")).unwrap(),
            b"image"
        );
        symlink(&outside, &pool).unwrap();
        retire_rootfs_pool(&data).unwrap();
        assert!(pool.symlink_metadata().is_err());
        // A linked parent is refused; its target and the owner's profile stay.
        let linked = base.join("linked");
        std::fs::create_dir(&linked).unwrap();
        symlink(&outside, linked.join("browser-vm")).unwrap();
        assert!(retire_rootfs_pool(&linked).is_err());
        assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"profile");
    }

    #[test]
    fn linux_browser_opt_in_requires_kvm_before_preparing_any_files() {
        assert!(validate_host("linux-arm64", true).is_ok());
        assert_eq!(
            validate_host("linux-arm64", false).unwrap_err().to_string(),
            KVM_MESSAGE
        );
        for platform in ["linux-amd64", "darwin-arm64", "darwin-amd64"] {
            assert!(validate_host(platform, true).is_err());
        }
        let temp = tempfile::NamedTempFile::new().unwrap();
        assert!(!kvm_present(temp.path()));
        assert!(kvm_present(Path::new("/dev/null")));
    }

    #[tokio::test]
    async fn linux_browser_opt_in_refuses_other_platforms_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("unprepared");
        assert!(enable(&data, "darwin-arm64").await.is_err());
        assert!(!data.exists());
    }

    #[tokio::test]
    async fn explicit_browser_setup_runs_root_helper_before_image_and_checks_decline() {
        for (statuses, prepares) in [
            (vec![true], true),
            (vec![false, true, true, true], true),
            (vec![false, false], false),
            (vec![false, true, false], false),
            (vec![false, true, true, false], false),
        ] {
            let calls = RefCell::new(Vec::new());
            let mut results = statuses.into_iter();
            let result = enable_with(
                || {
                    setup_host_with(
                        Path::new("/fixture/home with spaces"),
                        "home-user",
                        |program, args, quiet| {
                            calls
                                .borrow_mut()
                                .push(format!("{program} {:?} quiet={quiet}", args));
                            Ok(results.next().unwrap())
                        },
                    )
                },
                || async {
                    calls.borrow_mut().push("image".into());
                    Ok(())
                },
            )
            .await;
            assert_eq!(result.is_ok(), prepares);
            let calls = calls.into_inner();
            assert_eq!(calls.last().unwrap() == "image", prepares);
            if calls.len() == 5 {
                assert!(calls[1].starts_with("sudo [\"install\", \"-D\""));
                assert!(calls[1]
                    .contains("/fixture/home with spaces/scripts/browser-vm-linux-network.py"));
                assert!(calls[2].starts_with(&format!(
                    "sudo [\"/usr/bin/python3\", \"-I\", \"{ROOT_HELPER}\", \"setup\""
                )));
                assert!(calls[2].contains(
                    "\"--user\", \"home-user\", \"--data-dir\", \"/fixture/home with spaces\""
                ));
                assert!(calls[3].starts_with("/usr/bin/python3"));
            }
        }
    }
}
