use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use nix::unistd::geteuid;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use elastos_protected_content_contracts::{
    CanonicalContract, CustodyNodeProvisioningRecordIdentityV1, CustodyNodeProvisioningRecordV1,
    Digest32, KeyEnvelopeIdentityV1, NodePublicKey, RuntimeCustodyProvisioningIdV1,
    RuntimeOperationIssuerKeyV1, SignedRuntimeCustodyProvisioningV1, MAX_KEY_ENVELOPE_BYTES,
};

use crate::{NodeCustodySecretKeyV1, NodeLocalStoredShareV1};

const STORE_MAGIC: &[u8; 8] = b"epc-ns01";
const STORE_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/node-share-store/state/v1";
const SLOT_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/node-share-store/slot/v1";
const RECEIPT_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/node-share-store/receipt/v1";
const STORE_LOCK_FILE: &str = "node-share-store.lock";
const STORE_DIGEST_BYTES: usize = 32;
const STORE_HEADER_BYTES: usize = 8 + 32 + 32 + 8 + 4 + 4 + 32;
const MAX_RECORD_BYTES: usize = MAX_KEY_ENVELOPE_BYTES as usize;
const MAX_SIGNED_PROVISIONING_BYTES: usize = 8 * 1024;
const MAX_STORE_FILE_BYTES: usize =
    STORE_HEADER_BYTES + MAX_RECORD_BYTES + MAX_SIGNED_PROVISIONING_BYTES + STORE_DIGEST_BYTES;
const MAX_NODE_SHARE_RECORDS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NodeLocalShareStoreErrorV1 {
    #[error("node share store is unavailable")]
    Unavailable,
    #[error("node share store record is corrupt")]
    Corrupt,
    #[error("node share store record conflicts with existing authority")]
    Conflict,
    #[error("node share store capacity exceeded")]
    CapacityExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeLocalShareReceiptV1 {
    slot_hash: Digest32,
    receipt_hash: Digest32,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
    provisioning_id: RuntimeCustodyProvisioningIdV1,
    provisioning_operation_hash: Digest32,
    selected_node_public_key: NodePublicKey,
    accepted_at: u64,
}

impl NodeLocalShareReceiptV1 {
    pub const fn slot_hash(&self) -> Digest32 {
        self.slot_hash
    }

    pub const fn receipt_hash(&self) -> Digest32 {
        self.receipt_hash
    }

    pub const fn record_identity(&self) -> CustodyNodeProvisioningRecordIdentityV1 {
        self.record_identity
    }

    pub const fn provisioning_id(&self) -> RuntimeCustodyProvisioningIdV1 {
        self.provisioning_id
    }

    pub const fn provisioning_operation_hash(&self) -> Digest32 {
        self.provisioning_operation_hash
    }

    pub const fn selected_node_public_key(&self) -> NodePublicKey {
        self.selected_node_public_key
    }

    pub const fn accepted_at(&self) -> u64 {
        self.accepted_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedNodeLocalShareV1 {
    receipt: NodeLocalShareReceiptV1,
    node_share: NodeLocalStoredShareV1,
}

impl ProvisionedNodeLocalShareV1 {
    pub const fn receipt(&self) -> &NodeLocalShareReceiptV1 {
        &self.receipt
    }

    pub const fn node_share(&self) -> &NodeLocalStoredShareV1 {
        &self.node_share
    }
}

#[derive(Clone)]
pub struct NodeLocalShareStoreV1 {
    node_public_key: NodePublicKey,
    root_dir: PathBuf,
    lock_path: PathBuf,
}

impl fmt::Debug for NodeLocalShareStoreV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeLocalShareStoreV1")
            .field("node_public_key", &self.node_public_key)
            .field("root_dir", &"[redacted]")
            .field("lock_path", &"[redacted]")
            .finish()
    }
}

impl NodeLocalShareStoreV1 {
    pub fn new(node_public_key: NodePublicKey, root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            node_public_key,
            lock_path: root_dir.join(STORE_LOCK_FILE),
            root_dir,
        }
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub fn provision_node_share(
        &self,
        record: &CustodyNodeProvisioningRecordV1,
        signed_provisioning: &SignedRuntimeCustodyProvisioningV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        node_custody_secret: &NodeCustodySecretKeyV1,
        now: u64,
    ) -> Result<ProvisionedNodeLocalShareV1, crate::CustodyError> {
        let record_bytes = canonical_record_bytes(record)?;
        let signed_bytes = canonical_signed_provisioning_bytes(signed_provisioning)?;
        let authenticated =
            signed_provisioning.verify_for_record(record, expected_runtime_issuer, now)?;
        let node_share = NodeLocalStoredShareV1::from_authenticated_provisioning(
            record,
            &authenticated,
            self.node_public_key,
            node_custody_secret,
        )?;
        let slot_hash = slot_hash_for(record.key_envelope_identity(), self.node_public_key)?;
        let receipt = receipt_for(
            slot_hash,
            record.record_identity()?,
            authenticated.provisioning_id(),
            authenticated.operation_hash(),
            self.node_public_key,
            now,
        );
        let entry = StoreEntryV1 {
            slot_hash,
            accepted_at: now,
            record_bytes,
            signed_provisioning_bytes: signed_bytes,
            receipt_hash: receipt.receipt_hash(),
        };

        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        self.write_or_replay_exact(
            entry,
            receipt,
            node_share,
            expected_runtime_issuer,
            node_custody_secret,
        )
    }

    pub fn load_node_share(
        &self,
        key_envelope_identity: &KeyEnvelopeIdentityV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        node_custody_secret: &NodeCustodySecretKeyV1,
    ) -> Result<ProvisionedNodeLocalShareV1, crate::CustodyError> {
        let slot_hash = slot_hash_for(key_envelope_identity, self.node_public_key)?;
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let entry = self.read_entry_for_slot(slot_hash)?;
        validate_loaded_entry(
            entry,
            self.node_public_key,
            expected_runtime_issuer,
            node_custody_secret,
        )
    }

    fn write_or_replay_exact(
        &self,
        entry: StoreEntryV1,
        receipt: NodeLocalShareReceiptV1,
        node_share: NodeLocalStoredShareV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        node_custody_secret: &NodeCustodySecretKeyV1,
    ) -> Result<ProvisionedNodeLocalShareV1, crate::CustodyError> {
        let path = self.record_path(entry.slot_hash);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_owner_only_regular_file_metadata(&metadata)?;
                let existing = read_entry_from_path(&path, entry.slot_hash, self.node_public_key)?;
                if existing.record_bytes == entry.record_bytes
                    && existing.signed_provisioning_bytes == entry.signed_provisioning_bytes
                {
                    return validate_loaded_entry(
                        existing,
                        self.node_public_key,
                        expected_runtime_issuer,
                        node_custody_secret,
                    );
                }
                return Err(NodeLocalShareStoreErrorV1::Conflict.into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(NodeLocalShareStoreErrorV1::Unavailable.into()),
        }

        if self.record_count()? >= MAX_NODE_SHARE_RECORDS {
            return Err(NodeLocalShareStoreErrorV1::CapacityExceeded.into());
        }

        let bytes = encode_entry(self.node_public_key, &entry)?;
        self.write_new_entry(entry.slot_hash, &bytes)?;
        Ok(ProvisionedNodeLocalShareV1 {
            receipt,
            node_share,
        })
    }

    fn read_entry_for_slot(
        &self,
        slot_hash: Digest32,
    ) -> Result<StoreEntryV1, NodeLocalShareStoreErrorV1> {
        read_entry_from_path(
            &self.record_path(slot_hash),
            slot_hash,
            self.node_public_key,
        )
    }

    fn write_new_entry(
        &self,
        slot_hash: Digest32,
        bytes: &[u8],
    ) -> Result<(), NodeLocalShareStoreErrorV1> {
        let temp_path = self.temp_path(slot_hash);
        remove_temp_file_if_present(&temp_path)?;
        let mut temp_file = open_owner_only_temp_file_for_write(&temp_path)?;
        temp_file
            .write_all(bytes)
            .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
        temp_file
            .sync_all()
            .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
        validate_owner_only_regular_file_metadata(
            &temp_file
                .metadata()
                .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?,
        )?;
        fs::hard_link(&temp_path, self.record_path(slot_hash)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                NodeLocalShareStoreErrorV1::Conflict
            } else {
                NodeLocalShareStoreErrorV1::Unavailable
            }
        })?;
        sync_directory(&self.root_dir)?;
        fs::remove_file(&temp_path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
        sync_directory(&self.root_dir)
    }

    fn ensure_root_dir(&self) -> Result<(), NodeLocalShareStoreErrorV1> {
        let parent = self
            .root_dir
            .parent()
            .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
        validate_owner_only_directory(parent)?;
        match fs::symlink_metadata(&self.root_dir) {
            Ok(metadata) => validate_owner_only_directory_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_owner_only_directory(&self.root_dir)?;
                sync_directory(parent)?;
                validate_owner_only_directory(&self.root_dir)
            }
            Err(_) => Err(NodeLocalShareStoreErrorV1::Unavailable),
        }
    }

    fn cleanup_stale_temps(&self) -> Result<(), NodeLocalShareStoreErrorV1> {
        let mut removed_temp = false;
        for entry in
            fs::read_dir(&self.root_dir).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?
        {
            let entry = entry.map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
            if is_temp_name(name) {
                self.cleanup_temp_entry(name, &entry.path())?;
                removed_temp = true;
                continue;
            }
            if name == STORE_LOCK_FILE || is_record_name(name) {
                continue;
            }
            return Err(NodeLocalShareStoreErrorV1::Unavailable);
        }
        if removed_temp {
            sync_directory(&self.root_dir)
        } else {
            Ok(())
        }
    }

    fn cleanup_temp_entry(
        &self,
        name: &str,
        temp_path: &Path,
    ) -> Result<(), NodeLocalShareStoreErrorV1> {
        let slot_hash =
            slot_hash_from_temp_name(name).ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
        let temp_metadata =
            fs::symlink_metadata(temp_path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
        let record_path = self.record_path(slot_hash);
        match fs::symlink_metadata(&record_path) {
            Ok(record_metadata) => {
                validate_owner_only_regular_file_metadata_with_link_count(&temp_metadata, 2)?;
                validate_owner_only_regular_file_metadata_with_link_count(&record_metadata, 2)?;
                require_same_file(&temp_metadata, &record_metadata)?;
                let _entry = read_entry_from_path_with_link_count(
                    &record_path,
                    slot_hash,
                    self.node_public_key,
                    2,
                )?;
                fs::remove_file(temp_path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                validate_owner_only_regular_file_metadata(&temp_metadata)?;
                fs::remove_file(temp_path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)
            }
            Err(_) => Err(NodeLocalShareStoreErrorV1::Unavailable),
        }
    }

    fn record_count(&self) -> Result<usize, NodeLocalShareStoreErrorV1> {
        let mut count = 0usize;
        for entry in
            fs::read_dir(&self.root_dir).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?
        {
            let entry = entry.map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
            if name == STORE_LOCK_FILE {
                continue;
            }
            if !is_record_name(name) {
                return Err(NodeLocalShareStoreErrorV1::Unavailable);
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
            validate_owner_only_regular_file_metadata(&metadata)?;
            count = count
                .checked_add(1)
                .ok_or(NodeLocalShareStoreErrorV1::CapacityExceeded)?;
        }
        Ok(count)
    }

    fn record_path(&self, slot_hash: Digest32) -> PathBuf {
        self.root_dir.join(lower_hex_digest(slot_hash))
    }

    fn temp_path(&self, slot_hash: Digest32) -> PathBuf {
        self.root_dir
            .join(format!(".{}.tmp", lower_hex_digest(slot_hash)))
    }

    #[cfg(test)]
    fn test_record_path(&self, slot_hash: Digest32) -> PathBuf {
        self.record_path(slot_hash)
    }

    #[cfg(test)]
    fn test_temp_path(&self, slot_hash: Digest32) -> PathBuf {
        self.temp_path(slot_hash)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreEntryV1 {
    slot_hash: Digest32,
    accepted_at: u64,
    record_bytes: Vec<u8>,
    signed_provisioning_bytes: Vec<u8>,
    receipt_hash: Digest32,
}

fn validate_loaded_entry(
    entry: StoreEntryV1,
    expected_node_public_key: NodePublicKey,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    node_custody_secret: &NodeCustodySecretKeyV1,
) -> Result<ProvisionedNodeLocalShareV1, crate::CustodyError> {
    let record = CustodyNodeProvisioningRecordV1::from_canonical_bytes(&entry.record_bytes)?;
    if record.canonical_bytes()? != entry.record_bytes {
        return Err(NodeLocalShareStoreErrorV1::Corrupt.into());
    }
    let signed =
        SignedRuntimeCustodyProvisioningV1::from_canonical_bytes(&entry.signed_provisioning_bytes)?;
    if signed.canonical_bytes()? != entry.signed_provisioning_bytes {
        return Err(NodeLocalShareStoreErrorV1::Corrupt.into());
    }
    let slot_hash = slot_hash_for(record.key_envelope_identity(), expected_node_public_key)?;
    if slot_hash != entry.slot_hash {
        return Err(NodeLocalShareStoreErrorV1::Corrupt.into());
    }
    let authenticated =
        signed.verify_for_record(&record, expected_runtime_issuer, entry.accepted_at)?;
    let node_share = NodeLocalStoredShareV1::from_authenticated_provisioning(
        &record,
        &authenticated,
        expected_node_public_key,
        node_custody_secret,
    )?;
    let receipt = receipt_for(
        entry.slot_hash,
        record.record_identity()?,
        authenticated.provisioning_id(),
        authenticated.operation_hash(),
        expected_node_public_key,
        entry.accepted_at,
    );
    if receipt.receipt_hash() != entry.receipt_hash {
        return Err(NodeLocalShareStoreErrorV1::Corrupt.into());
    }
    Ok(ProvisionedNodeLocalShareV1 {
        receipt,
        node_share,
    })
}

fn canonical_record_bytes(
    record: &CustodyNodeProvisioningRecordV1,
) -> Result<Vec<u8>, crate::CustodyError> {
    let bytes = record.canonical_bytes()?;
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Unavailable.into());
    }
    Ok(bytes)
}

fn canonical_signed_provisioning_bytes(
    signed: &SignedRuntimeCustodyProvisioningV1,
) -> Result<Vec<u8>, crate::CustodyError> {
    let bytes = signed.canonical_bytes()?;
    if bytes.is_empty() || bytes.len() > MAX_SIGNED_PROVISIONING_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Unavailable.into());
    }
    Ok(bytes)
}

fn slot_hash_for(
    key_envelope_identity: &KeyEnvelopeIdentityV1,
    selected_node_public_key: NodePublicKey,
) -> Result<Digest32, crate::CustodyError> {
    let mut hasher = Sha256::new();
    hasher.update(SLOT_DIGEST_DOMAIN);
    hasher.update(key_envelope_identity.canonical_bytes()?);
    hasher.update(selected_node_public_key.as_bytes());
    Ok(Digest32::new(hasher.finalize().into()))
}

fn receipt_for(
    slot_hash: Digest32,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
    provisioning_id: RuntimeCustodyProvisioningIdV1,
    provisioning_operation_hash: Digest32,
    selected_node_public_key: NodePublicKey,
    accepted_at: u64,
) -> NodeLocalShareReceiptV1 {
    let receipt_hash = receipt_hash_for(
        slot_hash,
        record_identity,
        provisioning_id,
        provisioning_operation_hash,
        selected_node_public_key,
        accepted_at,
    );
    NodeLocalShareReceiptV1 {
        slot_hash,
        receipt_hash,
        record_identity,
        provisioning_id,
        provisioning_operation_hash,
        selected_node_public_key,
        accepted_at,
    }
}

fn receipt_hash_for(
    slot_hash: Digest32,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
    provisioning_id: RuntimeCustodyProvisioningIdV1,
    provisioning_operation_hash: Digest32,
    selected_node_public_key: NodePublicKey,
    accepted_at: u64,
) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(RECEIPT_DIGEST_DOMAIN);
    hasher.update(slot_hash.as_bytes());
    hasher.update(record_identity.record_sha256().as_bytes());
    hasher.update(record_identity.record_bytes().to_be_bytes());
    hasher.update(provisioning_id.digest().as_bytes());
    hasher.update(provisioning_operation_hash.as_bytes());
    hasher.update(selected_node_public_key.as_bytes());
    hasher.update(accepted_at.to_be_bytes());
    Digest32::new(hasher.finalize().into())
}

fn encode_entry(
    node_public_key: NodePublicKey,
    entry: &StoreEntryV1,
) -> Result<Vec<u8>, NodeLocalShareStoreErrorV1> {
    if entry.record_bytes.is_empty() || entry.record_bytes.len() > MAX_RECORD_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Unavailable);
    }
    if entry.signed_provisioning_bytes.is_empty()
        || entry.signed_provisioning_bytes.len() > MAX_SIGNED_PROVISIONING_BYTES
    {
        return Err(NodeLocalShareStoreErrorV1::Unavailable);
    }
    let record_len = u32::try_from(entry.record_bytes.len())
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    let signed_len = u32::try_from(entry.signed_provisioning_bytes.len())
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    let mut payload = Vec::with_capacity(
        STORE_HEADER_BYTES + entry.record_bytes.len() + entry.signed_provisioning_bytes.len(),
    );
    payload.extend_from_slice(STORE_MAGIC);
    payload.extend_from_slice(node_public_key.as_bytes());
    payload.extend_from_slice(entry.slot_hash.as_bytes());
    payload.extend_from_slice(&entry.accepted_at.to_be_bytes());
    payload.extend_from_slice(&record_len.to_be_bytes());
    payload.extend_from_slice(&signed_len.to_be_bytes());
    payload.extend_from_slice(entry.receipt_hash.as_bytes());
    payload.extend_from_slice(&entry.record_bytes);
    payload.extend_from_slice(&entry.signed_provisioning_bytes);
    if payload.len() + STORE_DIGEST_BYTES > MAX_STORE_FILE_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Unavailable);
    }
    let digest = store_integrity_digest(&payload);
    let mut bytes = payload;
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

fn decode_entry(
    bytes: &[u8],
    expected_slot_hash: Digest32,
    expected_node_public_key: NodePublicKey,
) -> Result<StoreEntryV1, NodeLocalShareStoreErrorV1> {
    if bytes.len() < STORE_HEADER_BYTES + STORE_DIGEST_BYTES || bytes.len() > MAX_STORE_FILE_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let (payload, digest) = bytes.split_at(bytes.len() - STORE_DIGEST_BYTES);
    if payload[..8] != *STORE_MAGIC {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    if store_integrity_digest(payload) != digest {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    if payload[8..40] != *expected_node_public_key.as_bytes() {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let slot_hash = Digest32::new(
        payload[40..72]
            .try_into()
            .map_err(|_| NodeLocalShareStoreErrorV1::Corrupt)?,
    );
    if slot_hash != expected_slot_hash {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let accepted_at = u64::from_be_bytes(
        payload[72..80]
            .try_into()
            .map_err(|_| NodeLocalShareStoreErrorV1::Corrupt)?,
    );
    let record_len = u32::from_be_bytes(
        payload[80..84]
            .try_into()
            .map_err(|_| NodeLocalShareStoreErrorV1::Corrupt)?,
    ) as usize;
    let signed_len = u32::from_be_bytes(
        payload[84..88]
            .try_into()
            .map_err(|_| NodeLocalShareStoreErrorV1::Corrupt)?,
    ) as usize;
    let receipt_hash = Digest32::new(
        payload[88..120]
            .try_into()
            .map_err(|_| NodeLocalShareStoreErrorV1::Corrupt)?,
    );
    if record_len == 0 || record_len > MAX_RECORD_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    if signed_len == 0 || signed_len > MAX_SIGNED_PROVISIONING_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let expected_len = STORE_HEADER_BYTES
        .checked_add(record_len)
        .and_then(|value| value.checked_add(signed_len))
        .ok_or(NodeLocalShareStoreErrorV1::Corrupt)?;
    if payload.len() != expected_len {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let record_start = STORE_HEADER_BYTES;
    let signed_start = record_start + record_len;
    Ok(StoreEntryV1 {
        slot_hash,
        accepted_at,
        record_bytes: payload[record_start..signed_start].to_vec(),
        signed_provisioning_bytes: payload[signed_start..].to_vec(),
        receipt_hash,
    })
}

fn read_entry_from_path(
    path: &Path,
    expected_slot_hash: Digest32,
    expected_node_public_key: NodePublicKey,
) -> Result<StoreEntryV1, NodeLocalShareStoreErrorV1> {
    read_entry_from_path_with_link_count(path, expected_slot_hash, expected_node_public_key, 1)
}

fn read_entry_from_path_with_link_count(
    path: &Path,
    expected_slot_hash: Digest32,
    expected_node_public_key: NodePublicKey,
    expected_link_count: u64,
) -> Result<StoreEntryV1, NodeLocalShareStoreErrorV1> {
    let pre_metadata =
        fs::symlink_metadata(path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_regular_file_metadata_with_link_count(&pre_metadata, expected_link_count)?;
    let file = open_owner_only_file_for_read(path)?;
    let open_metadata = file
        .metadata()
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_regular_file_metadata_with_link_count(&open_metadata, expected_link_count)?;
    require_same_file(&pre_metadata, &open_metadata)?;
    let metadata_len = usize::try_from(open_metadata.len())
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    if metadata_len > MAX_STORE_FILE_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    let mut bytes = Vec::with_capacity(metadata_len);
    file.take((MAX_STORE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    if bytes.len() > MAX_STORE_FILE_BYTES {
        return Err(NodeLocalShareStoreErrorV1::Corrupt);
    }
    decode_entry(&bytes, expected_slot_hash, expected_node_public_key)
}

fn create_owner_only_directory(path: &Path) -> Result<(), NodeLocalShareStoreErrorV1> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(NodeLocalShareStoreErrorV1::Unavailable),
    }
}

fn validate_owner_only_directory(path: &Path) -> Result<(), NodeLocalShareStoreErrorV1> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_directory_metadata(&metadata)
}

fn validate_owner_only_directory_metadata(
    metadata: &fs::Metadata,
) -> Result<(), NodeLocalShareStoreErrorV1> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(NodeLocalShareStoreErrorV1::Unavailable);
    }
    validate_owner_and_mode(metadata, 0o700)
}

fn validate_owner_only_regular_file_metadata(
    metadata: &fs::Metadata,
) -> Result<(), NodeLocalShareStoreErrorV1> {
    validate_owner_only_regular_file_metadata_with_link_count(metadata, 1)
}

fn validate_owner_only_regular_file_metadata_with_link_count(
    metadata: &fs::Metadata,
    expected_link_count: u64,
) -> Result<(), NodeLocalShareStoreErrorV1> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NodeLocalShareStoreErrorV1::Unavailable);
    }
    validate_owner_and_mode(metadata, 0o600)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != expected_link_count {
            return Err(NodeLocalShareStoreErrorV1::Unavailable);
        }
    }
    Ok(())
}

fn validate_owner_and_mode(
    metadata: &fs::Metadata,
    exact_mode: u32,
) -> Result<(), NodeLocalShareStoreErrorV1> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o777 != exact_mode {
            return Err(NodeLocalShareStoreErrorV1::Unavailable);
        }
    }
    Ok(())
}

fn require_same_file(
    pre_metadata: &fs::Metadata,
    open_metadata: &fs::Metadata,
) -> Result<(), NodeLocalShareStoreErrorV1> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if pre_metadata.dev() != open_metadata.dev()
            || pre_metadata.ino() != open_metadata.ino()
            || pre_metadata.len() != open_metadata.len()
        {
            return Err(NodeLocalShareStoreErrorV1::Unavailable);
        }
    }
    #[cfg(not(unix))]
    {
        if pre_metadata.len() != open_metadata.len() {
            return Err(NodeLocalShareStoreErrorV1::Unavailable);
        }
    }
    Ok(())
}

fn open_owner_only_file_for_read(path: &Path) -> Result<File, NodeLocalShareStoreErrorV1> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)
}

fn open_owner_only_temp_file_for_write(path: &Path) -> Result<File, NodeLocalShareStoreErrorV1> {
    let parent = path
        .parent()
        .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_directory(parent)?;
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_regular_file_metadata(
        &file
            .metadata()
            .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?,
    )?;
    sync_directory(parent)?;
    Ok(file)
}

fn remove_temp_file_if_present(path: &Path) -> Result<(), NodeLocalShareStoreErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_owner_only_regular_file_metadata(&metadata)?;
            fs::remove_file(path).map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
            let parent = path
                .parent()
                .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
            sync_directory(parent)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(NodeLocalShareStoreErrorV1::Unavailable),
    }
}

fn sync_directory(path: &Path) -> Result<(), NodeLocalShareStoreErrorV1> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)
}

#[derive(Debug)]
struct ExclusiveFileLock {
    file: Option<Flock<File>>,
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> Result<Self, NodeLocalShareStoreErrorV1> {
        let file = open_or_create_owner_only_lock_file(path)?;
        let file = Flock::lock(file, FlockArg::LockExclusive)
            .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
        validate_owner_only_regular_file_metadata(
            &file
                .metadata()
                .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?,
        )?;
        Ok(Self { file: Some(file) })
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

fn open_or_create_owner_only_lock_file(path: &Path) -> Result<File, NodeLocalShareStoreErrorV1> {
    let parent = path
        .parent()
        .ok_or(NodeLocalShareStoreErrorV1::Unavailable)?;
    validate_owner_only_directory(parent)?;
    let mut create_new = OpenOptions::new();
    create_new.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        create_new.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    match create_new.open(path) {
        Ok(file) => {
            validate_owner_only_regular_file_metadata(
                &file
                    .metadata()
                    .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?,
            )?;
            sync_directory(parent)?;
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;

                options.custom_flags(nix::libc::O_NOFOLLOW);
            }
            let file = options
                .open(path)
                .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?;
            validate_owner_only_regular_file_metadata(
                &file
                    .metadata()
                    .map_err(|_| NodeLocalShareStoreErrorV1::Unavailable)?,
            )?;
            Ok(file)
        }
        Err(_) => Err(NodeLocalShareStoreErrorV1::Unavailable),
    }
}

fn is_record_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else {
        return false;
    };
    let Some(stem) = rest.strip_suffix(".tmp") else {
        return false;
    };
    is_record_name(stem)
}

fn slot_hash_from_temp_name(name: &str) -> Option<Digest32> {
    let stem = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    digest_from_lower_hex(stem)
}

fn digest_from_lower_hex(value: &str) -> Option<Digest32> {
    if !is_record_name(value) {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes[index] = (high << 4) | low;
    }
    Some(Digest32::new(bytes))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn lower_hex_digest(digest: Digest32) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest.as_bytes() {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn store_integrity_digest(payload: &[u8]) -> [u8; STORE_DIGEST_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(STORE_DIGEST_DOMAIN);
    hasher.update(payload);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;
    use crate::test_support::{
        digest, node_custody_secret, node_public_key, provisioned_envelope, NOW,
    };
    use elastos_protected_content_contracts::{
        CustodyNodeProvisioningRecordV1, RuntimeCustodyProvisioningIdV1,
        RuntimeCustodyProvisioningStatementV1,
    };

    fn runtime_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
        RuntimeOperationIssuerKeyV1::new(runtime_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn provisioning_record(node_seed: u8) -> CustodyNodeProvisioningRecordV1 {
        let envelope = provisioned_envelope();
        let selected_node = node_public_key(node_seed);
        CustodyNodeProvisioningRecordV1::new(
            envelope.key_envelope_identity().unwrap(),
            envelope.manifest().clone(),
            selected_node,
            envelope
                .stored_share_for_node(selected_node)
                .unwrap()
                .clone(),
        )
        .unwrap()
    }

    fn signed_provisioning(
        record: &CustodyNodeProvisioningRecordV1,
        runtime_seed: u8,
        provisioning_seed: u8,
        issued_at: u64,
        expires_at: u64,
    ) -> SignedRuntimeCustodyProvisioningV1 {
        let key = runtime_key(runtime_seed);
        let statement = RuntimeCustodyProvisioningStatementV1::new(
            runtime_issuer(runtime_seed),
            record.record_identity().unwrap(),
            RuntimeCustodyProvisioningIdV1::new(digest(provisioning_seed)).unwrap(),
            issued_at,
            expires_at,
        )
        .unwrap();
        SignedRuntimeCustodyProvisioningV1::new(
            statement.clone(),
            key.sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn temp_root() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        temp
    }

    fn store(root: &Path, node_seed: u8) -> NodeLocalShareStoreV1 {
        NodeLocalShareStoreV1::new(node_public_key(node_seed), root.join("shares"))
    }

    fn provision(
        store: &NodeLocalShareStoreV1,
        record: &CustodyNodeProvisioningRecordV1,
        signed: &SignedRuntimeCustodyProvisioningV1,
    ) -> ProvisionedNodeLocalShareV1 {
        store
            .provision_node_share(
                record,
                signed,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 1,
            )
            .unwrap()
    }

    fn read_file(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    fn write_owner_only(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[test]
    fn node_share_store_provisions_restarts_loads_and_replays_exact_duplicate() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);

        let first = provision(&store, &record, &signed);
        let duplicate = store
            .provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 2,
            )
            .unwrap();
        assert_eq!(duplicate, first);

        let restarted = NodeLocalShareStoreV1::new(node_public_key(1), temp.path().join("shares"));
        let loaded = restarted
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .unwrap();
        assert_eq!(loaded, first);
        assert_eq!(loaded.node_share().node_public_key(), node_public_key(1));
        assert_eq!(loaded.node_share().stored_share(), record.sealed_share());
        assert_eq!(loaded.receipt().accepted_at(), NOW + 1);
        assert_eq!(
            loaded.receipt().record_identity(),
            record.record_identity().unwrap()
        );
        assert_eq!(
            loaded.receipt().provisioning_operation_hash(),
            signed.statement().canonical_hash().unwrap()
        );
    }

    #[test]
    fn node_share_store_uses_canonical_hash_filenames_and_no_aggregate_share_storage() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let envelope = provisioned_envelope();
        let selected_node = node_public_key(1);
        let record = CustodyNodeProvisioningRecordV1::new(
            envelope.key_envelope_identity().unwrap(),
            envelope.manifest().clone(),
            selected_node,
            envelope
                .stored_share_for_node(selected_node)
                .unwrap()
                .clone(),
        )
        .unwrap();
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let stored = provision(&store, &record, &signed);
        let record_path = store.test_record_path(stored.receipt().slot_hash());

        assert_eq!(
            record_path.file_name().unwrap().to_str().unwrap(),
            lower_hex_digest(stored.receipt().slot_hash())
        );
        let raw = read_file(&record_path);
        assert!(contains_subsequence(
            &raw,
            &record.sealed_share().canonical_bytes().unwrap()
        ));
        for node in [node_public_key(2), node_public_key(3)] {
            assert!(!contains_subsequence(
                &raw,
                &envelope
                    .stored_share_for_node(node)
                    .unwrap()
                    .canonical_bytes()
                    .unwrap()
            ));
        }
        assert!(!contains_subsequence(&raw, &[0x22; 32]));
        let debug = format!("{stored:?}");
        assert!(debug.contains("ProvisionedNodeLocalShareV1"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(&format!("{:?}", record.sealed_share().envelope())));
    }

    #[test]
    fn node_share_store_rejects_conflicting_reprovision_for_same_object_node_slot() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        provision(&store, &record, &signed);

        let different_operation = signed_provisioning(&record, 0x7a, 0xf2, NOW, NOW + 60);
        assert!(matches!(
            store.provision_node_share(
                &record,
                &different_operation,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 1,
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Conflict
            ))
        ));

        let mut changed_share = record.sealed_share().clone();
        changed_share = changed_share.tamper_envelope();
        let different_record = CustodyNodeProvisioningRecordV1::new(
            record.key_envelope_identity().clone(),
            record.manifest().clone(),
            record.selected_node_public_key(),
            changed_share,
        )
        .unwrap();
        let different_record_operation =
            signed_provisioning(&different_record, 0x7a, 0xf3, NOW, NOW + 60);
        assert!(matches!(
            store.provision_node_share(
                &different_record,
                &different_record_operation,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 1,
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Conflict
            ))
        ));
    }

    #[test]
    fn node_share_store_recovers_linked_temp_crash_and_preserves_exact_replay() {
        let temp = temp_root();
        let node_store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let first = provision(&node_store, &record, &signed);
        let record_path = node_store.test_record_path(first.receipt().slot_hash());
        let temp_path = node_store.test_temp_path(first.receipt().slot_hash());
        std::fs::hard_link(&record_path, &temp_path).unwrap();

        let restarted = store(temp.path(), 1);
        let loaded = restarted
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .unwrap();
        assert_eq!(loaded, first);
        assert!(!temp_path.exists());

        let duplicate = restarted
            .provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 2,
            )
            .unwrap();
        assert_eq!(duplicate, first);

        let conflicting_operation = signed_provisioning(&record, 0x7a, 0xf2, NOW, NOW + 60);
        assert!(matches!(
            restarted.provision_node_share(
                &record,
                &conflicting_operation,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 2,
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Conflict
            ))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn node_share_store_rejects_foreign_or_mismatched_temp_hard_links() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let first = provision(&store, &record, &signed);
        let record_path = store.test_record_path(first.receipt().slot_hash());
        let temp_path = store.test_temp_path(first.receipt().slot_hash());
        let foreign = temp.path().join("foreign-share-record");
        write_owner_only(&foreign, b"foreign");
        std::fs::hard_link(&foreign, &temp_path).unwrap();

        assert!(matches!(
            store.load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Unavailable
            ))
        ));
        assert!(temp_path.exists());

        std::fs::remove_file(&temp_path).unwrap();
        std::fs::remove_file(&foreign).unwrap();
        std::fs::hard_link(&record_path, &temp_path).unwrap();
        let extra_link = temp.path().join("extra-final-link");
        std::fs::hard_link(&record_path, &extra_link).unwrap();

        assert!(matches!(
            store.load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Unavailable
            ))
        ));
        assert!(temp_path.exists());
    }

    #[test]
    fn node_share_store_rejects_runtime_node_secret_and_time_mismatches_before_writing() {
        let temp = temp_root();
        let node_store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);

        assert!(matches!(
            node_store.provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7b),
                &node_custody_secret(1),
                NOW + 1,
            ),
            Err(crate::CustodyError::RuntimeCustodyProvisioning(
                elastos_protected_content_contracts::RuntimeCustodyProvisioningError::BindingMismatch(
                    "runtime_operation_issuer"
                )
            ))
        ));
        assert!(matches!(
            node_store.provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(2),
                NOW + 1,
            ),
            Err(crate::CustodyError::BindingMismatch(
                "node_custody_public_key"
            ))
        ));
        assert!(node_store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());
        assert!(node_store
            .provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 60,
            )
            .is_err());

        let wrong_node_store = store(temp.path(), 2);
        assert!(matches!(
            wrong_node_store.provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(2),
                NOW + 1,
            ),
            Err(crate::CustodyError::BindingMismatch("custody_node"))
        ));
    }

    #[test]
    fn node_share_store_reload_fails_closed_on_corruption_truncation_oversize_and_noncanonical_bytes(
    ) {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let stored = provision(&store, &record, &signed);
        let path = store.test_record_path(stored.receipt().slot_hash());

        let mut corrupt = read_file(&path);
        corrupt[STORE_HEADER_BYTES + 4] ^= 0x01;
        std::fs::write(&path, corrupt).unwrap();
        assert!(matches!(
            store.load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Corrupt
            ))
        ));

        std::fs::remove_file(&path).unwrap();
        write_owner_only(&path, &[0u8; STORE_HEADER_BYTES]);
        assert!(store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());

        std::fs::remove_file(&path).unwrap();
        write_owner_only(&path, &vec![0u8; MAX_STORE_FILE_BYTES + 1]);
        assert!(matches!(
            store.load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Corrupt
            ))
        ));

        std::fs::remove_file(&path).unwrap();
        let signed_bytes = signed.canonical_bytes().unwrap();
        let invalid_entry = StoreEntryV1 {
            slot_hash: stored.receipt().slot_hash(),
            accepted_at: NOW + 1,
            record_bytes: vec![1, 2, 3, 4],
            signed_provisioning_bytes: signed_bytes,
            receipt_hash: stored.receipt().receipt_hash(),
        };
        write_owner_only(
            &path,
            &encode_entry(node_public_key(1), &invalid_entry).unwrap(),
        );
        assert!(store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn node_share_store_rejects_symlink_hardlink_and_wrong_mode_records() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let stored = provision(&store, &record, &signed);
        let path = store.test_record_path(stored.receipt().slot_hash());
        let raw = read_file(&path);

        std::fs::remove_file(&path).unwrap();
        let target = temp.path().join("target");
        write_owner_only(&target, &raw);
        symlink(&target, &path).unwrap();
        assert!(store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&target).unwrap();
        write_owner_only(&path, &raw);
        let hardlink = temp.path().join("hardlink");
        std::fs::hard_link(&path, &hardlink).unwrap();
        assert!(store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());

        std::fs::remove_file(&hardlink).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store
            .load_node_share(
                record.key_envelope_identity(),
                runtime_issuer(0x7a),
                &node_custody_secret(1),
            )
            .is_err());
    }

    #[test]
    fn node_share_store_rejects_unexpected_entries_and_capacity_before_new_write() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);

        std::fs::create_dir_all(temp.path().join("shares")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(
                temp.path().join("shares"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        write_owner_only(
            &temp.path().join("shares").join("caller-path-string"),
            b"bad",
        );
        assert!(matches!(
            store.provision_node_share(
                &record,
                &signed,
                runtime_issuer(0x7a),
                &node_custody_secret(1),
                NOW + 1,
            ),
            Err(crate::CustodyError::NodeShareStore(
                NodeLocalShareStoreErrorV1::Unavailable
            ))
        ));

        std::fs::remove_file(temp.path().join("shares").join("caller-path-string")).unwrap();
        let temp_file = store.test_temp_path(Digest32::new([0x44; 32]));
        write_owner_only(&temp_file, b"stale temp");
        provision(&store, &record, &signed);
        assert!(!temp_file.exists());
    }

    #[test]
    fn node_share_store_debug_and_errors_are_redacted() {
        let temp = temp_root();
        let store = store(temp.path(), 1);
        let record = provisioning_record(1);
        let signed = signed_provisioning(&record, 0x7a, 0xf1, NOW, NOW + 60);
        let stored = provision(&store, &record, &signed);
        let debug = format!(
            "{store:?} {stored:?} {:?}",
            NodeLocalShareStoreErrorV1::Conflict
        );

        assert!(debug.contains("NodeLocalShareStoreV1"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(temp.path().to_str().unwrap()));
        assert!(!debug.contains("node-share-store.lock"));
        assert!(!debug.contains("shares"));
        assert!(!debug.contains(&format!("{:?}", record.sealed_share().envelope())));
        assert!(!debug.contains("share_bytes"));
        assert!(!NodeLocalShareStoreErrorV1::Conflict
            .to_string()
            .contains(&lower_hex_digest(stored.receipt().slot_hash())));
    }
}
