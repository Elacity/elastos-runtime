//! Receipt-bound image acquisition through the existing first-party installer.
//! This prepares image bytes; Engine host admission still owns launch readiness.

use super::{ComponentsManifest, PlatformInfo};
use anyhow::{bail, ensure, Context};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(super) const NAME: &str = "browser-vm-image";
pub(super) const INSTALL_PATH: &str = "browser-vm/image-set";
const RECEIPT: &str = "browser-vm-rootfs-manifest.json";
const RECEIPT_HASH: &str = ".elastos-image-manifest-sha256";
const FILES: [&str; 4] = [RECEIPT, "rootfs.ext4", "vmlinux", "initrd"];
// Existing Carrier file replies are buffered and bounded to 200 MiB.
const MAX_ARCHIVE_SIZE: u64 = 200 * 1024 * 1024;
const STAGING_PREFIX: &str = ".browser-image-stage-";
const VERIFIED_SET_CACHE_LIMIT: usize = 8;

// Like AuthState's verified-file cache, use inode plus nanosecond change time:
// a same-size rewrite with restored mtime still requires verification.
#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
    len: u64,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

#[derive(Clone, PartialEq, Eq)]
struct VerifiedSetKey {
    paths: [PathBuf; 4],
    platform: String,
    release: String,
    identities: Vec<(FileIdentity, FileIdentity)>,
}

fn verified_sets() -> &'static Mutex<VecDeque<VerifiedSetKey>> {
    static CACHE: OnceLock<Mutex<VecDeque<VerifiedSetKey>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn file_identity(metadata: &fs::Metadata) -> anyhow::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mode: metadata.mode(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
    #[cfg(not(unix))]
    bail!("Browser image verification cache requires Unix file identity")
}

fn image_identities(paths: &[PathBuf; 4]) -> anyhow::Result<Vec<(FileIdentity, FileIdentity)>> {
    paths
        .iter()
        .map(|path| {
            let target = fs::metadata(path)?;
            ensure!(
                target.is_file(),
                "Browser image artifact must be a regular file"
            );
            Ok((
                file_identity(&fs::symlink_metadata(path)?)?,
                file_identity(&target)?,
            ))
        })
        .collect()
}

fn verify_cached_payload(
    paths: &[PathBuf; 4],
    platform: &str,
    release: &str,
    verify: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    verify_cached_payload_in(verified_sets(), paths, platform, release, verify)
}

fn verify_cached_payload_in(
    cache: &Mutex<VecDeque<VerifiedSetKey>>,
    paths: &[PathBuf; 4],
    platform: &str,
    release: &str,
    verify: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    // Callers run on the blocking pool. Serialize verification with this small
    // cache so concurrent opens share one hash pass, including a cold miss.
    let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
    let key = VerifiedSetKey {
        paths: paths.clone(),
        platform: platform.to_owned(),
        release: release.to_owned(),
        identities: image_identities(paths)?,
    };
    let checked = (|| -> anyhow::Result<()> {
        if !cache.iter().any(|entry| entry == &key) {
            verify()?;
        }
        // Replacement during verification fails this admission and invalidates
        // any earlier successful entry before another caller can use it.
        ensure!(
            image_identities(paths)? == key.identities,
            "Browser image set changed during verification; retry preparation"
        );
        Ok(())
    })();
    cache.retain(|entry| entry.paths != key.paths);
    checked?;
    cache.push_back(key);
    while cache.len() > VERIFIED_SET_CACHE_LIMIT {
        cache.pop_front();
    }
    Ok(())
}

fn guest_platform(platform: &str) -> anyhow::Result<&str> {
    match platform {
        "darwin-arm64" | "linux-arm64" => Ok("linux-arm64"),
        "linux-amd64" => Ok("linux-amd64"),
        _ => bail!("Browser local Engine image acquisition is unsupported for {platform}"),
    }
}

pub(super) fn component_info<'a>(
    manifest: &'a ComponentsManifest,
    platform: &str,
) -> anyhow::Result<&'a PlatformInfo> {
    guest_platform(platform)?;
    let component = manifest.external.get(NAME).context(
        "Browser image release package is unavailable; install a release with a Browser local Engine image package or select an approved remote Engine",
    )?;
    // Images require an exact host row. A universal capsule row cannot certify an Engine host.
    let info = component
        .platforms
        .get(platform)
        .with_context(|| format!("Browser image release package is unavailable for {platform}"))?;
    ensure!(
        super::resolve_install_path(component, Some(info)) == Some(INSTALL_PATH),
        "Browser image package must install to {INSTALL_PATH}"
    );
    validate_info(info)?;
    Ok(info)
}

fn validate_info(info: &PlatformInfo) -> anyhow::Result<()> {
    ensure!(
        info.install_path.as_deref() == Some(INSTALL_PATH)
            && info.extract_path.as_deref() == Some(NAME)
            && info.strategy.as_deref() == Some(NAME)
            && info.binary_path.is_none()
            && info.source.is_none(),
        "Browser image package requires its atomic image-set layout and strategy"
    );
    let path = info
        .release_path
        .as_deref()
        .context("Browser image package requires a trusted-source release_path")?;
    ensure!(
        path.ends_with(".tar.gz")
            && Path::new(path)
                .components()
                .all(|p| matches!(p, std::path::Component::Normal(_))),
        "Browser image release_path must be a relative tar.gz artifact path"
    );
    let checksum = info.checksum.as_deref().unwrap_or_default();
    let digest = checksum.strip_prefix("sha256:").unwrap_or_default();
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Browser image release package requires a stamped lowercase SHA-256 checksum"
    );
    ensure!(
        info.size.is_some_and(|size| size > 0),
        "Browser image release package requires its archive size"
    );
    ensure!(info.size.is_some_and(|size| size <= MAX_ARCHIVE_SIZE), "Browser image archive exceeds the current 200 MiB Carrier file limit; release acquisition requires a supported artifact transport");
    Ok(())
}

fn aliases(platform: &str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("browser-vm/rootfs.ext4", "rootfs.ext4"),
        ("browser-vm/browser-vm-rootfs-manifest.json", RECEIPT),
        ("bin/vmlinux", "vmlinux"),
        (
            if platform == "darwin-arm64" {
                "bin/initrd"
            } else {
                "browser-vm/initrd"
            },
            "initrd",
        ),
    ]
}

fn check_aliases(data: &Path, platform: &str, require: bool) -> anyhow::Result<()> {
    for (alias, file) in aliases(platform) {
        let path = data.join(alias);
        match fs::symlink_metadata(&path) {
            Ok(meta) => ensure!(
                meta.file_type().is_symlink() && fs::read_link(&path)? == data.join(INSTALL_PATH).join(file),
                "Browser image path {} belongs to another installation; preserve it and use the release migration procedure", path.display()
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && !require => {},
            Err(err) => return Err(err).with_context(|| format!("Browser image path {} requires repair", path.display())),
        }
    }
    Ok(())
}

pub(super) fn validate_request(
    data: &Path,
    info: &PlatformInfo,
    dest: &Path,
    platform: &str,
) -> anyhow::Result<()> {
    guest_platform(platform)?;
    validate_info(info)?;
    ensure!(
        data.is_absolute() && dest == data.join(INSTALL_PATH),
        "Browser image destination must be the Runtime image-set directory"
    );
    for directory in [
        data.to_path_buf(),
        data.join("browser-vm"),
        data.join("bin"),
        dest.to_path_buf(),
    ] {
        match fs::symlink_metadata(&directory) {
            Ok(meta) => ensure!(
                meta.file_type().is_dir(),
                "Browser image directory must be a regular directory: {}",
                directory.display()
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    check_aliases(data, platform, false)
}

fn verify_payload(bundle: &Path, platform: &str) -> anyhow::Result<()> {
    for name in FILES {
        ensure!(
            fs::symlink_metadata(bundle.join(name))?
                .file_type()
                .is_file(),
            "Browser image package {name} must be a regular file"
        );
    }
    verify_payload_paths(&FILES.map(|name| bundle.join(name)), platform)
}

pub(super) fn verify_legacy_or_missing_release(data: &Path, platform: &str) -> anyhow::Result<()> {
    guest_platform(platform)?;
    let initrd = if platform == "darwin-arm64" {
        "bin/initrd"
    } else {
        "browser-vm/initrd"
    };
    // Existing source-home installations own these paths. Verify their bytes and
    // receipt in place; setup acquisition neither copies nor rewrites them.
    let paths = [
        data.join("browser-vm").join(RECEIPT),
        data.join("browser-vm/rootfs.ext4"),
        data.join("bin/vmlinux"),
        data.join(initrd),
    ];
    verify_cached_payload(&paths, platform, "legacy", || verify_payload_paths(&paths, platform))
        .map_err(|err| anyhow::anyhow!("Browser image release package metadata is unavailable and the installed image set requires preparation ({err}); install a release with browser-vm-image metadata or select an approved remote Engine"))
}

fn verify_payload_paths(paths: &[PathBuf; 4], platform: &str) -> anyhow::Result<()> {
    let target = guest_platform(platform)?;
    let receipt_path = &paths[0];
    ensure!(
        fs::metadata(receipt_path)?.file_type().is_file()
            && fs::metadata(&receipt_path)?.len() <= 1024 * 1024,
        "Browser image receipt must be a regular file of at most 1 MiB"
    );
    let receipt: Value = serde_json::from_slice(&fs::read(receipt_path)?)?;
    ensure!(
        receipt["schema"] == "elastos.browser.vm-rootfs-build/v1" && receipt["ok"] == true,
        "Browser image build receipt is invalid"
    );
    ensure!(
        receipt["target_platform"] == target,
        "Browser image architecture does not match {platform}"
    );
    for (path, name, entry) in [
        (&paths[1], "rootfs.ext4", &receipt),
        (&paths[2], "vmlinux", &receipt["kernel"]),
        (&paths[3], "initrd", &receipt["initrd"]),
    ] {
        let meta =
            fs::metadata(path).with_context(|| format!("Browser image set is missing {name}"))?;
        let hash = entry["sha256"].as_str().unwrap_or_default();
        ensure!(
            meta.file_type().is_file()
                && meta.len() > 0
                && entry["size"].as_u64() == Some(meta.len()),
            "Browser image {name} size or file type is invalid"
        );
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && super::file_matches_checksum(&path, &format!("sha256:{hash}"))?,
            "Browser image {name} checksum mismatch"
        );
    }
    let preflight = &receipt["preflight"];
    ensure!(
        preflight["ok"] == true && preflight["audio_default_ready"] == true,
        "Browser image preflight must verify media and audio dependencies"
    );
    for key in ["missing", "manifest_errors", "script_errors"] {
        ensure!(
            preflight[key].as_array().is_some_and(Vec::is_empty),
            "Browser image preflight {key} must be empty"
        );
    }
    for (group, required) in [
        ("required", "manifest init native_proxy runtime_relay guest_control_bridge control_service selkies_start node chromium xvfb python3 gst_inspect"),
        ("optional_audio", "pipewire pipewire_pulse wireplumber pw_cli"),
    ] {
        for name in required.split_whitespace() {
            ensure!(preflight[group][name]["ok"] == true, "Browser image preflight requires {name}");
        }
    }
    let contract = &preflight["manifest"];
    let expected = json!({"schema":"elastos.browser.vm-target/v1", "engine":"chromium_microvm", "network_mode":"runtime_net_only", "direct_network":false, "wallet_injection":false, "media_transport":"runtime_relay", "display_mode":"webrtc_remote_display", "guarantee_level":"mechanism_microvm", "control_transport":"vsock_relay", "control_port":19092});
    for (key, value) in expected.as_object().unwrap() {
        ensure!(
            contract[key] == *value,
            "Browser image contract {key} is incompatible"
        );
    }
    ensure!(
        matches!(
            contract["runtime_exit_transport"].as_str(),
            Some("carrier_stream" | "vsock_relay")
        ) && matches!(
            contract["display_backend"].as_str(),
            Some("vm_selkies_gstreamer_webrtc" | "vm_native_webrtc")
        ),
        "Browser image transport contract is incompatible"
    );
    Ok(())
}

pub(super) fn verify_installed(
    data: &Path,
    info: &PlatformInfo,
    platform: &str,
) -> anyhow::Result<()> {
    let dest = data.join(INSTALL_PATH);
    validate_request(data, info, &dest, platform)?;
    check_aliases(data, platform, true)?;
    ensure!(
        super::extracted_bundle_cache_stale_reason(&dest, info).is_none(),
        "Browser image archive identity is stale"
    );
    ensure!(
        fs::read_to_string(dest.join(RECEIPT_HASH))?.trim()
            == super::compute_sha256_checksum(&dest.join(RECEIPT))?,
        "Browser image receipt identity is stale"
    );
    let paths = FILES.map(|name| dest.join(name));
    verify_cached_payload(&paths, platform, &serde_json::to_string(info)?, || {
        verify_payload(&dest, platform)
    })
}

pub(super) fn install_archive(
    data: &Path,
    bytes: &[u8],
    info: &PlatformInfo,
    platform: &str,
) -> anyhow::Result<()> {
    let dest = data.join(INSTALL_PATH);
    validate_request(data, info, &dest, platform)?;
    ensure!(
        info.size == Some(bytes.len() as u64),
        "Browser image archive is incomplete: size mismatch"
    );
    super::verify_checksum(NAME, bytes, info)?;
    let parent = dest.parent().unwrap();
    fs::create_dir_all(parent)?;
    let _install_lock = lock_installation(parent)?;
    validate_request(data, info, &dest, platform)?;
    // A process interruption releases the lock. The next install discards only
    // this installer's abandoned stage directories before starting a fresh set.
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    // Same-volume staging allows a single rename/exchange at the commit point.
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)?;
    let bundle = staging.path().join(NAME);
    fs::create_dir(&bundle)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut found = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path == Path::new(NAME) && entry.header().entry_type().is_dir() {
            continue;
        }
        let name = path
            .strip_prefix(NAME)?
            .to_str()
            .context("Browser image archive path is not UTF-8")?;
        ensure!(
            FILES.contains(&name)
                && entry.header().entry_type().is_file()
                && found.insert(name.to_owned()),
            "Browser image archive contains an unexpected, duplicate or linked file: {}",
            path.display()
        );
        ensure!(
            entry.size() > 0
                && entry.size()
                    <= if name == RECEIPT {
                        1024 * 1024
                    } else {
                        64 * 1024 * 1024 * 1024
                    },
            "Browser image archive file size is outside the package bound"
        );
        check_disk_space(&bundle, entry.size())?;
        entry.unpack(bundle.join(name))?;
    }
    // Read the gzip trailer too; tar iteration stops at its end-of-archive blocks.
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())?;
    ensure!(
        found.len() == FILES.len(),
        "Browser image archive is missing a required image-set file"
    );
    verify_payload(&bundle, platform)?;
    super::write_platform_cache_metadata(info, &bundle)?;
    fs::write(
        bundle.join(RECEIPT_HASH),
        super::compute_sha256_checksum(&bundle.join(RECEIPT))?,
    )?;
    for file in fs::read_dir(&bundle)? {
        fs::File::open(file?.path())?.sync_all()?;
    }
    let mut created = Vec::<PathBuf>::new();
    let outcome = (|| {
        check_aliases(data, platform, false)?;
        for (alias, file) in aliases(platform) {
            let alias = data.join(alias);
            if fs::symlink_metadata(&alias).is_ok() {
                continue;
            }
            fs::create_dir_all(alias.parent().unwrap())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(dest.join(file), &alias)?;
            #[cfg(not(unix))]
            bail!("Browser image acquisition requires a supported Unix Engine host");
            created.push(alias);
        }
        replace_directory(&bundle, &dest)
    })();
    if outcome.is_err() {
        for path in created.iter().rev() {
            let _ = fs::remove_file(path);
        }
    }
    // TempDir removes the old set after exchange, or failed staging before it.
    outcome
}

fn lock_installation(parent: &Path) -> anyhow::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join(".browser-image-install.lock"))?;
        ensure!(
            file.metadata()?.is_file(),
            "Browser image install lock must be a regular file"
        );
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Browser image install lock failed");
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    bail!("Browser image installation requires a supported Unix host")
}

fn check_disk_space(path: &Path, needed: u64) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Browser image free-space check failed");
        }
        let stats = unsafe { stats.assume_init() };
        disk_budget(
            needed,
            (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64),
            (stats.f_blocks as u64).saturating_mul(stats.f_frsize as u64),
        )
    }
    #[cfg(not(unix))]
    bail!("Browser image free-space check requires a supported Unix host")
}

fn disk_budget(needed: u64, available: u64, total: u64) -> anyhow::Result<()> {
    ensure!(needed <= available.saturating_sub(total / 10), "Browser image preparation needs {needed} bytes while keeping 10% free space; free disk space and retry (previous set preserved)");
    Ok(())
}

fn replace_directory(staged: &Path, dest: &Path) -> anyhow::Result<()> {
    if !dest.exists() {
        fs::rename(staged, dest)?;
        return Ok(());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let staged = std::ffi::CString::new(staged.as_os_str().as_bytes())?;
        let dest = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
        // Both paths name owned directories on the same filesystem. An unsupported
        // exchange fails with the previous installation still in place.
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                staged.as_ptr(),
                libc::AT_FDCWD,
                dest.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                staged.as_ptr(),
                libc::AT_FDCWD,
                dest.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Browser image atomic replacement failed; previous set preserved");
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("Browser image atomic replacement is unsupported on this host")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    fn sha(bytes: &[u8]) -> String {
        hex::encode(sha2::Sha256::digest(bytes))
    }

    fn fixture(rootfs: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut receipt = json!({
            "schema":"elastos.browser.vm-rootfs-build/v1", "ok":true, "target_platform":"linux-arm64",
            "size":rootfs.len(), "sha256":sha(rootfs),
            "kernel":{"size":6,"sha256":sha(b"kernel")},
            "initrd":{"size":6,"sha256":sha(b"initrd")},
            "preflight":{"ok":true,"audio_default_ready":true,"missing":[],"manifest_errors":[],"script_errors":[],
                "manifest":{"schema":"elastos.browser.vm-target/v1","engine":"chromium_microvm","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"media_transport":"runtime_relay","display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm","control_transport":"vsock_relay","control_port":19092,"runtime_exit_transport":"vsock_relay","display_backend":"vm_selkies_gstreamer_webrtc"}}
        });
        for (group, names) in [("required", "manifest init native_proxy runtime_relay guest_control_bridge control_service selkies_start node chromium xvfb python3 gst_inspect"), ("optional_audio", "pipewire pipewire_pulse wireplumber pw_cli")] {
            receipt["preflight"][group] = names.split_whitespace().map(|name| (name.to_owned(), json!({"ok":true}))).collect::<serde_json::Map<_,_>>().into();
        }
        vec![
            (RECEIPT.into(), serde_json::to_vec(&receipt).unwrap()),
            ("rootfs.ext4".into(), rootfs.into()),
            ("vmlinux".into(), b"kernel".to_vec()),
            ("initrd".into(), b"initrd".to_vec()),
        ]
    }

    fn archive(files: &[(String, Vec<u8>)]) -> (Vec<u8>, PlatformInfo) {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{NAME}/{name}"), &bytes[..])
                .unwrap();
        }
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let info = serde_json::from_value(json!({"install_path":INSTALL_PATH,"extract_path":NAME,"strategy":NAME,"release_path":"test/browser-image.tar.gz","checksum":format!("sha256:{}",sha(&bytes)),"size":bytes.len()})).unwrap();
        (bytes, info)
    }

    fn manifest(info: &PlatformInfo) -> ComponentsManifest {
        serde_json::from_value(json!({"external":{NAME:{"install_path":INSTALL_PATH,"platforms":{"darwin-arm64":info}}},"profiles":{}})).unwrap()
    }

    #[test]
    fn browser_image_install_update_and_repair_keep_one_complete_set() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        let (old, old_info) = archive(&fixture(b"first-image"));
        install_archive(data, &old, &old_info, "darwin-arm64").unwrap();
        verify_installed(data, &old_info, "darwin-arm64").unwrap();
        let (next, info) = archive(&fixture(b"next-image"));
        install_archive(data, &next, &info, "darwin-arm64").unwrap();
        verify_installed(data, &info, "darwin-arm64").unwrap();
        assert_eq!(
            fs::read(data.join("browser-vm/rootfs.ext4")).unwrap(),
            b"next-image"
        );
        assert!(verify_installed(data, &old_info, "darwin-arm64").is_err());
        fs::write(data.join("bin/initrd"), b"broken").unwrap();
        assert!(verify_installed(data, &info, "darwin-arm64").is_err());
        install_archive(data, &next, &info, "darwin-arm64").unwrap();
        verify_installed(data, &info, "darwin-arm64").unwrap();
        assert_eq!(
            fs::read_dir(data.join("browser-vm")).unwrap().count(),
            4,
            "only image-set, install lock and two aliases remain"
        );
    }

    #[test]
    fn browser_image_corrupt_missing_and_mixed_files_preserve_installed_set() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        let (old, old_info) = archive(&fixture(b"working-image"));
        install_archive(data, &old, &old_info, "darwin-arm64").unwrap();
        for index in 0..FILES.len() {
            for missing in [false, true] {
                let mut files = fixture(b"replacement");
                if missing {
                    files.remove(index);
                } else {
                    files[index].1 = b"corrupt".to_vec();
                }
                let (bad, info) = archive(&files);
                assert!(
                    install_archive(data, &bad, &info, "darwin-arm64").is_err(),
                    "index {index}, missing {missing}"
                );
                verify_installed(data, &old_info, "darwin-arm64").unwrap();
            }
        }
        assert_eq!(
            fs::read_dir(data.join("browser-vm")).unwrap().count(),
            4,
            "failed staging was removed"
        );
    }

    #[test]
    fn browser_image_interrupted_download_and_invalid_archive_preserve_working_set() {
        let temp = tempfile::tempdir().unwrap();
        let (old, info) = archive(&fixture(b"working-image"));
        install_archive(temp.path(), &old, &info, "darwin-arm64").unwrap();
        assert!(
            install_archive(temp.path(), &old[..old.len() / 2], &info, "darwin-arm64").is_err()
        );
        let mut wrong_hash = old.clone();
        wrong_hash[0] ^= 1;
        assert!(install_archive(temp.path(), &wrong_hash, &info, "darwin-arm64").is_err());
        let mut malformed = info.clone();
        malformed.size = Some(3);
        malformed.checksum = Some(format!("sha256:{}", sha(b"bad")));
        assert!(install_archive(temp.path(), b"bad", &malformed, "darwin-arm64").is_err());
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
    }

    #[test]
    fn browser_image_architecture_and_contract_fail_before_installation() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("fresh");
        let (bytes, info) = archive(&fixture(b"image"));
        assert!(install_archive(&data, &bytes, &info, "darwin-amd64").is_err());
        assert!(!data.exists());
        assert!(install_archive(&data, &bytes, &info, "linux-amd64").is_err());
        assert!(!data.join(INSTALL_PATH).exists());
        let mut files = fixture(b"image");
        let mut receipt: Value = serde_json::from_slice(&files[0].1).unwrap();
        receipt["preflight"]["manifest"]["direct_network"] = json!(true);
        files[0].1 = serde_json::to_vec(&receipt).unwrap();
        let (bytes, info) = archive(&files);
        assert!(install_archive(&data, &bytes, &info, "darwin-arm64").is_err());
        assert!(!data.join("bin").exists());
    }

    #[test]
    fn browser_image_archive_rejects_duplicates_and_unexpected_members() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["vmlinux", "extra-file"] {
            let mut files = fixture(b"image");
            files.push((name.into(), b"unexpected".to_vec()));
            let (bytes, info) = archive(&files);
            assert!(install_archive(temp.path(), &bytes, &info, "darwin-arm64").is_err());
            assert!(!temp.path().join(INSTALL_PATH).exists());
        }
    }

    #[test]
    fn browser_image_foreign_paths_and_low_disk_space_are_actionable() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("bin")).unwrap();
        fs::write(temp.path().join("bin/vmlinux"), b"operator-owned").unwrap();
        let (bytes, info) = archive(&fixture(b"image"));
        assert!(install_archive(temp.path(), &bytes, &info, "darwin-arm64")
            .unwrap_err()
            .to_string()
            .contains("belongs to another installation"));
        assert_eq!(
            fs::read(temp.path().join("bin/vmlinux")).unwrap(),
            b"operator-owned"
        );
        assert!(!temp.path().join("browser-vm").exists());
        assert!(disk_budget(100, 199, 1000).is_err());
        assert!(disk_budget(100, 200, 1000).is_ok());
    }

    #[test]
    fn browser_image_failed_atomic_replacement_keeps_old_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("installed");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("good"), b"preserved").unwrap();
        assert!(replace_directory(&temp.path().join("absent"), &dest).is_err());
        assert_eq!(fs::read(dest.join("good")).unwrap(), b"preserved");
    }

    #[test]
    fn browser_image_retry_cleans_abandoned_stage_and_preserves_runtime_state() {
        let temp = tempfile::tempdir().unwrap();
        let abandoned = temp
            .path()
            .join("browser-vm")
            .join(format!("{STAGING_PREFIX}interrupted"));
        fs::create_dir_all(abandoned.join(NAME)).unwrap();
        fs::write(abandoned.join(NAME).join("rootfs.ext4"), b"partial").unwrap();
        fs::write(temp.path().join("browser-vm/profile-state"), b"user-state").unwrap();
        let (bytes, info) = archive(&fixture(b"complete-image"));
        install_archive(temp.path(), &bytes, &info, "darwin-arm64").unwrap();
        assert!(!abandoned.exists());
        assert_eq!(
            fs::read(temp.path().join("browser-vm/profile-state")).unwrap(),
            b"user-state"
        );
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
    }

    #[test]
    fn browser_image_requires_exact_release_metadata_before_fetch() {
        let (_, info) = archive(&fixture(b"image"));
        let mut manifest = manifest(&info);
        component_info(&manifest, "darwin-arm64").unwrap();
        assert!(component_info(&manifest, "linux-arm64").is_err());
        manifest
            .external
            .get_mut(NAME)
            .unwrap()
            .platforms
            .insert("*".into(), info.clone());
        assert!(component_info(&manifest, "linux-arm64").is_err());
        manifest
            .external
            .get_mut(NAME)
            .unwrap()
            .platforms
            .get_mut("darwin-arm64")
            .unwrap()
            .checksum = None;
        assert!(component_info(&manifest, "darwin-arm64").is_err());
        manifest.external.clear();
        assert!(component_info(&manifest, "darwin-arm64")
            .err()
            .unwrap()
            .to_string()
            .contains("release package is unavailable"));
    }

    #[test]
    fn browser_image_invalid_published_bundle_metadata_preserves_existing_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let (bytes, info) = archive(&fixture(b"working-image"));
        install_archive(temp.path(), &bytes, &info, "darwin-arm64").unwrap();
        let base = serde_json::to_value(&info).unwrap();
        for (key, value) in [
            ("size", json!(0)),
            ("size", json!(MAX_ARCHIVE_SIZE + 1)),
            ("checksum", json!("sha256:placeholder")),
            ("checksum", Value::Null),
            ("release_path", json!("../image.tar.gz")),
            ("release_path", Value::Null),
            ("extract_path", json!("../image")),
            ("install_path", json!("browser-vm")),
            ("strategy", json!("local-copy")),
            ("binary_path", json!("vmlinux")),
        ] {
            let mut invalid = base.clone();
            invalid[key] = value;
            let invalid: PlatformInfo = serde_json::from_value(invalid).unwrap();
            assert!(
                install_archive(temp.path(), &bytes, &invalid, "darwin-arm64").is_err(),
                "{key}"
            );
            verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
        }
        let mut invalid = info.clone();
        invalid.checksum = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(install_archive(temp.path(), &bytes, &invalid, "darwin-arm64").is_err());
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
    }

    #[tokio::test]
    async fn browser_image_existing_verified_manual_paths_survive_missing_release_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let data = temp.path().join("legacy");
        fs::create_dir_all(data.join("browser-vm")).unwrap();
        fs::create_dir_all(data.join("bin")).unwrap();
        fs::create_dir(&source).unwrap();
        let platform = super::super::detect_platform();
        let target = match guest_platform(&platform) {
            Ok(target) => target,
            Err(_) => return,
        };
        let mut files = fixture(b"existing-image");
        let mut receipt: Value = serde_json::from_slice(&files[0].1).unwrap();
        receipt["target_platform"] = json!(target);
        files[0].1 = serde_json::to_vec(&receipt).unwrap();
        for (name, bytes) in &files {
            fs::write(source.join(name), bytes).unwrap();
        }
        for (alias, file) in aliases(&platform) {
            #[cfg(unix)]
            std::os::unix::fs::symlink(source.join(file), data.join(alias)).unwrap();
        }
        // An absent components file and a source manifest without a released image
        // both preserve a complete existing installation without a fetch path.
        super::super::ensure_browser_vm_image_for_local_engine(&data)
            .await
            .unwrap();
        fs::write(
            data.join("components.json"),
            br#"{"external":{},"profiles":{}}"#,
        )
        .unwrap();
        super::super::ensure_browser_vm_image_for_local_engine(&data)
            .await
            .unwrap();
        for (name, bytes) in &files {
            assert_eq!(fs::read(source.join(name)).unwrap(), *bytes);
        }
        assert!(!data.join(INSTALL_PATH).exists());
        fs::write(source.join("initrd"), b"mixed-initrd").unwrap();
        assert!(
            super::super::ensure_browser_vm_image_for_local_engine(&data)
                .await
                .unwrap_err()
                .to_string()
                .contains("metadata is unavailable")
        );
        let fresh = temp.path().join("fresh");
        let err = super::super::ensure_browser_vm_image_for_local_engine(&fresh)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("browser-vm-image metadata"));
        assert!(!fresh.exists());
    }

    #[tokio::test]
    async fn browser_image_missing_trusted_source_keeps_working_set() {
        let temp = tempfile::tempdir().unwrap();
        let (old, info) = archive(&fixture(b"working-image"));
        let platform = super::super::detect_platform();
        if platform != "darwin-arm64" && platform != "linux-arm64" {
            return;
        }
        install_archive(temp.path(), &old, &info, &platform).unwrap();
        let (_, new_info) = archive(&fixture(b"new-image"));
        assert!(super::super::install_first_party_component_via_carrier(
            temp.path(),
            NAME,
            &new_info,
            &temp.path().join(INSTALL_PATH)
        )
        .await
        .is_err());
        verify_installed(temp.path(), &info, &platform).unwrap();
    }

    #[test]
    fn browser_image_packaging_output_installs_with_existing_component_contract() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        for (name, bytes) in fixture(b"fixture-image-never-launched") {
            fs::write(source.join(name), bytes).unwrap();
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let archive = temp.path().join("image.tar.gz");
        let metadata = temp.path().join("components.json");
        let run = || {
            std::process::Command::new("python3")
                .arg(repo.join("scripts/package-browser-vm-image.py"))
                .arg("--image-dir")
                .arg(&source)
                .args([
                    "--platform",
                    "darwin-arm64",
                    "--release-path",
                    "test/browser-image.tar.gz",
                ])
                .arg("--archive")
                .arg(&archive)
                .arg("--manifest-output")
                .arg(&metadata)
                .env("ELASTOS_DEBUGFS_BIN", temp.path().join("absent-debugfs"))
                .output()
                .unwrap()
        };
        let output = run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let first_archive = fs::read(&archive).unwrap();
        let first_metadata = fs::read(&metadata).unwrap();
        let output = run();
        assert!(output.status.success());
        assert_eq!(
            fs::read(&archive).unwrap(),
            first_archive,
            "archive must reproduce byte-for-byte"
        );
        assert_eq!(fs::read(&metadata).unwrap(), first_metadata);
        let manifest: ComponentsManifest = serde_json::from_slice(&first_metadata).unwrap();
        let info = component_info(&manifest, "darwin-arm64").unwrap();
        let installed = temp.path().join("installed");
        install_archive(&installed, &first_archive, info, "darwin-arm64").unwrap();
        verify_installed(&installed, info, "darwin-arm64").unwrap();
        fs::write(source.join("vmlinux"), b"wrong-kernel").unwrap();
        assert!(!run().status.success());
        assert_eq!(fs::read(&archive).unwrap(), first_archive);
        assert_eq!(fs::read(&metadata).unwrap(), first_metadata);
    }

    fn cache_fixture(path: &Path) -> [PathBuf; 4] {
        fs::create_dir_all(path).unwrap();
        for (name, bytes) in fixture(b"working-image") {
            fs::write(path.join(name), bytes).unwrap();
        }
        FILES.map(|name| path.join(name))
    }

    #[test]
    fn browser_image_concurrent_opens_share_one_verification() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, TryLockError,
        };
        let temp = tempfile::tempdir().unwrap();
        let paths = cache_fixture(temp.path());
        let cache = Mutex::new(VecDeque::new());
        let count = AtomicUsize::new(0);
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        std::thread::scope(|scope| {
            let (cache_ref, paths_ref, count_ref) = (&cache, &paths, &count);
            let first = scope.spawn(move || {
                verify_cached_payload_in(cache_ref, paths_ref, "darwin-arm64", "legacy", || {
                    count_ref.fetch_add(1, Ordering::SeqCst);
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    verify_payload_paths(paths_ref, "darwin-arm64")
                })
            });
            started.recv().unwrap();
            let second = scope.spawn(|| {
                verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
                    count.fetch_add(1, Ordering::SeqCst);
                    verify_payload_paths(&paths, "darwin-arm64")
                })
            });
            let serialized = matches!(cache.try_lock(), Err(TryLockError::WouldBlock));
            release.send(()).unwrap();
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
            assert!(
                serialized,
                "a concurrent cold miss must share the in-flight verification"
            );
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
        verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
            panic!("the completed verification must remain cached")
        })
        .unwrap();
    }

    #[test]
    fn browser_image_cache_reuses_unchanged_set_and_invalidates_release_receipt_and_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let paths = cache_fixture(temp.path());
        let cache = Mutex::new(VecDeque::new());
        let count = std::cell::Cell::new(0);
        let check = |release| {
            verify_cached_payload_in(&cache, &paths, "darwin-arm64", release, || {
                count.set(count.get() + 1);
                verify_payload_paths(&paths, "darwin-arm64")
            })
        };
        check("release-one").unwrap();
        check("release-one").unwrap();
        assert_eq!(
            count.get(),
            1,
            "unchanged set must reuse its verified result"
        );
        check("release-two").unwrap();
        assert_eq!(
            count.get(),
            2,
            "selected release changes require a fresh verification"
        );
        let replacement = temp.path().join("replacement");
        fs::write(&replacement, fs::read(&paths[1]).unwrap()).unwrap();
        fs::rename(&replacement, &paths[1]).unwrap();
        check("release-two").unwrap();
        assert_eq!(
            count.get(),
            3,
            "same-byte replacement must invalidate the cache"
        );
        let old_receipt = fs::read(&paths[0]).unwrap();
        let mut receipt: Value = serde_json::from_slice(&old_receipt).unwrap();
        receipt["target_platform"] = json!("linux-amd64");
        fs::write(&paths[0], serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(check("release-two").is_err());
        assert!(cache.lock().unwrap().is_empty());
        fs::write(&paths[0], old_receipt).unwrap();
        check("release-two").unwrap();
        assert_eq!(count.get(), 5);
    }

    #[test]
    fn browser_image_cache_rejects_same_size_mutation_with_restored_mtime_and_keeps_failures_uncached(
    ) {
        let temp = tempfile::tempdir().unwrap();
        let paths = cache_fixture(temp.path());
        let cache = Mutex::new(VecDeque::new());
        let count = std::cell::Cell::new(0);
        let check = || {
            verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
                count.set(count.get() + 1);
                verify_payload_paths(&paths, "darwin-arm64")
            })
        };
        check().unwrap();
        let before = fs::metadata(&paths[1]).unwrap();
        fs::write(&paths[1], b"changed-image").unwrap();
        fs::File::open(&paths[1])
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        assert_eq!(before.len(), fs::metadata(&paths[1]).unwrap().len());
        assert_eq!(
            before.modified().unwrap(),
            fs::metadata(&paths[1]).unwrap().modified().unwrap()
        );
        assert!(
            check().is_err(),
            "same-size mutation must fail before admission"
        );
        assert!(check().is_err(), "a failed verification must run again");
        assert_eq!(count.get(), 3);
        assert!(cache.lock().unwrap().is_empty());
    }

    #[test]
    fn browser_image_cache_rejects_mutation_between_verification_and_admission() {
        let temp = tempfile::tempdir().unwrap();
        let paths = cache_fixture(temp.path());
        let cache = Mutex::new(VecDeque::new());
        let result = verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
            verify_payload_paths(&paths, "darwin-arm64")?;
            fs::write(&paths[1], b"changed-image")?;
            Ok(())
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("changed during verification"));
        assert!(cache.lock().unwrap().is_empty());
    }

    #[test]
    fn browser_image_successful_verification_cache_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let cache = Mutex::new(VecDeque::new());
        let mut first = None;
        for index in 0..VERIFIED_SET_CACHE_LIMIT + 1 {
            let paths = cache_fixture(&temp.path().join(index.to_string()));
            verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
                verify_payload_paths(&paths, "darwin-arm64")
            })
            .unwrap();
            if first.is_none() {
                first = Some(paths);
            }
        }
        assert_eq!(cache.lock().unwrap().len(), VERIFIED_SET_CACHE_LIMIT);
        let paths = first.unwrap();
        let called = std::cell::Cell::new(false);
        verify_cached_payload_in(&cache, &paths, "darwin-arm64", "legacy", || {
            called.set(true);
            verify_payload_paths(&paths, "darwin-arm64")
        })
        .unwrap();
        assert!(
            called.get(),
            "the oldest verified set should have been evicted"
        );
    }
}
