//! Runtime's installed signed inputs. Publisher keeps independent publication custody.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result};
use elastos_common::localhost::{
    installation_release_head_path, installation_release_manifest_path,
    publisher_release_head_path, publisher_release_manifest_path,
};
use sha2::{Digest, Sha256};

use crate::install_transaction::{InstallTransaction, InstallationGuard};
use crate::sources::TrustedSource;

const MAX_METADATA: u64 = 256 * 1024;
const MAX_COMPONENTS: u64 = 4 * 1024 * 1024;
const MIGRATION: &str = ".elastos.installation-migrate";
const HEAD: &str = "release-head.json";
const RELEASE: &str = "release.json";
const REPAIR: &str = "Installed release inputs require repair. Preserve the installation files and ask the operator to restore the verified signed pair.";

pub(crate) struct InstalledRelease {
    pub(crate) head: Vec<u8>,
    pub(crate) release: Vec<u8>,
    pub(crate) binary_sha256: String,
}

/// The caller holds the installation guard across migration and its next mutation.
pub(crate) fn load_or_migrate(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    guard: &InstallationGuard,
) -> Result<InstalledRelease> {
    guard.require_binary(binary)?;
    require_no_pending(binary)?;
    check_directory(data, false)?;
    require_current_source(data, source)?;
    let root = data.join("installation");
    let stage = data.join(MIGRATION);
    if consumed_present(data)? {
        let admitted = read_for_setup(data, binary, source, guard)?;
        let release = serde_json::from_slice(&admitted.release).context(REPAIR)?;
        admit_complete_support(data, binary, &release).context(REPAIR)?;
        if present(&stage)? {
            clean_partial_stage(&stage, &admitted)?;
        }
        return Ok(admitted);
    }
    let mut legacy = None;
    if present(&stage)? {
        let names = stage_inventory(&stage)?;
        if names.len() == 2 {
            let admitted = read_pair(
                data,
                binary,
                source,
                &stage.join(HEAD),
                &stage.join(RELEASE),
                true,
            )?;
            publish_stage(data, binary, &stage, &root)?;
            return Ok(admitted);
        }
        let admitted = read_pair(
            data,
            binary,
            source,
            &publisher_release_head_path(data),
            &publisher_release_manifest_path(data),
            false,
        )?;
        clean_partial_stage(&stage, &admitted)?;
        legacy = Some(admitted);
    }
    let admitted = match legacy {
        Some(admitted) => admitted,
        None => read_pair(
            data,
            binary,
            source,
            &publisher_release_head_path(data),
            &publisher_release_manifest_path(data),
            false,
        )?,
    };
    crate::install_transaction::require_controller_disk_reserve(
        data,
        (admitted.head.len() + admitted.release.len()) as u64,
    )?;
    fs::DirBuilder::new().mode(0o700).create(&stage)?;
    sync(data)?;
    write_private(&stage.join(HEAD), &admitted.head)?;
    write_private(&stage.join(RELEASE), &admitted.release)?;
    sync(&stage)?;
    publish_stage(data, binary, &stage, &root)?;
    Ok(admitted)
}

/// Early space refusal can admit an unchanged Home without creating migration state.
/// Any present consumed state remains authoritative, including a partial or unsafe pair.
pub(crate) fn read_without_migration(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
) -> Result<InstalledRelease> {
    require_no_pending(binary)?;
    require_current_source(data, source)?;
    if consumed_present(data)? {
        read_pair(
            data,
            binary,
            source,
            &installation_release_head_path(data),
            &installation_release_manifest_path(data),
            true,
        )
    } else {
        read_pair(
            data,
            binary,
            source,
            &publisher_release_head_path(data),
            &publisher_release_manifest_path(data),
            false,
        )
    }
}

/// Setup holds this guard while it obtains signed support for an installed binary.
/// Its authority starts with Runtime's private pair, before support is present.
pub(crate) fn read_for_setup(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    guard: &InstallationGuard,
) -> Result<InstalledRelease> {
    guard.require_binary(binary)?;
    require_no_pending(binary)?;
    require_current_source(data, source)?;
    anyhow::ensure!(consumed_present(data)?, "{REPAIR}");
    read_pair_and_binary(
        data,
        binary,
        source,
        &installation_release_head_path(data),
        &installation_release_manifest_path(data),
        true,
    )
    .map(|(installed, _)| installed)
}

/// Recovery owns the namespace permanently selected by its journal schema.
pub(crate) fn read_for_transaction(
    transaction: &InstallTransaction,
    source: &TrustedSource,
) -> Result<InstalledRelease> {
    transaction
        .writer_guard()
        .require_binary(transaction.binary_path())?;
    read_pair(
        transaction.data_dir(),
        transaction.binary_path(),
        source,
        transaction.release_head_path(),
        transaction.release_manifest_path(),
        transaction.uses_consumed_layout(),
    )
}

fn require_current_source(data: &Path, source: &TrustedSource) -> Result<()> {
    let current = crate::sources::load_trusted_sources(data)?;
    anyhow::ensure!(
        current
            .source_named(Some(&source.name))
            .is_some_and(
                |stored| serde_json::to_value(stored).ok() == serde_json::to_value(source).ok()
            ),
        "Installed trusted source changed. Check the update again."
    );
    Ok(())
}

fn require_no_pending(binary: &Path) -> Result<()> {
    anyhow::ensure!(!InstallTransaction::has_pending_recovery(binary),
        "An interrupted update requires its original recovery owner before installed-input migration.");
    Ok(())
}

fn read_pair(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    head: &Path,
    release: &Path,
    private: bool,
) -> Result<InstalledRelease> {
    let (installed, release) = read_pair_and_binary(data, binary, source, head, release, private)?;
    admit_complete_support(data, binary, &release).context(REPAIR)?;
    Ok(installed)
}

fn admit_complete_support(data: &Path, binary: &Path, release: &serde_json::Value) -> Result<()> {
    let platform = &release["payload"]["platforms"][crate::update::detect_release_platform()];
    let components = read_regular(&data.join("components.json"), MAX_COMPONENTS, false)?;
    let components_sha256 = hex::encode(Sha256::digest(&components));
    admit_descriptor(
        &platform["components"],
        &components_sha256,
        components.len() as u64,
    )?;
    // The signed descriptor binds chunked artifact bytes by checksum and size;
    // the head/release envelopes additionally have reconstructible metadata CIDs.
    admit_support(data, binary, &components)
}

fn read_pair_and_binary(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    head: &Path,
    release: &Path,
    private: bool,
) -> Result<(InstalledRelease, serde_json::Value)> {
    check_parents(data, head.parent().context(REPAIR)?, private)?;
    let head_bytes = read_regular(head, MAX_METADATA, private).context(REPAIR)?;
    let release_bytes = read_regular(release, MAX_METADATA, private).context(REPAIR)?;
    admit_pair_and_binary(binary, source, &head_bytes, &release_bytes).context(REPAIR)
}

fn admit_pair_and_binary(
    binary: &Path,
    source: &TrustedSource,
    head_bytes: &[u8],
    release_bytes: &[u8],
) -> Result<(InstalledRelease, serde_json::Value)> {
    anyhow::ensure!(
        !source.publisher_dids.is_empty()
            && source.publisher_dids.iter().all(|did| !did.is_empty()),
        "Installed publisher pins are missing"
    );
    let (head, _) = crate::crypto::verify_release_envelope_against_dids(
        head_bytes,
        "elastos.release.head.v1",
        &source.publisher_dids,
    )?;
    let (release, _) = crate::crypto::verify_release_envelope_against_dids(
        release_bytes,
        "elastos.release.v1",
        &source.publisher_dids,
    )?;
    // Older verified installers saved an empty discovery pin. Their signed pair
    // still binds the installed release; an independently saved pin remains exact.
    if !source.head_cid.is_empty() {
        crate::update::verify_release_metadata_cid(&source.head_cid, head_bytes)?;
    }
    let release_cid = head["payload"]["latest_release_cid"]
        .as_str()
        .context("Installed release CID is missing")?;
    crate::update::verify_release_metadata_cid(release_cid, release_bytes)?;
    crate::update::verify_release_binding(&head, release_bytes, &release)?;
    crate::update::verify_source_channel(
        source,
        head["payload"]["channel"].as_str().unwrap_or(""),
    )?;
    anyhow::ensure!(
        head["payload"]["version"].as_str() == Some(source.installed_version.as_str()),
        "Installed signed version differs from its source record"
    );
    semver::Version::parse(&source.installed_version)?;
    anyhow::ensure!(
        fs::symlink_metadata(&source.install_path)?.is_file()
            && fs::canonicalize(&source.install_path)? == binary,
        "Installed source binary path differs from its writer"
    );
    let platform = &release["payload"]["platforms"][crate::update::detect_release_platform()];
    let (binary_sha256, binary_size) = file_digest(binary)?;
    admit_descriptor(&platform["binary"], &binary_sha256, binary_size)?;
    Ok((
        InstalledRelease {
            head: head_bytes.to_vec(),
            release: release_bytes.to_vec(),
            binary_sha256,
        },
        release,
    ))
}

pub(crate) fn admit_descriptor(
    descriptor: &serde_json::Value,
    hash: &str,
    size: u64,
) -> Result<()> {
    let cid = cid::Cid::try_from(
        descriptor["cid"]
            .as_str()
            .context("Installed artifact CID is missing")?,
    )?;
    anyhow::ensure!(
        cid.hash().code() == 0x12
            && cid.hash().digest().len() == 32
            && matches!(cid.codec(), 0x55 | 0x70),
        "Installed artifact CID is unsupported"
    );
    anyhow::ensure!(
        descriptor["sha256"].as_str() == Some(hash),
        "Installed artifact differs from its signed checksum"
    );
    if cid.codec() == 0x55 {
        anyhow::ensure!(
            cid.hash().digest() == hex::decode(hash)?.as_slice(),
            "Installed raw artifact CID differs from its checksum"
        );
    }
    anyhow::ensure!(
        descriptor["size"].as_u64() == Some(size),
        "Installed artifact differs from its signed size"
    );
    Ok(())
}

fn admit_support(data: &Path, binary: &Path, components: &[u8]) -> Result<()> {
    let excluded = BTreeSet::from([
        binary.to_path_buf(),
        binary
            .parent()
            .context(REPAIR)?
            .join(".elastos.install.lock"),
    ]);
    crate::update::frozen_support_snapshot(
        data,
        components,
        components,
        &crate::setup::detect_platform(),
        &excluded,
    )?;
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(components)?;
    let platform = crate::setup::detect_platform();
    let required = manifest
        .profiles
        .get("home")
        .map(|profile| &profile.components);
    for (name, component) in &manifest.external {
        let info = crate::setup::resolve_platform_info(component, &platform);
        let relative = crate::setup::resolve_install_path(component, info);
        if component.provider_runtime.is_some() {
            crate::setup::validate_provider_runtime(name, component)?;
            // A provider outside the Home profile (for example custody-provider)
            // is optional: verify it when installed, never require it.
            let path = relative.map(|relative| data.join(relative));
            let present = path
                .as_ref()
                .is_some_and(|path| fs::symlink_metadata(path).is_ok());
            if present || required.is_some_and(|names| names.contains(name)) {
                let path = path.context("Installed native provider path is missing")?;
                check_parents(data, path.parent().context(REPAIR)?, false)?;
                file_digest(&path)?;
                crate::setup::verify_installed_component_binary(data, name, &path)?;
            }
        } else if let (Some(info), Some(relative)) = (info, relative) {
            if info.extract_path.is_none() && name != "home" {
                let path = data.join(relative);
                let state = fs::symlink_metadata(&path);
                if state.is_ok() || required.is_some_and(|names| names.contains(name)) {
                    check_parents(data, path.parent().context(REPAIR)?, false)?;
                    let (hash, _) = file_digest(&path)?;
                    anyhow::ensure!(
                        info.checksum.as_deref() == Some(format!("sha256:{hash}").as_str()),
                        "Installed component differs from its signed checksum"
                    );
                }
            }
        }
    }
    if let Some(trust) = &manifest.model_catalog {
        let bytes = read_regular(&data.join("model-catalog.json"), 128 * 1024, false)?;
        let envelope: serde_json::Value = serde_json::from_slice(&bytes)?;
        let published_at = envelope["payload"]["published_at"]
            .as_u64()
            .context("Installed catalogue publication time is missing")?;
        // Installed custody retains the signed snapshot. Current offer discovery
        // continues to enforce its expiry at the current time.
        crate::api::capsule_inventory::verify_model_catalog(trust, &bytes, published_at)?;
    }
    if manifest.external.contains_key("home") || manifest.capsules.contains_key("home") {
        let document = crate::api::browser_capsules::installed_home_document(data)
            .context("Installed Home document is unavailable")?;
        read_regular(&document, 2 * 1024 * 1024, false)?;
    }
    Ok(())
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn consumed_present(data: &Path) -> Result<bool> {
    let root = data.join("installation");
    if !present(&root)? {
        return Ok(false);
    }
    check_directory(&root, true)?;
    let mut count = 0;
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name();
        anyhow::ensure!(name == HEAD || name == RELEASE, "{REPAIR}");
        count += 1;
    }
    Ok(count > 0)
}

fn check_directory(path: &Path, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7022 == 0
            && (!private || metadata.mode() & 0o7777 == 0o700),
        "{REPAIR}"
    );
    Ok(())
}

fn check_parents(data: &Path, parent: &Path, private: bool) -> Result<()> {
    check_directory(data, false)?;
    let relative = parent.strip_prefix(data).context(REPAIR)?;
    let mut path = data.to_path_buf();
    for part in relative.components() {
        anyhow::ensure!(matches!(part, std::path::Component::Normal(_)), "{REPAIR}");
        path.push(part);
        check_directory(&path, private)?;
    }
    Ok(())
}

fn open_regular(path: &Path, private: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7022 == 0
            && (!private || metadata.mode() & 0o7777 == 0o600),
        "{REPAIR}"
    );
    Ok(file)
}

fn read_regular(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>> {
    let file = open_regular(path, private)?;
    anyhow::ensure!(
        file.metadata()?.len() <= limit,
        "Installed input exceeds its byte bound"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "Installed input exceeds its byte bound"
    );
    Ok(bytes)
}

fn file_digest(path: &Path) -> Result<(String, u64)> {
    let mut file = open_regular(path, false)?;
    let before = file.metadata()?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    anyhow::ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Installed artifact changed during admission"
    );
    Ok((hex::encode(digest.finalize()), after.len()))
}

fn stage_inventory(stage: &Path) -> Result<BTreeSet<String>> {
    check_directory(stage, true)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(stage)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!(REPAIR))?;
        anyhow::ensure!(
            matches!(name.as_str(), HEAD | RELEASE) && names.insert(name.clone()),
            "{REPAIR}"
        );
        read_regular(&stage.join(name), MAX_METADATA, true)?;
    }
    Ok(names)
}

fn clean_partial_stage(stage: &Path, admitted: &InstalledRelease) -> Result<()> {
    let names = stage_inventory(stage)?;
    anyhow::ensure!(names.len() < 2, "{REPAIR}");
    for name in &names {
        let bytes = read_regular(&stage.join(name), MAX_METADATA, true)?;
        let expected = if name == HEAD {
            &admitted.head
        } else {
            &admitted.release
        };
        anyhow::ensure!(expected.starts_with(&bytes), "{REPAIR}");
    }
    for name in names {
        fs::remove_file(stage.join(name))?;
    }
    fs::remove_dir(stage)?;
    sync(stage.parent().context(REPAIR)?)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn publish_stage(data: &Path, binary: &Path, stage: &Path, root: &Path) -> Result<()> {
    require_no_pending(binary)?;
    anyhow::ensure!(!consumed_present(data)?, "{REPAIR}");
    sync(stage)?;
    fs::rename(stage, root)?;
    sync(data)
}

fn sync(path: &Path) -> Result<()> {
    File::open(path)?.sync_all().map_err(Into::into)
}

#[cfg(test)]
mod tests;
