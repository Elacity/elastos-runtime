//! The Linux owner explicitly grants Browser host access before image acquisition.
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) const ENABLE_COMMAND: &str = "elastos setup --browser";
pub(crate) const SETUP_MESSAGE: &str =
    "Browser is available on this computer. To turn it on, run: elastos setup --browser. Then close Home and start it again with elastos home --browser.";
pub(crate) const KVM_MESSAGE: &str =
    "Browser needs KVM on this computer. After KVM is available, run: elastos setup --browser. Then close Home and start it again with elastos home --browser.";
const ROOT_BOOTSTRAP: &str = include_str!("browser_linux_root.py");
const MAX_HELPER_BYTES: u64 = 1024 * 1024;
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
    let (checksum, payload) = read_admitted_helper(data, platform)?;
    let user = Command::new("/usr/bin/id").arg("-un").output()?;
    anyhow::ensure!(user.status.success(), "Could not identify the Home user");
    let user = String::from_utf8(user.stdout)?;
    enable_with(
        || setup_host_with(data, user.trim(), &checksum, &payload, run_host_command),
        || async {
            super::browser_config::require_linux_host(data)?;
            println!("Preparing Browser Engine image...");
            super::ensure_browser_vm_image_in_context(data, super::FirstPartyCarrierContext::Setup)
                .await
        },
    )
    .await?;
    println!("To remove Browser host access after closing Browser:");
    println!(
        "  sudo /usr/bin/python3 -I {ROOT_HELPER} remove --user {}",
        user.trim()
    );
    println!("Browser is ready. Close Home, then start it again with elastos home --browser.");
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

/// The expected digest belongs to the admitted release, independently of the
/// user-owned helper. Capture bounded bytes once, before sudo can prompt.
fn read_admitted_helper(data: &Path, platform: &str) -> anyhow::Result<(String, Vec<u8>)> {
    let admitted = super::admit_installed_components(data, super::FirstPartyCarrierContext::Setup)?
        .ok_or_else(|| {
            anyhow::anyhow!("Browser administrator setup requires an installed signed release")
        })?;
    let name = "browser-vm-linux-network";
    let component =
        admitted.manifest.external.get(name).ok_or_else(|| {
            anyhow::anyhow!("Signed release is missing the Browser network helper")
        })?;
    let info = super::resolve_platform_info(component, platform).ok_or_else(|| {
        anyhow::anyhow!("Signed release has no Browser network helper for {platform}")
    })?;
    anyhow::ensure!(
        info.extract_path.is_none(),
        "Browser network helper requires a signed plain file"
    );
    let row = admitted.admission.row(name, info)?;
    let checksum = row
        .info()
        .checksum
        .as_deref()
        .and_then(|checksum| checksum.strip_prefix("sha256:"))
        .ok_or_else(|| anyhow::anyhow!("Browser network helper requires an admitted SHA-256"))?;
    let mut payload = Vec::new();
    std::fs::File::open(data.join("scripts/browser-vm-linux-network.py"))?
        .take(MAX_HELPER_BYTES + 1)
        .read_to_end(&mut payload)?;
    anyhow::ensure!(
        payload.len() as u64 <= MAX_HELPER_BYTES,
        "Browser network helper exceeds its size limit"
    );
    super::verify_checksum(name, &payload, row.info())?;
    Ok((checksum.to_owned(), payload))
}

fn run_host_command(
    program: &str,
    args: &[&std::ffi::OsStr],
    payload: Option<&[u8]>,
    quiet: bool,
) -> anyhow::Result<bool> {
    let mut command = Command::new(program);
    command.args(args);
    if quiet {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    if let Some(payload) = payload {
        // sudo keeps its default controlling-tty password prompt. Its stdin
        // carries only the captured data to the fixed, embedded bootstrap.
        let mut child = command.stdin(Stdio::piped()).spawn()?;
        let written = child
            .stdin
            .take()
            .expect("piped helper input")
            .write_all(payload);
        let status = child.wait()?;
        written?;
        Ok(status.success())
    } else {
        Ok(command.status()?.success())
    }
}

fn setup_host_with(
    data: &Path,
    user: &str,
    checksum: &str,
    payload: &[u8],
    mut run: impl FnMut(&str, &[&std::ffi::OsStr], Option<&[u8]>, bool) -> anyhow::Result<bool>,
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
    let needs_setup = !run("/usr/bin/python3", &check, None, true)?;
    if needs_setup {
        println!(
            "Browser needs one-time administrator setup for KVM and its private network devices."
        );
        println!("The setup shows its changes and asks for confirmation. Runtime then runs as your user.");
    }
    // Always replace/verify the root copy, including an already prepared host:
    // the printed removal command and boot service use only this owned file.
    let install = [
        OsStr::new("/usr/bin/python3"),
        OsStr::new("-I"),
        OsStr::new("-c"),
        OsStr::new(ROOT_BOOTSTRAP),
        OsStr::new(checksum),
        OsStr::new(user),
        data.as_os_str(),
        OsStr::new(if needs_setup { "setup" } else { "install" }),
    ];
    anyhow::ensure!(
        run("/usr/bin/sudo", &install, Some(payload), false)?,
        "Browser administrator setup stopped. Retry with {ENABLE_COMMAND}"
    );
    // The helper returns success when confirmation is declined. Prove access
    // before downloading an image, including that declined case.
    anyhow::ensure!(
        run("/usr/bin/python3", &check, None, true)?,
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

    #[test]
    fn privileged_handoff_refuses_user_path_replacement() {
        use sha2::Digest as _;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        let helper = data.join("scripts/browser-vm-linux-network.py");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, b"admitted").unwrap();
        let payload = std::fs::read(&helper).unwrap();
        let expected = format!("{:x}", sha2::Sha256::digest(&payload));
        let mut checks = 0;
        let mut privileged = Vec::new();
        setup_host_with(
            data,
            "home-user",
            &expected,
            &payload,
            |program, args, stdin, quiet| {
                if program != "/usr/bin/sudo" {
                    checks += 1;
                    assert!(stdin.is_none() && quiet);
                    return Ok(checks > 1);
                }
                // The same account swaps the user path during the sudo password wait.
                std::fs::write(&helper, b"changed!").unwrap();
                assert!(!quiet);
                assert_eq!(args[0], "/usr/bin/python3");
                assert_eq!(args[3], ROOT_BOOTSTRAP);
                assert!(!args.contains(&helper.as_os_str()));
                let received = stdin.unwrap();
                let digest = format!("{:x}", sha2::Sha256::digest(received));
                if Some(digest.as_str()) != args[4].to_str() {
                    return Ok(false);
                }
                privileged.extend_from_slice(received);
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(helper).unwrap(), b"changed!");
        assert_eq!(privileged, b"admitted");
        assert_eq!(checks, 2);
    }

    #[test]
    fn privileged_handoff_requires_signed_release_instead_of_local_digest() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("components.json"), b"{}").unwrap();
        assert!(read_admitted_helper(temp.path(), "linux-arm64")
            .unwrap_err()
            .to_string()
            .contains("requires an installed signed release"));
    }

    #[tokio::test]
    async fn explicit_browser_setup_runs_root_helper_before_image_and_checks_decline() {
        for (statuses, prepares) in [
            (vec![true, true, true], true),
            (vec![false, true, true], true),
            (vec![false, false], false),
            (vec![true, false], false),
            (vec![false, true, false], false),
        ] {
            let calls = RefCell::new(Vec::new());
            let mut results = statuses.into_iter();
            let result = enable_with(
                || {
                    setup_host_with(
                        Path::new("/fixture/home with spaces"),
                        "home-user",
                        &"a".repeat(64),
                        b"admitted",
                        |program, args, stdin, quiet| {
                            if program == "/usr/bin/sudo" {
                                assert_eq!(args[0], "/usr/bin/python3");
                                assert_eq!(args[1], "-I");
                                assert_eq!(args[2], "-c");
                                assert_eq!(args[3], ROOT_BOOTSTRAP);
                                assert!(ROOT_BOOTSTRAP.contains(ROOT_HELPER));
                                assert_eq!(args[4], "a".repeat(64).as_str());
                                assert_eq!(args[5], "home-user");
                                assert_eq!(args[6], "/fixture/home with spaces");
                                assert!(args[7] == "setup" || args[7] == "install");
                                assert_eq!(stdin, Some(b"admitted".as_slice()));
                                assert!(!quiet);
                            }
                            calls.borrow_mut().push(program.to_owned());
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
            assert_eq!(calls[0], "/usr/bin/python3");
            assert_eq!(calls[1], "/usr/bin/sudo");
        }
    }
}
