//! Receipt-bound image acquisition through the existing first-party installer.
//! This prepares image bytes; Engine host admission still owns launch readiness.

use super::{replace_directory, ComponentsManifest, PlatformInfo};
use anyhow::{bail, ensure, Context};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub(super) const NAME: &str = "browser-vm-image";
pub(super) const INSTALL_PATH: &str = "browser-vm/image-set";
const RECEIPT: &str = "browser-vm-rootfs-manifest.json";
const RECEIPT_HASH: &str = ".elastos-image-manifest-sha256";
const FILES: [&str; 4] = [RECEIPT, "rootfs.ext4", "vmlinux", "initrd"];
// An exact stamped size bounds each disk-backed transfer independently of RAM.
const MAX_ARCHIVE_SIZE: u64 = 64 * 1024 * 1024 * 1024;
const IO_CHUNK_SIZE: usize = 64 * 1024;
const STAGING_PREFIX: &str = ".browser-image-stage-";
const VERIFIED_SET_CACHE_LIMIT: usize = 8;
// The unpacked set: the 4096 MiB root disk that
// scripts/build/build-browser-vm-rootfs.sh builds, plus kernel, initrd and
// receipts (about 120 MB). A test keeps it above the builder's root disk size.
const UNPACKED_SET_BYTES: u64 = 4352 * 1024 * 1024;
// Unit tests unpack small fixtures, so they do not ask for the full set's room.
const UNPACK_ESTIMATE: u64 = if cfg!(test) { 0 } else { UNPACKED_SET_BYTES };

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
    ensure!(
        info.size.is_some_and(|size| size <= MAX_ARCHIVE_SIZE),
        "Browser image archive exceeds the 64 GiB package bound"
    );
    Ok(())
}

fn aliases(platform: &str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("browser-vm/rootfs.ext4", "rootfs.ext4"),
        ("browser-vm/browser-vm-rootfs-manifest.json", RECEIPT),
        (
            if platform == "darwin-arm64" {
                "bin/vmlinux"
            } else {
                "browser-vm/vmlinux"
            },
            "vmlinux",
        ),
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
    verify_payload_with_cancel(bundle, platform, None)
}

fn verify_payload_with_cancel(
    bundle: &Path,
    platform: &str,
    cancelled: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    for name in FILES {
        ensure!(
            fs::symlink_metadata(bundle.join(name))?
                .file_type()
                .is_file(),
            "Browser image package {name} must be a regular file"
        );
    }
    verify_payload_paths_with_cancel(&FILES.map(|name| bundle.join(name)), platform, cancelled)
}

pub(super) fn verify_legacy_or_missing_release(data: &Path, platform: &str) -> anyhow::Result<()> {
    guest_platform(platform)?;
    // Existing source-home installations own these paths. Verify their bytes and
    // receipt in place; setup acquisition neither copies nor rewrites them.
    let image_aliases = aliases(platform);
    let mut kernel = data.join(image_aliases[2].0);
    if platform.starts_with("linux-")
        && matches!(fs::symlink_metadata(&kernel), Err(err) if err.kind() == std::io::ErrorKind::NotFound)
    {
        // Source Home staged the guest kernel here before release image aliases.
        // The receipt must still bind its bytes; an existing Browser alias wins,
        // including a broken or corrupt alias that requires repair.
        kernel = data.join("bin/vmlinux");
    }
    let paths = [
        data.join(image_aliases[1].0),
        data.join(image_aliases[0].0),
        kernel,
        data.join(image_aliases[3].0),
    ];
    verify_cached_payload(&paths, platform, "legacy", || verify_payload_paths(&paths, platform))
        .map_err(|err| anyhow::anyhow!("Browser image release package metadata is unavailable and the installed image set requires preparation ({err}); install a release with browser-vm-image metadata or select an approved remote Engine"))
}

fn verify_payload_paths(paths: &[PathBuf; 4], platform: &str) -> anyhow::Result<()> {
    verify_payload_paths_with_cancel(paths, platform, None)
}

fn verify_payload_paths_with_cancel(
    paths: &[PathBuf; 4],
    platform: &str,
    cancelled: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    let target = guest_platform(platform)?;
    let receipt_path = &paths[0];
    ensure!(
        fs::metadata(receipt_path)?.file_type().is_file()
            && fs::metadata(receipt_path)?.len() <= 1024 * 1024,
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
    let mut verified = Vec::with_capacity(3);
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
        // The identity is taken before the bytes are read, so a write during
        // hashing cannot bind its new identity to the old digest.
        let identity = VerifiedArtifact::observe(path);
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && (identity
                    .as_ref()
                    .is_some_and(|artifact| artifact.recorded_digest().as_deref() == Some(hash))
                    || hash_reader(&mut fs::File::open(path)?, cancelled)? == hash),
            "Browser image {name} checksum mismatch"
        );
        verified.push((path, hash, identity));
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
    record_verified_identities(&verified);
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

/// Image artifacts whose bytes matched the receipt, by file identity, kept in
/// the image set. A later verifier (the Runtime at a Browser open, the
/// Engine's host readiness) then trusts them without hashing the 4 GiB root
/// disk again: 45-80 s on an SD card (#90). Any write or replacement changes
/// the inode or the change time, which an unprivileged process cannot set back.
const VERIFIED_IDENTITY: &str = ".elastos-verified-identity.json";
const VERIFIED_IDENTITY_SCHEMA: &str = "elastos.browser.vm-image-verified-identity/v1";
const VERIFIED_IDENTITY_MAX_BYTES: u64 = 64 * 1024;

/// One artifact's record location and identity, observed before its bytes
/// are read.
#[derive(PartialEq, Eq)]
struct VerifiedArtifact {
    record: PathBuf,
    name: String,
    identity: Value,
}

impl VerifiedArtifact {
    #[cfg(unix)]
    fn observe(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let real = fs::canonicalize(path).ok()?;
        let meta = fs::metadata(&real).ok()?;
        let nanos = |secs: i64, nsec: i64| secs.checked_mul(1_000_000_000)?.checked_add(nsec);
        Some(Self {
            record: real.parent()?.join(VERIFIED_IDENTITY),
            name: real.file_name()?.to_str()?.to_owned(),
            identity: json!({
                "dev": meta.dev(),
                "ino": meta.ino(),
                "size": meta.len(),
                "mtime_ns": nanos(meta.mtime(), meta.mtime_nsec())?,
                "ctime_ns": nanos(meta.ctime(), meta.ctime_nsec())?,
            }),
        })
    }

    #[cfg(not(unix))]
    fn observe(_path: &Path) -> Option<Self> {
        None
    }

    fn recorded_digest(&self) -> Option<String> {
        let entry = &read_verified_record(&self.record)?["files"][&self.name];
        (entry["identity"] == self.identity)
            .then(|| entry["sha256"].as_str().map(str::to_owned))
            .flatten()
    }
}

fn read_verified_record(record: &Path) -> Option<Value> {
    let meta = fs::symlink_metadata(record).ok()?;
    if !meta.file_type().is_file() || meta.len() > VERIFIED_IDENTITY_MAX_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(record).ok()?).ok()?;
    (value["schema"] == VERIFIED_IDENTITY_SCHEMA).then_some(value)
}

/// Records the verified artifacts when they share one image-set directory and
/// none changed since its identity was observed. Best effort: without a
/// record the next verifier hashes again.
fn record_verified_identities(verified: &[(&PathBuf, &str, Option<VerifiedArtifact>)]) {
    let mut record = None;
    let mut files = serde_json::Map::new();
    for (path, sha256, observed) in verified {
        let Some(observed) = observed else { return };
        if VerifiedArtifact::observe(path).as_ref() != Some(observed)
            || record.get_or_insert_with(|| observed.record.clone()) != &observed.record
        {
            return;
        }
        files.insert(
            observed.name.clone(),
            json!({"identity": observed.identity, "sha256": sha256}),
        );
    }
    let Some(record) = record else { return };
    let value = json!({"schema": VERIFIED_IDENTITY_SCHEMA, "files": files});
    if read_verified_record(&record).as_ref() == Some(&value) {
        return;
    }
    let partial = record.with_file_name(format!("{VERIFIED_IDENTITY}.{}", std::process::id()));
    let _ = fs::remove_file(&partial);
    let written = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&value)?)?;
        file.sync_all()?;
        fs::rename(&partial, &record)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
}

fn check_cancelled(cancelled: Option<&AtomicBool>) -> anyhow::Result<()> {
    ensure!(
        !cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)),
        "Browser image preparation cancelled (previous set preserved)"
    );
    Ok(())
}

fn hash_reader(reader: &mut impl Read, cancelled: Option<&AtomicBool>) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    let mut chunk = [0u8; IO_CHUNK_SIZE];
    loop {
        check_cancelled(cancelled)?;
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        hash.update(&chunk[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

// Dropping the async preparation cancels its blocking reader before commit.
// The anonymous download file and TempDir stages clean up when their owners exit.
#[derive(Default)]
struct PreparationCancellation(Arc<AtomicBool>);

impl Drop for PreparationCancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct CancellableReader<'a, R> {
    reader: R,
    cancelled: &'a AtomicBool,
}

impl<R: Read> Read for CancellableReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Browser image preparation cancelled"));
        }
        self.reader.read(buffer)
    }
}

pub(super) async fn install_via_carrier(
    data: &Path,
    row: &crate::capsule_record::AdmittedRow,
    dest: &Path,
    platform: &str,
    context: super::FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    let info = row.info();
    validate_request(data, info, dest, platform)?;
    let bind_addr = super::first_party_carrier_bind_addr(data, context)?;
    let source = crate::sources::load_trusted_sources(data)?
        .default_source()
        .cloned()
        .ok_or_else(super::missing_trusted_source_error)?;
    let parent = dest.parent().unwrap();
    require_space(dest, info)?;
    let size = info.size.unwrap();
    let cancellation = PreparationCancellation::default();
    // tempfile creates an unlinked/private file on supported Unix Engine hosts.
    // Process exit or cancellation cannot leave a partial archive pathname.
    let temporary = tempfile::tempfile_in(parent)?;
    let mut download = tokio::fs::File::from_std(temporary);
    let mut checkpoint = 0;
    let mut displayed = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut progress = |received: u64, total: u64| -> anyhow::Result<()> {
        check_cancelled(Some(&cancellation.0))?;
        if received == 0
            || received == total
            || displayed.elapsed() >= std::time::Duration::from_secs(1)
        {
            eprint!(
                "\rBrowser image: {} / {} MiB ({}%)",
                received / (1024 * 1024),
                total / (1024 * 1024),
                received.saturating_mul(100) / total.max(1)
            );
            if received == total {
                eprintln!();
                eprintln!("Checking and unpacking the Browser image...");
            }
            displayed = std::time::Instant::now();
        }
        if received == 0
            || received == total
            || received.saturating_sub(checkpoint) >= 16 * 1024 * 1024
        {
            // The rest of the download and the unpacked set still need room.
            check_disk_space(
                parent,
                total
                    .saturating_sub(received)
                    .saturating_add(UNPACK_ESTIMATE),
            )?;
            checkpoint = received;
            // Debug: an INFO line here would cut through the progress line above.
            tracing::debug!(
                component = NAME,
                phase = "downloading",
                bytes_received = received,
                total_bytes = total,
                "Browser image preparation"
            );
        }
        Ok(())
    };
    // The download fails only when no bytes arrive for 30 s (each Carrier
    // read/write has that idle limit); progress keeps it alive.
    crate::carrier::fetch_file_from_trusted_source_to_bound(
        &source,
        info.release_path.as_deref().unwrap(),
        &mut download,
        size,
        &mut progress,
        bind_addr,
    )
    .await
    .context("Browser image download failed (previous set preserved)")?;
    tokio::time::timeout(std::time::Duration::from_secs(30), download.sync_all())
        .await
        .context("Browser image download sync deadline")??;
    let file = download.into_std().await;
    let data = data.to_path_buf();
    let info = info.clone();
    let platform = platform.to_owned();
    let cancelled = cancellation.0.clone();
    tokio::task::spawn_blocking(move || {
        install_archive_reader(&data, file, &info, &platform, &cancelled)
    })
    .await?
}

#[cfg(test)]
fn install_archive(
    data: &Path,
    bytes: &[u8],
    info: &PlatformInfo,
    platform: &str,
) -> anyhow::Result<()> {
    install_archive_reader(
        data,
        std::io::Cursor::new(bytes),
        info,
        platform,
        &AtomicBool::new(false),
    )
}

fn install_archive_reader(
    data: &Path,
    mut reader: impl Read + Seek,
    info: &PlatformInfo,
    platform: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    let dest = data.join(INSTALL_PATH);
    validate_request(data, info, &dest, platform)?;
    check_cancelled(Some(cancelled))?;
    ensure!(
        info.size == Some(reader.seek(SeekFrom::End(0))?),
        "Browser image archive is incomplete: size mismatch"
    );
    reader.rewind()?;
    tracing::debug!(
        component = NAME,
        phase = "verifying",
        "Browser image preparation"
    );
    ensure!(
        info.checksum.as_deref()
            == Some(&format!(
                "sha256:{}",
                hash_reader(&mut reader, Some(cancelled))?
            )),
        "Browser image archive checksum mismatch (previous set preserved)"
    );
    reader.rewind()?;
    let parent = dest.parent().unwrap();
    fs::create_dir_all(parent)?;
    let _install_lock = lock_installation(parent)?;
    validate_request(data, info, &dest, platform)?;
    remove_abandoned_stages(parent)?;
    // Same-volume staging allows a single rename/exchange at the commit point.
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)?;
    let bundle = staging.path().join(NAME);
    fs::create_dir(&bundle)?;
    let reader = CancellableReader { reader, cancelled };
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    tracing::debug!(
        component = NAME,
        phase = "installing",
        "Browser image preparation"
    );
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
    verify_payload_with_cancel(&bundle, platform, Some(cancelled))?;
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
        check_cancelled(Some(cancelled))?;
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
        check_cancelled(Some(cancelled))?;
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

fn lock_installation(parent: &Path) -> anyhow::Result<crate::host_lock::FileLock> {
    #[cfg(unix)]
    {
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
        crate::host_lock::FileLock::exclusive(file).context("Browser image install lock failed")
    }
    #[cfg(not(unix))]
    bail!("Browser image installation requires a supported Unix host")
}

/// Before the first byte is fetched, the download and the unpacked set must
/// both fit with the shared reserve kept free. Otherwise one sentence names the
/// space needed, instead of a refusal after the whole download.
pub(super) fn require_space(dest: &Path, info: &PlatformInfo) -> anyhow::Result<()> {
    require_space_measured(dest, info, UNPACK_ESTIMATE, available_space)
}

fn require_space_measured(
    dest: &Path,
    info: &PlatformInfo,
    unpacked: u64,
    available: impl FnOnce(&Path) -> anyhow::Result<u64>,
) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .context("Browser image install path has no parent")?;
    fs::create_dir_all(parent)?;
    // An unpack stopped by Ctrl+C or a crash leaves its stage behind; it must
    // not count against the space a retry needs.
    {
        let _install_lock = lock_installation(parent)?;
        remove_abandoned_stages(parent)?;
    }
    image_space(available(parent)?, info.size.unwrap_or_default(), unpacked)
}

/// A process interruption releases the install lock. With that lock held, the
/// next install discards only this installer's abandoned stage directories.
fn remove_abandoned_stages(parent: &Path) -> anyhow::Result<()> {
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
    Ok(())
}

fn image_space(available: u64, archive: u64, unpacked: u64) -> anyhow::Result<()> {
    let needed = archive.saturating_add(unpacked);
    if elastos_common::require_free_space(u128::from(available), u128::from(needed)).is_err() {
        bail!(
            "the Browser Engine image needs {} of free disk space to download and unpack, and {} is free",
            super::progress::format_size(
                needed.saturating_add(elastos_common::FREE_SPACE_RESERVE_BYTES)
            ),
            super::progress::format_size(available)
        );
    }
    Ok(())
}

/// Cheap check for setup's download summary: the installed set's receipts name
/// this release row. `verify_installed` still hashes the files before use.
pub(super) fn receipts_match(data: &Path, info: &PlatformInfo) -> bool {
    let dest = data.join(INSTALL_PATH);
    super::extracted_bundle_cache_stale_reason(&dest, info).is_none()
        && fs::read_to_string(dest.join(RECEIPT_HASH))
            .ok()
            .zip(super::compute_sha256_checksum(&dest.join(RECEIPT)).ok())
            .is_some_and(|(recorded, current)| recorded.trim() == current)
}

fn check_disk_space(path: &Path, needed: u64) -> anyhow::Result<()> {
    Ok(elastos_common::require_free_space(
        u128::from(available_space(path)?),
        u128::from(needed),
    )?)
}

fn available_space(path: &Path) -> anyhow::Result<u64> {
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
        fn disk_block_count(count: impl Into<u64>) -> u64 {
            count.into()
        }
        #[cfg(target_pointer_width = "64")]
        let block_size = stats.f_frsize;
        #[cfg(target_pointer_width = "32")]
        let block_size = u64::from(stats.f_frsize);
        Ok(disk_block_count(stats.f_bavail).saturating_mul(block_size))
    }
    #[cfg(not(unix))]
    bail!("Browser image free-space check requires a supported Unix host")
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
            "inputs_sha256":"c".repeat(64),
            "recipe_options":{"target_platform":"linux-arm64", "rootfs_size":"8192M",
                "debian_suite":"bookworm", "debian_mirror":"https://deb.debian.org/debian",
                "cdp_timeout_ms":"20000"},
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
    fn browser_image_verified_identity_record_skips_the_hash_until_a_write() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        let (bytes, info) = archive(&fixture(b"recorded-image"));
        install_archive(data, &bytes, &info, "darwin-arm64").unwrap();
        let dest = data.join(INSTALL_PATH);
        let mut record: Value =
            serde_json::from_slice(&fs::read(dest.join(VERIFIED_IDENTITY)).unwrap()).unwrap();
        assert_eq!(record["files"].as_object().unwrap().len(), 3);
        assert_eq!(
            record["files"]["rootfs.ext4"]["sha256"],
            sha(b"recorded-image")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.join(VERIFIED_IDENTITY))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A digest recorded for this exact identity admits the file without
        // reading it: the receipt now names a digest the bytes do not have.
        let paths = FILES.map(|name| dest.join(name));
        let claimed = "0".repeat(64);
        let mut receipt: Value = serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
        receipt["sha256"] = json!(claimed);
        fs::write(&paths[0], serde_json::to_vec(&receipt).unwrap()).unwrap();
        record["files"]["rootfs.ext4"]["sha256"] = json!(claimed);
        fs::write(
            dest.join(VERIFIED_IDENTITY),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        verify_payload_paths(&paths, "darwin-arm64").unwrap();

        // Any write changes the identity, so the bytes are hashed and refused.
        fs::write(&paths[1], b"recorded-image").unwrap();
        assert!(verify_payload_paths(&paths, "darwin-arm64").is_err());
    }

    #[test]
    fn browser_image_disk_reader_verifies_before_replacing_and_cleans_cancelled_staging() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let (bytes, info) = archive(&fixture(b"working-image"));
        let mut file = tempfile::tempfile_in(temp.path()).unwrap();
        file.write_all(&bytes).unwrap();
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            0,
            "download file has no orphan pathname"
        );
        install_archive_reader(
            temp.path(),
            &mut file,
            &info,
            "darwin-arm64",
            &AtomicBool::new(false),
        )
        .unwrap();
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
        let (replacement, next_info) = archive(&fixture(b"replacement"));
        struct CancelAfterVerification<'a> {
            reader: std::io::Cursor<&'a [u8]>,
            cancelled: &'a AtomicBool,
            rewinds: usize,
        }
        impl Read for CancelAfterVerification<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                assert!(bytes.len() <= IO_CHUNK_SIZE);
                self.reader.read(bytes)
            }
        }
        impl Seek for CancelAfterVerification<'_> {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                if matches!(position, SeekFrom::Start(0)) {
                    self.rewinds += 1;
                    if self.rewinds == 2 {
                        self.cancelled.store(true, Ordering::Release);
                    }
                }
                self.reader.seek(position)
            }
        }
        let cancelled = AtomicBool::new(false);
        let reader = CancelAfterVerification {
            reader: std::io::Cursor::new(&replacement),
            cancelled: &cancelled,
            rewinds: 0,
        };
        let error =
            install_archive_reader(temp.path(), reader, &next_info, "darwin-arm64", &cancelled)
                .unwrap_err();
        assert!(format!("{error:#}").contains("cancelled"));
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
        assert_eq!(
            fs::read_dir(temp.path().join("browser-vm"))
                .unwrap()
                .count(),
            4
        );
        let owner = PreparationCancellation::default();
        let flag = owner.0.clone();
        drop(owner);
        assert!(install_archive_reader(temp.path(), file, &info, "darwin-arm64", &flag).is_err());
        verify_installed(temp.path(), &info, "darwin-arm64").unwrap();
    }

    #[test]
    fn browser_image_streaming_metadata_accepts_large_exact_size_and_retains_bound() {
        let (_, mut info) = archive(&fixture(b"image"));
        info.size = Some(200 * 1024 * 1024 + 1);
        validate_info(&info).unwrap();
        info.size = Some(MAX_ARCHIVE_SIZE);
        validate_info(&info).unwrap();
        info.size = Some(MAX_ARCHIVE_SIZE + 1);
        assert!(validate_info(&info).is_err());
    }

    #[tokio::test]
    async fn browser_image_first_party_stream_installs_verified_file_and_preserves_set_on_bad_hash()
    {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let platform = super::super::detect_platform();
        let Ok(guest) = guest_platform(&platform) else {
            return;
        };
        let target = tempfile::tempdir().unwrap();
        let mut files = fixture(b"working-image");
        let mut receipt: Value = serde_json::from_slice(&files[0].1).unwrap();
        receipt["target_platform"] = json!(guest);
        files[0].1 = serde_json::to_vec(&receipt).unwrap();
        let (bytes, info) = archive(&files);
        let publisher = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .alpns(vec![b"elastos/carrier/1".to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let runtime = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let bind_addr = runtime.bound_sockets()[0];
        fs::write(
            target.path().join("config.toml"),
            format!("carrier_bind_addr = \"{bind_addr}\"\n"),
        )
        .unwrap();
        let address = crate::carrier::tests::wait_for_direct_endpoint_addr(&publisher).await;
        let ticket = data_encoding::BASE32_NOPAD
            .encode(&serde_json::to_vec(&json!({"endpoints":[address]})).unwrap())
            .to_ascii_lowercase();
        let sources = serde_json::from_value(json!({
            "schema":"elastos.trusted-sources/v1", "default_source":"test",
            "sources":[{"name":"test","publisher_node_id":publisher.id().to_string(),"connect_ticket":ticket}]
        })).unwrap();
        crate::sources::save_trusted_sources(target.path(), &sources).unwrap();
        let dest = target.path().join(INSTALL_PATH);
        let refused = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            super::super::install_first_party_component_via_carrier(
                target.path(),
                &crate::capsule_record::AdmittedRow::for_test(NAME, &info),
                &dest,
                super::super::FirstPartyCarrierContext::Setup,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(format!("{refused:#}").contains(&bind_addr.to_string()));
        assert!(!dest.exists());
        let server = publisher.clone();
        let release_path = info.release_path.clone().unwrap();
        let serving = tokio::spawn(async move {
            for _ in 0..2 {
                let incoming = server.accept().await.unwrap();
                let iroh::endpoint::IncomingAddr::Ip(client_addr) = incoming.remote_addr() else {
                    panic!("image download must use the configured loopback IP");
                };
                assert_eq!(client_addr.ip(), bind_addr.ip());
                assert_ne!(client_addr.port(), bind_addr.port());
                let connection = incoming.await.unwrap();
                let (mut send, recv) = connection.accept_bi().await.unwrap();
                let mut request = String::new();
                BufReader::new(recv).read_line(&mut request).await.unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&request).unwrap()["path"],
                    release_path
                );
                send.write_all(&(bytes.len() as u64).to_be_bytes())
                    .await
                    .unwrap();
                send.write_all(&bytes).await.unwrap();
                send.finish().unwrap();
                assert!(matches!(
                    connection.closed().await,
                    iroh::endpoint::ConnectionError::ApplicationClosed(_)
                ));
                tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    loop {
                        match std::net::UdpSocket::bind(client_addr) {
                            Ok(socket) => break drop(socket),
                            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                            Err(error) => panic!("image download port cleanup failed: {error}"),
                        }
                    }
                })
                .await
                .expect("image download must close its temporary listener");
            }
        });
        super::super::install_first_party_component_via_carrier(
            target.path(),
            &crate::capsule_record::AdmittedRow::for_test(NAME, &info),
            &dest,
            super::super::FirstPartyCarrierContext::Runtime,
        )
        .await
        .unwrap();
        verify_installed(target.path(), &info, &platform).unwrap();
        assert!(!runtime.is_closed());
        assert!(std::net::UdpSocket::bind(bind_addr).is_err());
        let mut corrupt = info.clone();
        corrupt.checksum = Some(format!("sha256:{}", "0".repeat(64)));
        let error = super::super::install_first_party_component_via_carrier(
            target.path(),
            &crate::capsule_record::AdmittedRow::for_test(NAME, &corrupt),
            &dest,
            super::super::FirstPartyCarrierContext::Runtime,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("checksum mismatch"));
        verify_installed(target.path(), &info, &platform).unwrap();
        assert!(!runtime.is_closed());
        assert_eq!(runtime.bound_sockets(), vec![bind_addr]);
        assert!(std::net::UdpSocket::bind(bind_addr).is_err());
        let mut expected_entries = BTreeSet::from([
            "image-set".to_string(),
            ".browser-image-install.lock".to_string(),
        ]);
        expected_entries.extend(aliases(&platform).into_iter().filter_map(|(alias, _)| {
            Path::new(alias)
                .strip_prefix("browser-vm")
                .ok()
                .map(|name| name.to_string_lossy().into_owned())
        }));
        assert_eq!(
            fs::read_dir(target.path().join("browser-vm"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<BTreeSet<_>>(),
            expected_entries,
            "download and failed stages leave only the installed set, aliases and lock"
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap();
        runtime.close().await;
        publisher.close().await;
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
    fn browser_image_linux_keeps_capsule_kernel_owned_separately() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("bin")).unwrap();
        fs::write(temp.path().join("bin/vmlinux"), b"capsule kernel").unwrap();
        let (bytes, info) = archive(&fixture(b"image"));
        install_archive(temp.path(), &bytes, &info, "linux-arm64").unwrap();
        verify_installed(temp.path(), &info, "linux-arm64").unwrap();
        assert_eq!(
            fs::read(temp.path().join("bin/vmlinux")).unwrap(),
            b"capsule kernel"
        );
        assert_eq!(
            fs::read(temp.path().join("browser-vm/image-set/vmlinux")).unwrap(),
            b"kernel"
        );
    }

    #[test]
    fn browser_image_foreign_paths_are_actionable() {
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
    fn browser_image_ended_install_lock_is_free_while_a_command_spawned_under_it_runs() {
        use crate::host_lock::test_support::{lock_is_free, SpawnedWhileOpen};

        let temp = tempfile::tempdir().unwrap();
        let lock = lock_installation(temp.path()).unwrap();
        let path = temp.path().join(".browser-image-install.lock");
        let _command = SpawnedWhileOpen::new(&path);
        drop(lock);
        assert!(
            lock_is_free(&path),
            "the next install must not wait for a command spawned under the last"
        );
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

    #[test]
    fn browser_image_manual_aliases_verify_the_guest_kernel_on_each_host() {
        for platform in ["darwin-arm64", "linux-arm64", "linux-amd64"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path();
            let source = data.join("source");
            fs::create_dir_all(data.join("browser-vm")).unwrap();
            fs::create_dir_all(data.join("bin")).unwrap();
            fs::create_dir(&source).unwrap();
            let mut files = fixture(b"manual-image");
            let mut receipt: Value = serde_json::from_slice(&files[0].1).unwrap();
            receipt["target_platform"] = json!(guest_platform(platform).unwrap());
            files[0].1 = serde_json::to_vec(&receipt).unwrap();
            for (name, bytes) in &files {
                fs::write(source.join(name), bytes).unwrap();
            }
            for (alias, file) in aliases(platform) {
                #[cfg(unix)]
                std::os::unix::fs::symlink(source.join(file), data.join(alias)).unwrap();
            }
            if platform.starts_with("linux-") {
                // The capsule VM kernel is a separate artifact from the Browser guest.
                fs::write(data.join("bin/vmlinux"), b"capsule-kernel").unwrap();
            }
            verify_legacy_or_missing_release(data, platform).unwrap();
            if platform.starts_with("linux-") {
                fs::remove_file(data.join("browser-vm/vmlinux")).unwrap();
                fs::write(data.join("bin/vmlinux"), b"kernel").unwrap();
                verify_legacy_or_missing_release(data, platform).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(
                    source.join("missing-kernel"),
                    data.join("browser-vm/vmlinux"),
                )
                .unwrap();
                assert!(verify_legacy_or_missing_release(data, platform).is_err());
                fs::remove_file(data.join("browser-vm/vmlinux")).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(source.join("vmlinux"), data.join("browser-vm/vmlinux"))
                    .unwrap();
            }
            fs::write(source.join("vmlinux"), b"mutated").unwrap();
            assert!(verify_legacy_or_missing_release(data, platform).is_err());
        }
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
            &crate::capsule_record::AdmittedRow::for_test(NAME, &new_info),
            &temp.path().join(INSTALL_PATH),
            super::super::FirstPartyCarrierContext::Setup,
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

    #[test]
    fn browser_image_refuses_before_download_unless_download_and_unpack_fit() {
        // The unpacked estimate covers the root disk the release image builder makes.
        let builder = include_str!("../../../../../scripts/build/build-browser-vm-rootfs.sh");
        let root_disk_mib: u64 = builder
            .split("rootfs_size=\"${ELASTOS_BROWSER_VM_ROOTFS_SIZE:-")
            .nth(1)
            .and_then(|rest| rest.split("M}\"").next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(UNPACKED_SET_BYTES >= (root_disk_mib + 128) * 1024 * 1024);
        // The 2.0.0 Mac image: 868 MiB to download, about 4.2 GB unpacked.
        let archive = 868 * 1024 * 1024;
        let fits = archive + UNPACKED_SET_BYTES + elastos_common::FREE_SPACE_RESERVE_BYTES;
        assert!(image_space(fits, archive, UNPACKED_SET_BYTES).is_ok());
        assert!(image_space(fits - 1, archive, UNPACKED_SET_BYTES).is_err());
        // A 3-7 GB disk once downloaded the whole archive and then failed to unpack.
        assert_eq!(
            image_space(5 * 1024 * 1024 * 1024, archive, UNPACKED_SET_BYTES)
                .unwrap_err()
                .to_string(),
            "the Browser Engine image needs 7.1 GB of free disk space to download and unpack, and 5.0 GB is free"
        );
    }

    #[test]
    fn browser_image_receipts_match_only_the_installed_release_row() {
        let temp = tempfile::tempdir().unwrap();
        let (bytes, info) = archive(&fixture(b"rootfs"));
        assert!(!receipts_match(temp.path(), &info));
        install_archive(temp.path(), &bytes, &info, "darwin-arm64").unwrap();
        assert!(receipts_match(temp.path(), &info));
        let mut other = info.clone();
        other.checksum = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(!receipts_match(temp.path(), &other));
    }

    #[test]
    fn browser_image_space_check_discards_an_interrupted_unpack_before_measuring() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join(INSTALL_PATH);
        let parent = dest.parent().unwrap();
        // Ctrl+C during unpack left a 3 GiB stage (sparse here) next to a set
        // and Browser state that stay.
        let abandoned = parent.join(format!("{STAGING_PREFIX}interrupted"));
        fs::create_dir_all(abandoned.join(NAME)).unwrap();
        fs::File::create(abandoned.join(NAME).join("rootfs.ext4"))
            .unwrap()
            .set_len(3 << 30)
            .unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("rootfs.ext4"), b"working set").unwrap();
        fs::write(parent.join("profile-state"), b"user-state").unwrap();
        fn used(path: &Path) -> u64 {
            fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        used(&entry.path())
                    } else {
                        meta.len()
                    }
                })
                .sum()
        }
        // The disk fits the download and unpack only without the abandoned stage.
        let (archive, unpacked) = (868 << 20, UNPACKED_SET_BYTES);
        let disk = archive
            + unpacked
            + elastos_common::FREE_SPACE_RESERVE_BYTES
            + (1 << 20)
            + used(parent)
            - (3 << 30);
        let free = |path: &Path| -> anyhow::Result<u64> { Ok(disk - used(path)) };
        assert!(image_space(free(parent).unwrap(), archive, unpacked).is_err());
        let info = PlatformInfo {
            size: Some(archive),
            ..serde_json::from_value(json!({})).unwrap()
        };
        require_space_measured(&dest, &info, unpacked, free).unwrap();
        assert!(!abandoned.exists());
        assert_eq!(fs::read(dest.join("rootfs.ext4")).unwrap(), b"working set");
        assert_eq!(
            fs::read(parent.join("profile-state")).unwrap(),
            b"user-state"
        );
    }
}
