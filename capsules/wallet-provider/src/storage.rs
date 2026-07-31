use super::*;
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(super) fn load_store(path: &Path) -> Result<WalletStore, String> {
    if !path.exists() {
        return Ok(WalletStore::default());
    }
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
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
        ) && (request.session_id.is_empty() || request.launch_id.is_empty())
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
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

pub(super) fn load_or_create_storage_key(wallet_dir: &Path) -> Result<[u8; 32], String> {
    let key_path = wallet_dir.join(WALLET_KEY_FILE);
    if key_path.exists() {
        let value = fs::read_to_string(&key_path).map_err(|err| err.to_string())?;
        let bytes = hex::decode(value.trim()).map_err(|err| err.to_string())?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "wallet storage key must be 32 bytes".to_string())?;
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&key_path).map_err(|err| err.to_string())?;
    file.write_all(hex::encode(key).as_bytes())
        .map_err(|err| err.to_string())?;
    file.write_all(b"\n").map_err(|err| err.to_string())?;
    Ok(key)
}
