//! Durable acceptance of one complete signed collaboration-network profile chain.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::collaboration_default_conversation::{
    verify_default_conversation_grant, VerifiedDefaultConversationGrant,
};
use crate::collaboration_network::{
    validate_collaboration_network_profile, validate_network_id, CollaborationNetworkProfileMode,
    VerifiedCollaborationNetworkProfile, MAX_PROFILE_BYTES,
};

const ACCEPTED_HEAD_SCHEMA: &str = "elastos.collaboration-network.accepted-head/v1";
const CONFIG_DIR: &str = "collaboration/config";
const ACCEPTED_HEAD_FILE: &str = "accepted-profile-head-v1.json";
const ACCEPTED_HEAD_LOCK_FILE: &str = "accepted-profile-head-v1.lock";
pub(crate) const MAX_CHAIN_PROFILES: usize = 64;
const MAX_CHAIN_BYTES: usize = 2 * 1024 * 1024;
const MAX_ACCEPTED_HEAD_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(crate) enum CollaborationNetworkConfiguration {
    Isolated,
    Configured {
        profile: Box<VerifiedCollaborationNetworkProfile>,
        grant: Option<VerifiedDefaultConversationGrant>,
    },
}

pub(crate) struct ValidatedCollaborationNetworkConfiguration {
    profiles: Vec<VerifiedCollaborationNetworkProfile>,
    grant: Option<VerifiedDefaultConversationGrant>,
    trusted_signers_sha256: String,
}

impl ValidatedCollaborationNetworkConfiguration {
    pub(crate) fn head(&self) -> Option<&VerifiedCollaborationNetworkProfile> {
        self.profiles.last()
    }

    pub(crate) fn grant(&self) -> Option<&VerifiedDefaultConversationGrant> {
        self.grant.as_ref()
    }
}

pub(crate) struct CollaborationProfileChainLoader {
    config_dir: PathBuf,
    mutation_mutex: Mutex<()>,
    #[cfg(test)]
    directory_sync_count: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    write_fault: std::sync::atomic::AtomicU8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedProfileHead {
    schema: String,
    network_id: String,
    revision: u64,
    profile_sha256: String,
    trusted_signers_sha256: String,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteFault {
    BeforeWrite = 1,
    AfterFileSync = 2,
    AfterRename = 3,
}

impl CollaborationProfileChainLoader {
    pub(crate) fn new(data_root: &Path) -> Self {
        Self {
            config_dir: data_root.join(CONFIG_DIR),
            mutation_mutex: Mutex::new(()),
            #[cfg(test)]
            directory_sync_count: std::sync::atomic::AtomicU8::new(0),
            #[cfg(test)]
            write_fault: std::sync::atomic::AtomicU8::new(0),
        }
    }

    pub(crate) fn load_absent(&self) -> anyhow::Result<CollaborationNetworkConfiguration> {
        if self.load_accepted_head()?.is_some() {
            anyhow::bail!("configured collaboration-network profile cannot be removed");
        }
        Ok(CollaborationNetworkConfiguration::Isolated)
    }

    #[cfg(test)]
    pub(crate) fn load_configured(
        &self,
        expected_network_id: &str,
        trusted_signer_dids: &[String],
        profile_chain: &[Vec<u8>],
        grant_bytes: Option<&[u8]>,
    ) -> anyhow::Result<CollaborationNetworkConfiguration> {
        self.accept_validated(validate_collaboration_network_configuration(
            expected_network_id,
            trusted_signer_dids,
            profile_chain,
            grant_bytes,
        )?)
    }

    pub(crate) fn accept_validated(
        &self,
        validated: ValidatedCollaborationNetworkConfiguration,
    ) -> anyhow::Result<CollaborationNetworkConfiguration> {
        let head = validated
            .head()
            .context("validated collaboration profile chain is empty")?;
        let candidate = AcceptedProfileHead {
            schema: ACCEPTED_HEAD_SCHEMA.to_string(),
            network_id: head.profile().network_id.clone(),
            revision: head.profile().revision,
            profile_sha256: head.profile_sha256().to_string(),
            trusted_signers_sha256: validated.trusted_signers_sha256.clone(),
        };
        validate_accepted_head(&candidate)?;

        let _process_guard = self
            .mutation_mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration profile loader mutex is poisoned"))?;
        let created_config_dir = self.ensure_config_directory()?;
        let _file_guard = ExclusiveFileLock::acquire(&self.lock_path())?;
        let existing = self.read_accepted_head(created_config_dir)?;
        if let Some(existing) = existing.as_ref() {
            validate_accepted_head_transition(existing, &candidate, &validated.profiles)?;
        }
        if existing.as_ref() != Some(&candidate) {
            self.write_accepted_head(&candidate)?;
        }

        Ok(CollaborationNetworkConfiguration::Configured {
            profile: Box::new(head.clone()),
            grant: validated.grant,
        })
    }

    fn load_accepted_head(&self) -> anyhow::Result<Option<AcceptedProfileHead>> {
        self.read_accepted_head(false)
    }

    fn read_accepted_head(
        &self,
        allow_missing_marker: bool,
    ) -> anyhow::Result<Option<AcceptedProfileHead>> {
        if !self.validate_existing_config_ancestors()? {
            return Ok(None);
        }
        let path = self.head_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing_marker => {
                return Ok(None);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "collaboration config namespace exists without its accepted-head marker"
                );
            }
            Err(error) => return Err(error.into()),
        };
        validate_owner_only_regular_file(&path, &metadata)?;

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path).with_context(|| {
            format!(
                "failed to open collaboration profile head {}",
                path.display()
            )
        })?;
        let metadata = file.metadata()?;
        validate_owner_only_regular_file(&path, &metadata)?;
        if metadata.len() as usize > MAX_ACCEPTED_HEAD_BYTES {
            anyhow::bail!("collaboration accepted-head marker exceeds its byte limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)?;
        let head: AcceptedProfileHead =
            serde_json::from_slice(&bytes).context("invalid collaboration accepted-head marker")?;
        if canonical_head_bytes(&head)? != bytes {
            anyhow::bail!("collaboration accepted-head marker is not canonical JSON");
        }
        validate_accepted_head(&head)?;
        Ok(Some(head))
    }

    fn ensure_config_directory(&self) -> anyhow::Result<bool> {
        let data_root = self
            .config_dir
            .ancestors()
            .nth(2)
            .context("collaboration config path is not data-root derived")?;
        let metadata =
            fs::symlink_metadata(data_root).context("collaboration data root does not exist")?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!("collaboration data root must be a real directory");
        }
        let collaboration_dir = data_root.join("collaboration");
        if ensure_owner_only_directory(&collaboration_dir)? {
            sync_directory(data_root)?;
            #[cfg(test)]
            self.record_directory_sync();
        }
        let created_config_dir = ensure_owner_only_directory(&self.config_dir)?;
        if created_config_dir {
            sync_directory(&collaboration_dir)?;
            #[cfg(test)]
            self.record_directory_sync();
        }
        Ok(created_config_dir)
    }

    fn validate_existing_config_ancestors(&self) -> anyhow::Result<bool> {
        let data_root = self
            .config_dir
            .ancestors()
            .nth(2)
            .context("collaboration config path is not data-root derived")?;
        match fs::symlink_metadata(data_root) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => anyhow::bail!("collaboration data root must be a real directory"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        for path in [data_root.join("collaboration"), self.config_dir.clone()] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => validate_owner_only_directory(&path, &metadata)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(true)
    }

    fn write_accepted_head(&self, head: &AcceptedProfileHead) -> anyhow::Result<()> {
        validate_accepted_head(head)?;
        let bytes = canonical_head_bytes(head)?;
        if bytes.len() > MAX_ACCEPTED_HEAD_BYTES {
            anyhow::bail!("collaboration accepted-head marker exceeds its byte limit");
        }
        #[cfg(test)]
        let write_fault = self.take_write_fault();
        #[cfg(test)]
        if write_fault == Some(WriteFault::BeforeWrite) {
            anyhow::bail!("injected collaboration profile failure before write");
        }

        let temp_path = self
            .config_dir
            .join(format!(".{ACCEPTED_HEAD_FILE}.{}.tmp", random_hex_128()?));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut renamed = false;
        let result = (|| -> anyhow::Result<()> {
            let mut file = options.open(&temp_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            validate_owner_only_regular_file(&temp_path, &file.metadata()?)?;
            #[cfg(test)]
            if write_fault == Some(WriteFault::AfterFileSync) {
                anyhow::bail!("injected collaboration profile failure after file sync");
            }
            if let Ok(metadata) = fs::symlink_metadata(self.head_path()) {
                validate_owner_only_regular_file(&self.head_path(), &metadata)?;
            }
            fs::rename(&temp_path, self.head_path())?;
            renamed = true;
            #[cfg(test)]
            if write_fault == Some(WriteFault::AfterRename) {
                anyhow::bail!("collaboration profile durability is indeterminate after rename");
            }
            File::open(&self.config_dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() && !renamed {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    fn head_path(&self) -> PathBuf {
        self.config_dir.join(ACCEPTED_HEAD_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.config_dir.join(ACCEPTED_HEAD_LOCK_FILE)
    }

    #[cfg(test)]
    fn record_directory_sync(&self) {
        self.directory_sync_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn directory_sync_count(&self) -> u8 {
        self.directory_sync_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    fn inject_write_fault(&self, fault: WriteFault) {
        self.write_fault
            .store(fault as u8, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn take_write_fault(&self) -> Option<WriteFault> {
        match self
            .write_fault
            .swap(0, std::sync::atomic::Ordering::SeqCst)
        {
            1 => Some(WriteFault::BeforeWrite),
            2 => Some(WriteFault::AfterFileSync),
            3 => Some(WriteFault::AfterRename),
            _ => None,
        }
    }
}

pub(crate) fn validate_collaboration_network_configuration(
    expected_network_id: &str,
    trusted_signer_dids: &[String],
    profile_chain: &[Vec<u8>],
    grant_bytes: Option<&[u8]>,
) -> anyhow::Result<ValidatedCollaborationNetworkConfiguration> {
    let profiles =
        validate_complete_chain(profile_chain, expected_network_id, trusted_signer_dids)?;
    let head = profiles
        .last()
        .context("collaboration-network profile chain is empty")?;
    let grant = verify_head_grant(head, grant_bytes)?;
    Ok(ValidatedCollaborationNetworkConfiguration {
        profiles,
        grant,
        trusted_signers_sha256: trusted_signers_digest(trusted_signer_dids)?,
    })
}

fn validate_complete_chain(
    profile_chain: &[Vec<u8>],
    expected_network_id: &str,
    trusted_signer_dids: &[String],
) -> anyhow::Result<Vec<VerifiedCollaborationNetworkProfile>> {
    if profile_chain.is_empty() || profile_chain.len() > MAX_CHAIN_PROFILES {
        anyhow::bail!("collaboration-network profile chain has an invalid entry count");
    }
    let mut aggregate_bytes = 0usize;
    for profile_bytes in profile_chain {
        if profile_bytes.is_empty() || profile_bytes.len() > MAX_PROFILE_BYTES {
            anyhow::bail!("collaboration-network profile has an invalid byte length");
        }
        aggregate_bytes = aggregate_bytes
            .checked_add(profile_bytes.len())
            .context("collaboration-network profile chain byte count overflow")?;
        if aggregate_bytes > MAX_CHAIN_BYTES {
            anyhow::bail!("collaboration-network profile chain exceeds its byte limit");
        }
    }

    let mut verified = Vec::with_capacity(profile_chain.len());
    for profile_bytes in profile_chain {
        let mode = validate_collaboration_network_profile(
            Some(profile_bytes),
            expected_network_id,
            trusted_signer_dids,
            verified.last(),
        )?;
        let CollaborationNetworkProfileMode::Configured(profile) = mode else {
            anyhow::bail!("configured collaboration profile chain resolved to isolated mode");
        };
        verified.push(profile);
    }
    Ok(verified)
}

fn verify_head_grant(
    head: &VerifiedCollaborationNetworkProfile,
    grant_bytes: Option<&[u8]>,
) -> anyhow::Result<Option<VerifiedDefaultConversationGrant>> {
    match (head.profile().default_conversation.as_ref(), grant_bytes) {
        (None, None) => Ok(None),
        (None, Some(_)) => {
            anyhow::bail!("collaboration profile has no descriptor for supplied grant bytes")
        }
        (Some(_), None) => {
            anyhow::bail!("collaboration profile requires exact default-conversation grant bytes")
        }
        (Some(_), Some(grant_bytes)) => {
            Ok(Some(verify_default_conversation_grant(head, grant_bytes)?))
        }
    }
}

fn validate_accepted_head_transition(
    existing: &AcceptedProfileHead,
    candidate: &AcceptedProfileHead,
    profiles: &[VerifiedCollaborationNetworkProfile],
) -> anyhow::Result<()> {
    if existing.network_id != candidate.network_id {
        anyhow::bail!("collaboration-network change requires an explicit operator transition");
    }
    if existing.trusted_signers_sha256 != candidate.trusted_signers_sha256 {
        anyhow::bail!("collaboration trust-root change requires an explicit operator transition");
    }
    let persisted_index = usize::try_from(existing.revision.saturating_sub(1))?;
    let persisted = profiles
        .get(persisted_index)
        .context("complete profile chain does not contain the persisted accepted head")?;
    if persisted.profile().revision != existing.revision
        || persisted.profile_sha256() != existing.profile_sha256
    {
        anyhow::bail!("collaboration profile chain forks from the persisted accepted head");
    }
    if candidate.revision < existing.revision {
        anyhow::bail!("collaboration profile head revision rollback");
    }
    if candidate.revision == existing.revision && candidate != existing {
        anyhow::bail!("collaboration profile head replacement is not allowed");
    }
    Ok(())
}

fn validate_accepted_head(head: &AcceptedProfileHead) -> anyhow::Result<()> {
    if head.schema != ACCEPTED_HEAD_SCHEMA {
        anyhow::bail!("unsupported collaboration accepted-head schema");
    }
    validate_network_id(&head.network_id)?;
    if head.revision == 0 {
        anyhow::bail!("collaboration accepted-head revision is invalid");
    }
    validate_sha256_label(&head.profile_sha256, "accepted profile hash")?;
    validate_sha256_label(&head.trusted_signers_sha256, "trusted signer-set hash")?;
    Ok(())
}

fn trusted_signers_digest(trusted_signer_dids: &[String]) -> anyhow::Result<String> {
    let signers = trusted_signer_dids.iter().collect::<BTreeSet<_>>();
    if signers.len() != trusted_signer_dids.len() {
        anyhow::bail!("collaboration trusted signer set contains a duplicate DID");
    }
    let canonical = serde_json::to_vec(&signers)?;
    Ok(sha256_label(&canonical))
}

fn canonical_head_bytes(head: &AcceptedProfileHead) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(head)?)?)
}

fn validate_sha256_label(value: &str, field: &str) -> anyhow::Result<()> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if !valid {
        anyhow::bail!("{field} must be sha256:<64 lowercase hex>");
    }
    Ok(())
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn random_hex_128() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .context("OS randomness unavailable for collaboration profile state")?;
    Ok(hex::encode(bytes))
}

fn ensure_owner_only_directory(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_owner_only_directory(path, &metadata)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            let created = match builder.create(path) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error.into()),
            };
            validate_owner_only_directory(path, &fs::symlink_metadata(path)?)?;
            Ok(created)
        }
        Err(error) => Err(error.into()),
    }
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn validate_owner_only_directory(path: &Path, metadata: &fs::Metadata) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "collaboration config directory is not a real directory: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, metadata)
}

fn validate_owner_only_regular_file(path: &Path, metadata: &fs::Metadata) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "collaboration config path is not a regular file: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, metadata)
}

fn validate_owner_and_mode(path: &Path, metadata: &fs::Metadata) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            anyhow::bail!(
                "collaboration config path is not owner-only: {}",
                path.display()
            );
        }
    }
    Ok(())
}

struct ExclusiveFileLock {
    file: File,
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> anyhow::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        validate_owner_only_regular_file(path, &file.metadata()?)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(Self { file })
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use elastos_runtime::signature::{generate_keypair, SigningKey};

    use crate::collaboration_default_conversation::{
        canonical_default_conversation_grant_bytes, DefaultConversationAdmissionPolicy,
        DefaultConversationGrant, DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
    };
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes, CollaborationNetworkProfile,
        DefaultConversationGrantDescriptor, SignedCollaborationNetworkProfile,
        COLLABORATION_NETWORK_PROFILE_SCHEMA, COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };

    const NETWORK: &str = "collaboration-loader-test";
    const OTHER_NETWORK: &str = "collaboration-loader-other";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";

    struct Fixture {
        _temp: tempfile::TempDir,
        data_root: PathBuf,
        signer_a: SigningKey,
        signer_b: SigningKey,
        trusted: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data_root = temp.path().join("data");
            fs::create_dir(&data_root).unwrap();
            let (signer_a, _) = generate_keypair();
            let (signer_b, _) = generate_keypair();
            let trusted = vec![did(&signer_a), did(&signer_b)];
            Self {
                _temp: temp,
                data_root,
                signer_a,
                signer_b,
                trusted,
            }
        }

        fn loader(&self) -> CollaborationProfileChainLoader {
            CollaborationProfileChainLoader::new(&self.data_root)
        }
    }

    fn did(key: &SigningKey) -> String {
        crate::crypto::encode_did_key(&key.verifying_key())
    }

    fn grant_bytes(network_id: &str) -> Vec<u8> {
        canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: network_id.to_string(),
            conversation_id: CONVERSATION.to_string(),
            sender_service: SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        })
        .unwrap()
    }

    fn raw_sha256_cid(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, multihash).to_string()
    }

    fn append_profile(
        chain: &mut Vec<Vec<u8>>,
        signer: &SigningKey,
        network_id: &str,
        grant_cid: Option<String>,
    ) {
        let signer_did = did(signer);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: chain.len() as u64 + 1,
            previous_profile_sha256: chain.last().map(|bytes| sha256_label(bytes)),
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: grant_cid
                .map(|grant_cid| DefaultConversationGrantDescriptor { grant_cid }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            signer,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        assert_eq!(signer_did, envelope_signer);
        chain.push(
            serde_json::to_vec(
                &serde_json::to_value(SignedCollaborationNetworkProfile {
                    payload,
                    signature,
                    signer_did: envelope_signer,
                })
                .unwrap(),
            )
            .unwrap(),
        );
    }

    fn configured(
        result: CollaborationNetworkConfiguration,
    ) -> (
        VerifiedCollaborationNetworkProfile,
        Option<VerifiedDefaultConversationGrant>,
    ) {
        match result {
            CollaborationNetworkConfiguration::Configured { profile, grant } => (*profile, grant),
            CollaborationNetworkConfiguration::Isolated => panic!("expected configured mode"),
        }
    }

    fn write_owner_only(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn recursive_entries(root: &Path) -> Vec<String> {
        fn visit(root: &Path, path: &Path, entries: &mut Vec<String>) {
            let mut children = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                entries.push(
                    child
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                );
                if child.is_dir() {
                    visit(root, &child, entries);
                }
            }
        }

        let mut entries = Vec::new();
        if root.exists() {
            visit(root, root, &mut entries);
        }
        entries
    }

    #[test]
    fn typed_absent_first_run_is_isolated_and_creates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let missing_data_root = temp.path().join("missing-data-root");
        let loader = CollaborationProfileChainLoader::new(&missing_data_root);
        assert!(matches!(
            loader.load_absent().unwrap(),
            CollaborationNetworkConfiguration::Isolated
        ));
        assert!(!missing_data_root.exists());
    }

    #[test]
    fn valid_initial_restart_and_complete_successor_chain_persist_only_the_head() {
        let fixture = Fixture::new();
        let loader = fixture.loader();
        let mut chain = Vec::new();
        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);

        let (initial, initial_grant) = configured(
            loader
                .load_configured(NETWORK, &fixture.trusted, &chain, None)
                .unwrap(),
        );
        assert_eq!(initial.profile().revision, 1);
        assert!(initial_grant.is_none());
        assert_eq!(loader.directory_sync_count(), 2);
        let initial_marker = fs::read(loader.head_path()).unwrap();

        let mut reversed_trust = fixture.trusted.clone();
        reversed_trust.reverse();
        let restarted_loader = fixture.loader();
        let (restarted, _) = configured(
            restarted_loader
                .load_configured(NETWORK, &reversed_trust, &chain, None)
                .unwrap(),
        );
        assert_eq!(restarted.profile_sha256(), initial.profile_sha256());
        assert_eq!(restarted_loader.directory_sync_count(), 0);
        assert_eq!(fs::read(loader.head_path()).unwrap(), initial_marker);

        let grant = grant_bytes(NETWORK);
        append_profile(
            &mut chain,
            &fixture.signer_b,
            NETWORK,
            Some(raw_sha256_cid(&grant)),
        );
        let (updated, verified_grant) = configured(
            fixture
                .loader()
                .load_configured(NETWORK, &fixture.trusted, &chain, Some(&grant))
                .unwrap(),
        );
        assert_eq!(updated.profile().revision, 2);
        assert_eq!(
            verified_grant.unwrap().grant().conversation_id,
            CONVERSATION
        );

        let marker: AcceptedProfileHead =
            serde_json::from_slice(&fs::read(loader.head_path()).unwrap()).unwrap();
        assert_eq!(marker.network_id, NETWORK);
        assert_eq!(marker.revision, 2);
        assert_eq!(marker.profile_sha256, updated.profile_sha256());
        assert_eq!(
            marker.trusted_signers_sha256,
            trusted_signers_digest(&fixture.trusted).unwrap()
        );
        assert_eq!(
            recursive_entries(&fixture.data_root),
            vec![
                "collaboration".to_string(),
                "collaboration/config".to_string(),
                format!("collaboration/config/{ACCEPTED_HEAD_FILE}"),
                format!("collaboration/config/{ACCEPTED_HEAD_LOCK_FILE}"),
            ]
        );
    }

    #[test]
    fn incomplete_noncanonical_and_unbounded_chains_fail_before_state_creation() {
        let fixture = Fixture::new();
        let loader = fixture.loader();
        let mut valid = Vec::new();
        append_profile(&mut valid, &fixture.signer_a, NETWORK, None);
        append_profile(&mut valid, &fixture.signer_a, NETWORK, None);

        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &[], None)
            .unwrap_err()
            .to_string()
            .contains("entry count"));
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &valid[1..], None)
            .unwrap_err()
            .to_string()
            .contains("revision must be 1"));

        let mut gap = Vec::new();
        append_profile(&mut gap, &fixture.signer_a, NETWORK, None);
        let initial = gap[0].clone();
        let mut gap_envelope: SignedCollaborationNetworkProfile =
            serde_json::from_slice(&gap[0]).unwrap();
        gap_envelope.payload.revision = 3;
        gap_envelope.payload.previous_profile_sha256 = Some(sha256_label(&gap[0]));
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&gap_envelope.payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &fixture.signer_a,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        gap_envelope.signature = signature;
        gap_envelope.signer_did = signer_did;
        let gap_bytes = serde_json::to_vec(&serde_json::to_value(gap_envelope).unwrap()).unwrap();
        let gap_chain = vec![initial, gap_bytes];
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &gap_chain, None)
            .unwrap_err()
            .to_string()
            .contains("revision gap"));

        let unsigned = serde_json::to_vec(&CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: NETWORK.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: did(&fixture.signer_a),
            bootstrap_peers: Vec::new(),
            default_conversation: None,
        })
        .unwrap();
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &[unsigned], None)
            .is_err());

        let mut noncanonical = valid[0].clone();
        noncanonical.push(b'\n');
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &[noncanonical], None)
            .unwrap_err()
            .to_string()
            .contains("not canonical"));

        let too_many = vec![valid[0].clone(); MAX_CHAIN_PROFILES + 1];
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &too_many, None)
            .unwrap_err()
            .to_string()
            .contains("entry count"));
        let oversized_profile = vec![b' '; MAX_PROFILE_BYTES + 1];
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &[oversized_profile], None)
            .unwrap_err()
            .to_string()
            .contains("invalid byte length"));
        let aggregate_overflow = vec![vec![b' '; MAX_PROFILE_BYTES]; 33];
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &aggregate_overflow, None)
            .unwrap_err()
            .to_string()
            .contains("chain exceeds"));
        assert!(!loader.config_dir.exists());
    }

    #[test]
    fn persisted_head_rejects_removal_rollback_fork_network_and_trust_change() {
        let fixture = Fixture::new();
        let loader = fixture.loader();
        let mut chain = Vec::new();
        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
        loader
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap();

        let mut replacement = Vec::new();
        append_profile(&mut replacement, &fixture.signer_b, NETWORK, None);
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &replacement, None)
            .unwrap_err()
            .to_string()
            .contains("forks"));

        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
        loader
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap();
        assert!(loader
            .load_absent()
            .unwrap_err()
            .to_string()
            .contains("cannot be removed"));
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &chain[..1], None)
            .unwrap_err()
            .to_string()
            .contains("persisted accepted head"));

        let mut fork = vec![chain[0].clone()];
        append_profile(&mut fork, &fixture.signer_b, NETWORK, None);
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &fork, None)
            .unwrap_err()
            .to_string()
            .contains("forks"));

        let mut other_network = Vec::new();
        append_profile(&mut other_network, &fixture.signer_a, OTHER_NETWORK, None);
        assert!(loader
            .load_configured(OTHER_NETWORK, &fixture.trusted, &other_network, None)
            .unwrap_err()
            .to_string()
            .contains("operator transition"));

        let reduced_trust = vec![did(&fixture.signer_a)];
        assert!(loader
            .load_configured(NETWORK, &reduced_trust, &chain, None)
            .unwrap_err()
            .to_string()
            .contains("trust-root change"));
    }

    #[test]
    fn head_grant_presence_and_exact_cid_are_fail_closed() {
        let fixture = Fixture::new();
        let grant = grant_bytes(NETWORK);

        let mut absent_descriptor = Vec::new();
        append_profile(&mut absent_descriptor, &fixture.signer_a, NETWORK, None);
        assert!(fixture
            .loader()
            .load_configured(NETWORK, &fixture.trusted, &absent_descriptor, Some(&grant),)
            .unwrap_err()
            .to_string()
            .contains("no descriptor"));

        let present_fixture = Fixture::new();
        let mut descriptor = Vec::new();
        append_profile(
            &mut descriptor,
            &present_fixture.signer_a,
            NETWORK,
            Some(raw_sha256_cid(&grant)),
        );
        assert!(present_fixture
            .loader()
            .load_configured(NETWORK, &present_fixture.trusted, &descriptor, None)
            .unwrap_err()
            .to_string()
            .contains("requires exact"));
        assert!(present_fixture
            .loader()
            .load_configured(
                NETWORK,
                &present_fixture.trusted,
                &descriptor,
                Some(b"wrong"),
            )
            .unwrap_err()
            .to_string()
            .contains("CID"));
        let mut tampered = grant.clone();
        tampered.push(b'\n');
        assert!(present_fixture
            .loader()
            .load_configured(
                NETWORK,
                &present_fixture.trusted,
                &descriptor,
                Some(&tampered),
            )
            .is_err());
        let (_, verified_grant) = configured(
            present_fixture
                .loader()
                .load_configured(NETWORK, &present_fixture.trusted, &descriptor, Some(&grant))
                .unwrap(),
        );
        assert_eq!(
            verified_grant.unwrap().grant().network_id,
            NETWORK.to_string()
        );
    }

    #[test]
    fn retained_namespace_without_accepted_head_fails_closed() {
        let fixture = Fixture::new();
        let loader = fixture.loader();
        let mut chain = Vec::new();
        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
        loader
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap();
        fs::remove_file(loader.head_path()).unwrap();

        for error in [
            fixture.loader().load_absent().unwrap_err(),
            fixture
                .loader()
                .load_configured(NETWORK, &fixture.trusted, &chain, None)
                .unwrap_err(),
        ] {
            assert!(error.to_string().contains("without its accepted-head"));
        }
        assert!(loader.config_dir.exists());
        assert!(loader.lock_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn accepted_head_modes_canonical_state_and_symlink_boundaries_are_enforced() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let fixture = Fixture::new();
        let loader = fixture.loader();
        let mut chain = Vec::new();
        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
        loader
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap();

        assert_eq!(
            fs::metadata(fixture.data_root.join("collaboration"))
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&loader.config_dir).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(loader.head_path()).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(loader.lock_path()).unwrap().mode() & 0o777,
            0o600
        );

        let original = fs::read(loader.head_path()).unwrap();
        let object = serde_json::from_slice::<serde_json::Value>(&original).unwrap();
        assert_eq!(object.as_object().unwrap().len(), 5);

        let mut unknown = object.clone();
        unknown["unexpected"] = serde_json::json!(true);
        write_owner_only(&loader.head_path(), &serde_json::to_vec(&unknown).unwrap());
        assert!(loader.load_accepted_head().is_err());
        write_owner_only(&loader.head_path(), &original);

        let mut tampered_head: AcceptedProfileHead = serde_json::from_slice(&original).unwrap();
        tampered_head.profile_sha256 = format!("sha256:{}", "0".repeat(64));
        write_owner_only(
            &loader.head_path(),
            &canonical_head_bytes(&tampered_head).unwrap(),
        );
        assert!(loader
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap_err()
            .to_string()
            .contains("forks"));
        write_owner_only(&loader.head_path(), &original);

        let mut noncanonical = original.clone();
        noncanonical.push(b'\n');
        write_owner_only(&loader.head_path(), &noncanonical);
        assert!(loader
            .load_accepted_head()
            .unwrap_err()
            .to_string()
            .contains("not canonical"));
        write_owner_only(&loader.head_path(), &original);

        fs::set_permissions(loader.head_path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(loader
            .load_accepted_head()
            .unwrap_err()
            .to_string()
            .contains("owner-only"));

        let directory_mode_fixture = Fixture::new();
        let directory_mode_loader = directory_mode_fixture.loader();
        directory_mode_loader.ensure_config_directory().unwrap();
        fs::set_permissions(
            &directory_mode_loader.config_dir,
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(directory_mode_loader
            .load_accepted_head()
            .unwrap_err()
            .to_string()
            .contains("owner-only"));

        let lock_mode_fixture = Fixture::new();
        let lock_mode_loader = lock_mode_fixture.loader();
        lock_mode_loader.ensure_config_directory().unwrap();
        write_owner_only(&lock_mode_loader.lock_path(), b"");
        fs::set_permissions(
            lock_mode_loader.lock_path(),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let mut lock_mode_chain = Vec::new();
        append_profile(
            &mut lock_mode_chain,
            &lock_mode_fixture.signer_a,
            NETWORK,
            None,
        );
        assert!(lock_mode_loader
            .load_configured(NETWORK, &lock_mode_fixture.trusted, &lock_mode_chain, None,)
            .unwrap_err()
            .to_string()
            .contains("owner-only"));

        let symlink_fixture = Fixture::new();
        let symlink_loader = symlink_fixture.loader();
        let external = symlink_fixture._temp.path().join("external-config");
        fs::create_dir(&external).unwrap();
        fs::set_permissions(&external, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&external, symlink_fixture.data_root.join("collaboration")).unwrap();
        assert!(symlink_loader.load_accepted_head().is_err());

        let state_symlink_fixture = Fixture::new();
        let state_symlink_loader = state_symlink_fixture.loader();
        let mut state_chain = Vec::new();
        append_profile(
            &mut state_chain,
            &state_symlink_fixture.signer_a,
            NETWORK,
            None,
        );
        state_symlink_loader
            .load_configured(NETWORK, &state_symlink_fixture.trusted, &state_chain, None)
            .unwrap();
        let external_state = state_symlink_fixture._temp.path().join("external-head");
        fs::rename(state_symlink_loader.head_path(), &external_state).unwrap();
        symlink(&external_state, state_symlink_loader.head_path()).unwrap();
        assert!(state_symlink_loader.load_accepted_head().is_err());

        let lock_symlink_fixture = Fixture::new();
        let lock_symlink_loader = lock_symlink_fixture.loader();
        lock_symlink_loader.ensure_config_directory().unwrap();
        let external_lock = lock_symlink_fixture._temp.path().join("external-lock");
        write_owner_only(&external_lock, b"");
        symlink(&external_lock, lock_symlink_loader.lock_path()).unwrap();
        let mut lock_chain = Vec::new();
        append_profile(
            &mut lock_chain,
            &lock_symlink_fixture.signer_a,
            NETWORK,
            None,
        );
        assert!(lock_symlink_loader
            .load_configured(NETWORK, &lock_symlink_fixture.trusted, &lock_chain, None,)
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_lock_and_each_write_failure_boundary_preserve_exact_restart_state() {
        for fault in [
            WriteFault::BeforeWrite,
            WriteFault::AfterFileSync,
            WriteFault::AfterRename,
        ] {
            let fixture = Fixture::new();
            let loader = fixture.loader();
            let mut chain = Vec::new();
            append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
            loader.inject_write_fault(fault);
            assert!(loader
                .load_configured(NETWORK, &fixture.trusted, &chain, None)
                .is_err());
            if fault == WriteFault::AfterRename {
                let persisted = fs::read(loader.head_path()).unwrap();
                configured(
                    fixture
                        .loader()
                        .load_configured(NETWORK, &fixture.trusted, &chain, None)
                        .unwrap(),
                );
                assert_eq!(fs::read(loader.head_path()).unwrap(), persisted);
            } else {
                assert!(!loader.head_path().exists());
                assert!(loader.config_dir.exists());
                assert!(loader.lock_path().exists());
                assert!(fixture
                    .loader()
                    .load_absent()
                    .unwrap_err()
                    .to_string()
                    .contains("without its accepted-head"));
                assert!(fixture
                    .loader()
                    .load_configured(NETWORK, &fixture.trusted, &chain, None)
                    .unwrap_err()
                    .to_string()
                    .contains("without its accepted-head"));
            }
            assert!(fs::read_dir(&loader.config_dir).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        }

        let fixture = Fixture::new();
        let first = Arc::new(fixture.loader());
        let mut chain = Vec::new();
        append_profile(&mut chain, &fixture.signer_a, NETWORK, None);
        first
            .load_configured(NETWORK, &fixture.trusted, &chain, None)
            .unwrap();
        let guard = ExclusiveFileLock::acquire(&first.lock_path()).unwrap();
        let second = Arc::new(fixture.loader());
        let trusted = fixture.trusted.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(second.load_configured(NETWORK, &trusted, &chain, None))
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(guard);
        configured(
            done_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
        );
        worker.join().unwrap();
        assert!(first.load_accepted_head().unwrap().is_some());
    }
}
