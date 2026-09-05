use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const FILE: &str = ".elastos-engine.json";
const SCHEMA: &str = "elastos.local-model-engine/v2";

#[derive(Default)]
struct Inventory {
    entries: Vec<serde_json::Value>,
    directories: Vec<PathBuf>,
    files: Vec<(PathBuf, bool)>,
}

fn scan(bundle: &Path, directory: &Path, inventory: &mut Inventory) -> anyhow::Result<()> {
    inventory.directories.push(directory.to_path_buf());
    for child in fs::read_dir(directory)? {
        let path = child?.path();
        let relative_path = path.strip_prefix(bundle)?;
        if relative_path == Path::new(FILE) {
            continue;
        }
        let relative = relative_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("bundle path is not UTF-8"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            scan(bundle, &path, inventory)?;
        } else if metadata.file_type().is_file() {
            inventory.entries.push(serde_json::json!({
                "path": relative,
                "sha256": super::compute_sha256_checksum(&path)?,
                "type": "file"
            }));
            inventory
                .files
                .push((path, metadata.permissions().mode() & 0o111 != 0));
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            if target.as_os_str().is_empty() || unsafe_path(&target) {
                anyhow::bail!("bundle symlink target is unsafe");
            }
            inventory.entries.push(serde_json::json!({
                "path": relative,
                "target": target
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("bundle symlink target is not UTF-8"))?,
                "type": "symlink"
            }));
        } else {
            anyhow::bail!("bundle contains a special file");
        }
    }
    Ok(())
}

fn unsafe_path(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn protected(path: &Path, modes: &[u32], one_link: bool) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.uid() == unsafe { libc::getuid() }
        && (!one_link || metadata.nlink() == 1)
        && (modes.is_empty() || modes.contains(&(metadata.permissions().mode() & 0o7777)))
}

fn expected(
    bundle: &Path,
    version: &str,
    platform: &str,
    archive_sha256: &str,
    binary_path: &str,
) -> anyhow::Result<(serde_json::Value, Inventory)> {
    let binary_path = Path::new(binary_path);
    if unsafe_path(binary_path) {
        anyhow::bail!("bundle executable path is unsafe");
    }
    let binary = fs::symlink_metadata(bundle.join(binary_path))?;
    if !binary.file_type().is_file() || binary.permissions().mode() & 0o111 == 0 {
        anyhow::bail!("bundle executable is not a regular executable file");
    }
    let mut inventory = Inventory::default();
    scan(bundle, bundle, &mut inventory)?;
    inventory
        .entries
        .sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok((
        serde_json::json!({
            "archive_sha256": archive_sha256,
            "entries": inventory.entries,
            "platform": platform,
            "schema": SCHEMA,
            "version": version
        }),
        inventory,
    ))
}

fn validate_protection(bundle: &Path, inventory: &Inventory) -> anyhow::Result<()> {
    if inventory
        .directories
        .iter()
        .any(|path| !protected(path, &[0o500], false))
        || inventory
            .files
            .iter()
            .any(|(path, _)| !protected(path, &[0o400, 0o500], true))
    {
        anyhow::bail!("bundle protection is invalid");
    }
    let receipt = fs::symlink_metadata(bundle.join(FILE))?;
    if !receipt.file_type().is_file() || !protected(&bundle.join(FILE), &[0o400], true) {
        anyhow::bail!("bundle receipt protection is invalid");
    }
    Ok(())
}

pub(super) fn verify(
    bundle: &Path,
    version: &str,
    platform: &str,
    archive_sha256: &str,
    binary_path: &str,
) -> anyhow::Result<()> {
    let receipt = bundle.join(FILE);
    if !fs::symlink_metadata(&receipt)?.file_type().is_file() {
        anyhow::bail!("bundle receipt is not a regular file");
    }
    let (expected, inventory) = expected(bundle, version, platform, archive_sha256, binary_path)?;
    if serde_json::from_slice::<serde_json::Value>(&fs::read(receipt)?)? != expected {
        anyhow::bail!("bundle receipt does not match its inventory");
    }
    validate_protection(bundle, &inventory)
}

pub(super) fn write(
    bundle: &Path,
    version: &str,
    platform: &str,
    archive_sha256: &str,
    binary_path: &str,
) -> anyhow::Result<()> {
    let receipt = bundle.join(FILE);
    if fs::symlink_metadata(&receipt).is_ok() {
        anyhow::bail!("bundle contains the reserved receipt path");
    }
    let (payload, mut inventory) =
        expected(bundle, version, platform, archive_sha256, binary_path)?;
    if inventory
        .directories
        .iter()
        .any(|path| !protected(path, &[], false))
        || inventory
            .files
            .iter()
            .any(|(path, _)| !protected(path, &[], true))
    {
        anyhow::bail!("bundle ownership or link count is invalid");
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt)?;
    serde_json::to_writer(&mut output, &payload)?;
    output.write_all(b"\n")?;
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o400))?;
    for (path, executable) in &inventory.files {
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if *executable { 0o500 } else { 0o400 }),
        )?;
    }
    inventory
        .directories
        .sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in &inventory.directories {
        fs::set_permissions(path, fs::Permissions::from_mode(0o500))?;
    }
    validate_protection(bundle, &inventory)
}
