use super::*;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const MAX_WALLET_STORE_BYTES: usize = 64 * 1024 * 1024;
const MAX_WALLET_STORAGE_KEY_BYTES: usize = 128;

pub(super) fn load_store(path: &Path) -> Result<WalletStore, String> {
    Ok(load_store_if_present(path)?.unwrap_or_default())
}

pub(super) fn load_store_if_present(path: &Path) -> Result<Option<WalletStore>, String> {
    let Some(bytes) = read_regular_file_if_present(path, "wallet store", MAX_WALLET_STORE_BYTES)?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|err| err.to_string())
}

fn metadata_is_regular_leaf(metadata: &Metadata) -> bool {
    metadata.file_type().is_file() && !metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &Metadata, _right: &Metadata) -> bool {
    true
}

fn open_regular_file_if_present(path: &Path, label: &str) -> Result<Option<File>, String> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("failed to inspect {label}: {err}")),
    };
    if !metadata_is_regular_leaf(&before) {
        return Err(format!("{label} must be a regular non-symlink file"));
    }
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|err| format!("failed to open {label}: {err}"))?;
    let opened = file
        .metadata()
        .map_err(|err| format!("failed to inspect open {label}: {err}"))?;
    let after =
        fs::symlink_metadata(path).map_err(|err| format!("failed to re-inspect {label}: {err}"))?;
    if !metadata_is_regular_leaf(&opened)
        || !metadata_is_regular_leaf(&after)
        || !same_file(&before, &opened)
        || !same_file(&opened, &after)
    {
        return Err(format!("{label} changed while it was being opened"));
    }
    Ok(Some(file))
}

fn verify_open_file_is_current(path: &Path, file: &File, label: &str) -> Result<(), String> {
    let opened = file
        .metadata()
        .map_err(|err| format!("failed to inspect open {label}: {err}"))?;
    let current = fs::symlink_metadata(path)
        .map_err(|err| format!("failed to inspect current {label}: {err}"))?;
    if !metadata_is_regular_leaf(&opened)
        || !metadata_is_regular_leaf(&current)
        || !same_file(&opened, &current)
    {
        return Err(format!("{label} changed during access"));
    }
    Ok(())
}

fn read_limited_file(file: &mut File, label: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let file_bytes = file
        .metadata()
        .map_err(|err| format!("failed to inspect open {label}: {err}"))?
        .len();
    ensure_size_within_limit(label, file_bytes, max_bytes)?;
    let read_limit = max_bytes
        .checked_add(1)
        .ok_or_else(|| format!("{label} size limit is invalid"))? as u64;
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("failed to read {label}: {err}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("{label} exceeds the {max_bytes}-byte size limit"));
    }
    Ok(bytes)
}

fn ensure_size_within_limit(label: &str, size: u64, max_bytes: usize) -> Result<(), String> {
    if size > max_bytes as u64 {
        return Err(format!("{label} exceeds the {max_bytes}-byte size limit"));
    }
    Ok(())
}

fn read_regular_file_if_present(
    path: &Path,
    label: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let Some(mut file) = open_regular_file_if_present(path, label)? else {
        return Ok(None);
    };
    let bytes = read_limited_file(&mut file, label, max_bytes)?;
    verify_open_file_is_current(path, &file, label)?;
    Ok(Some(bytes))
}

fn ensure_regular_file_or_absent(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_regular_leaf(&metadata) => Ok(()),
        Ok(_) => Err(format!("{label} must be a regular non-symlink file")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to inspect {label}: {err}")),
    }
}

fn ensure_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|err| format!("failed to inspect {label}: {err}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{label} must be a non-symlink directory"));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path, label: &str) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| format!("failed to sync {label}: {err}"))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path, _label: &str) -> Result<(), String> {
    Ok(())
}

pub(super) fn create_wallet_dir_durable(wallet_dir: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut cursor = wallet_dir;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                    return Err(format!(
                        "wallet storage directory component must be a non-symlink directory: {}",
                        cursor.display()
                    ));
                }
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| "wallet storage directory has no existing parent".to_string())?;
            }
            Err(err) => {
                return Err(format!(
                    "failed to inspect wallet storage directory {}: {err}",
                    cursor.display()
                ))
            }
        }
    }

    fs::create_dir_all(wallet_dir)
        .map_err(|err| format!("failed to create wallet storage directory: {err}"))?;
    for directory in missing.iter().rev() {
        ensure_directory(directory, "wallet storage directory")?;
        sync_directory(directory, "wallet storage directory")?;
        let parent = directory
            .parent()
            .ok_or_else(|| "wallet storage directory has no parent to synchronize".to_string())?;
        ensure_directory(parent, "wallet storage parent directory")?;
        sync_directory(parent, "wallet storage parent directory")?;
    }
    ensure_directory(wallet_dir, "wallet storage directory")?;
    sync_directory(wallet_dir, "wallet storage directory")?;
    let parent = wallet_dir
        .parent()
        .ok_or_else(|| "wallet storage directory has no parent to synchronize".to_string())?;
    ensure_directory(parent, "wallet storage parent directory")?;
    sync_directory(parent, "wallet storage parent directory")
}

pub(super) fn prune_store(mut store: WalletStore, now: u64) -> WalletStore {
    prune_expired_lifecycles(&mut store, now);
    store
        .challenges
        .retain(|stored| stored.challenge.expires_at > now);
    store
        .bitcoin_challenges
        .retain(|stored| stored.challenge.expires_at > now);
    for request in &mut store.approval_requests {
        expire_approval_if_elapsed(request, now);
    }
    if store.approval_requests.len() > MAX_APPROVAL_HISTORY {
        store.approval_requests.sort_by_key(|request| {
            (
                request.status == ApprovalStatus::Pending,
                request.created_at,
            )
        });
        let excess = store.approval_requests.len() - MAX_APPROVAL_HISTORY;
        store.approval_requests.drain(0..excess);
        store
            .approval_requests
            .sort_by_key(|request| request.created_at);
    }
    store
}

pub(super) fn prune_expired_lifecycles(store: &mut WalletStore, now: u64) {
    store
        .consumed_lifecycles
        .retain(|record| record.request_expires_at > now);
}

pub(super) fn reject_pre_v2_pending_approvals(store: &mut WalletStore, now: u64) -> usize {
    let mut changed = 0;
    for request in &mut store.approval_requests {
        if matches!(
            request.status,
            ApprovalStatus::Pending | ApprovalStatus::Approved
        ) && (request.session_id.is_empty()
            || request.launch_id.is_empty()
            || request.wallet_request_sha256.is_empty()
            || request.authority_binding.is_empty())
        {
            request.status = ApprovalStatus::Rejected;
            request.resolved_at = Some(now);
            request.rejection_reason = Some(
                "pre-v2 approval preserved as history; a new authority-bound request is required"
                    .to_string(),
            );
            changed += 1;
        }
    }
    changed
}

pub(super) fn expire_approval_if_elapsed(request: &mut WalletApprovalRequest, now: u64) {
    if matches!(
        request.status,
        ApprovalStatus::Pending | ApprovalStatus::Approved
    ) && request.expires_at <= now
    {
        request.status = ApprovalStatus::Expired;
        request.resolved_at = Some(now);
    }
}

pub(super) fn save_store(path: &Path, store: &WalletStore) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(store).map_err(|err| err.to_string())?;
    ensure_size_within_limit("wallet store", bytes.len() as u64, MAX_WALLET_STORE_BYTES)?;
    let parent = path
        .parent()
        .ok_or_else(|| "wallet store path has no parent directory".to_string())?;
    ensure_directory(parent, "wallet store parent directory")?;
    ensure_regular_file_or_absent(path, "wallet store")?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "wallet store path has an invalid file name".to_string())?;
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        rand::thread_rng().next_u64()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&tmp).map_err(|err| err.to_string())?;
        #[cfg(unix)]
        {
            let mut permissions = file
                .metadata()
                .map_err(|err| err.to_string())?
                .permissions();
            permissions.set_mode(0o600);
            file.set_permissions(permissions)
                .map_err(|err| err.to_string())?;
        }
        file.write_all(&bytes).map_err(|err| err.to_string())?;
        file.sync_all().map_err(|err| err.to_string())?;
        ensure_regular_file_or_absent(path, "wallet store")?;
        fs::rename(&tmp, path).map_err(|err| err.to_string())?;
        verify_open_file_is_current(path, &file, "wallet store")?;
        sync_directory(parent, "wallet store parent directory")?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(err) => match fs::remove_file(&tmp) {
            Ok(()) => Err(err),
            Err(cleanup_err) if cleanup_err.kind() == std::io::ErrorKind::NotFound => Err(err),
            Err(cleanup_err) => Err(format!(
                "failed to save wallet store: {err}; failed to remove staging store: {cleanup_err}"
            )),
        },
    }
}

pub(super) fn load_or_create_storage_key(wallet_dir: &Path) -> Result<[u8; 32], String> {
    ensure_directory(wallet_dir, "wallet storage directory")?;
    let key_path = wallet_dir.join(WALLET_KEY_FILE);
    if let Some(key) = read_storage_key_if_present(&key_path)? {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let tmp = wallet_dir.join(format!(
        ".{WALLET_KEY_FILE}.{}.{}.tmp",
        std::process::id(),
        rand::thread_rng().next_u64()
    ));
    let staged = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&tmp).map_err(|err| err.to_string())?;
        #[cfg(unix)]
        {
            let mut permissions = file
                .metadata()
                .map_err(|err| err.to_string())?
                .permissions();
            permissions.set_mode(0o600);
            file.set_permissions(permissions)
                .map_err(|err| err.to_string())?;
        }
        file.write_all(hex::encode(key).as_bytes())
            .map_err(|err| err.to_string())?;
        file.write_all(b"\n").map_err(|err| err.to_string())?;
        file.sync_all().map_err(|err| err.to_string())
    })();
    if let Err(err) = staged {
        return match fs::remove_file(&tmp) {
            Ok(()) => Err(err),
            Err(cleanup_err) if cleanup_err.kind() == std::io::ErrorKind::NotFound => Err(err),
            Err(cleanup_err) => Err(format!(
                "failed to stage wallet storage key: {err}; failed to remove staging key: {cleanup_err}"
            )),
        };
    }
    match fs::hard_link(&tmp, &key_path) {
        Ok(()) => {
            sync_directory(wallet_dir, "wallet storage directory")?;
            fs::remove_file(&tmp).map_err(|err| err.to_string())?;
            sync_directory(wallet_dir, "wallet storage directory")?;
            let installed = read_storage_key(&key_path)?;
            if installed != key {
                return Err("wallet storage key changed during installation".to_string());
            }
            Ok(installed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(&tmp).map_err(|remove_err| remove_err.to_string())?;
            sync_directory(wallet_dir, "wallet storage directory")?;
            read_storage_key(&key_path)
        }
        Err(err) => {
            let cleanup = fs::remove_file(&tmp);
            match cleanup {
                Ok(()) => Err(err.to_string()),
                Err(cleanup_err) => Err(format!(
                    "failed to install wallet storage key: {err}; failed to remove staging key: {cleanup_err}"
                )),
            }
        }
    }
}

fn read_storage_key(key_path: &Path) -> Result<[u8; 32], String> {
    read_storage_key_if_present(key_path)?
        .ok_or_else(|| "wallet storage key does not exist".to_string())
}

fn read_storage_key_if_present(key_path: &Path) -> Result<Option<[u8; 32]>, String> {
    let Some(mut file) = open_regular_file_if_present(key_path, "wallet storage key")? else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        let mut permissions = file
            .metadata()
            .map_err(|err| format!("failed to inspect wallet storage key mode: {err}"))?
            .permissions();
        if permissions.mode() & 0o7777 != 0o600 {
            permissions.set_mode(0o600);
            file.set_permissions(permissions)
                .map_err(|err| format!("failed to repair wallet storage key mode: {err}"))?;
            file.sync_all()
                .map_err(|err| format!("failed to sync repaired wallet storage key: {err}"))?;
        }
    }
    let value = String::from_utf8(read_limited_file(
        &mut file,
        "wallet storage key",
        MAX_WALLET_STORAGE_KEY_BYTES,
    )?)
    .map_err(|err| format!("wallet storage key must be UTF-8 text: {err}"))?;
    verify_open_file_is_current(key_path, &file, "wallet storage key")?;
    let wallet_dir = key_path
        .parent()
        .ok_or_else(|| "wallet storage key path has no parent directory".to_string())?;
    sync_directory(wallet_dir, "wallet storage directory")?;
    let bytes = hex::decode(value.trim()).map_err(|err| err.to_string())?;
    bytes
        .try_into()
        .map(Some)
        .map_err(|_| "wallet storage key must be 32 bytes".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_key_bootstrap_is_private_atomic_and_concurrent() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::sync::Arc::new(dir.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let threads = (0..12)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    load_or_create_storage_key(path.as_ref())
                })
            })
            .collect::<Vec<_>>();
        let keys = threads
            .into_iter()
            .map(|thread| thread.join().unwrap().unwrap())
            .collect::<Vec<_>>();

        assert!(keys.iter().all(|key| key == &keys[0]));
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>(),
            vec![std::ffi::OsString::from(WALLET_KEY_FILE)]
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(dir.path().join(WALLET_KEY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }

    #[test]
    fn wallet_store_is_atomic_private_and_size_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet-state.json");
        let mut store = WalletStore::default();
        store.default_accounts.push(DefaultWalletAccount {
            schema: "elastos.wallet.default_account/v1".to_string(),
            principal_id: "person:local:alice".to_string(),
            chain_namespace: "eip155:20".to_string(),
            intent: "browser_personal_sign".to_string(),
            account_id: "wallet:eip155:20:test".to_string(),
            set_at: 1,
        });
        save_store(&path, &store).unwrap();
        assert_eq!(load_store(&path).unwrap().default_accounts.len(), 1);
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let oversized = dir.path().join("oversized.json");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_WALLET_STORE_BYTES as u64 + 1)
            .unwrap();
        assert!(load_store(&oversized)
            .unwrap_err()
            .contains("67108864-byte size limit"));
    }

    #[cfg(unix)]
    #[test]
    fn wallet_storage_rejects_symlink_leafs_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let external_store = dir.path().join("external-state.json");
        let external_bytes = serde_json::to_vec_pretty(&WalletStore::default()).unwrap();
        fs::write(&external_store, &external_bytes).unwrap();
        let store_path = dir.path().join("wallet-state.json");
        symlink(&external_store, &store_path).unwrap();
        assert!(load_store(&store_path)
            .unwrap_err()
            .contains("regular non-symlink file"));
        assert!(save_store(&store_path, &WalletStore::default())
            .unwrap_err()
            .contains("regular non-symlink file"));
        assert_eq!(fs::read(&external_store).unwrap(), external_bytes);

        let wallet_dir = dir.path().join("wallet");
        fs::create_dir(&wallet_dir).unwrap();
        let external_key = dir.path().join("external-key.hex");
        let external_key_contents = format!("{}\n", "ab".repeat(32));
        fs::write(&external_key, &external_key_contents).unwrap();
        symlink(&external_key, wallet_dir.join(WALLET_KEY_FILE)).unwrap();
        assert!(load_or_create_storage_key(&wallet_dir)
            .unwrap_err()
            .contains("regular non-symlink file"));
        assert_eq!(
            fs::read_to_string(&external_key).unwrap(),
            external_key_contents
        );
    }

    #[test]
    fn wallet_directory_bootstrap_creates_the_full_durable_leaf_chain() {
        let dir = tempfile::tempdir().unwrap();
        let wallet_dir = dir
            .path()
            .join("ElastOS")
            .join("SystemServices")
            .join("Wallet");

        create_wallet_dir_durable(&wallet_dir).unwrap();
        create_wallet_dir_durable(&wallet_dir).unwrap();
        assert!(wallet_dir.is_dir());
    }
}
