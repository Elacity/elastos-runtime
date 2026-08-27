//! Typed presence projection for the verified default collaboration conversation.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    canonical_signed_collaboration_message_bytes, MAX_COLLABORATION_CLOCK_SKEW_SECS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::collaboration_core::{
    ensure_owner_only_directory, random_hex_128, validate_owner_only_directory,
    validate_owner_only_regular_file, CollaborationCore, DurableOutgoingMessage, ExclusiveFileLock,
    PendingProductHandoff, DEFAULT_CONVERSATION_SEND_METHOD,
};
use crate::collaboration_product::{CHAT_INTERFACE, CHAT_ROOM_CAPSULE, CHAT_SERVICE};
use crate::esp_binding::{esp_request_binding, EspRequestBinding};

const PRESENCE_PAYLOAD_TYPE: &str = "elastos.chat.presence/v1";
const PRESENCE_RESOURCE: &str = "elastos://chat/presence";
const PRESENCE_TTL_SECS: u64 = 45;
const PRESENCE_STATE_SCHEMA: &str = "elastos.chat.presence-state/v1";
const PRESENCE_STATE_PARENT: &str = "collaboration/default-conversation-presence";
const PRESENCE_STATE_FILE: &str = "presence-v1.json";
const PRESENCE_LOCK_FILE: &str = "presence-v1.lock";
const PRESENCE_NAMESPACE_DOMAIN: &[u8] = b"elastos.chat.presence-state.v1";
const MAX_PRESENCE_RECORDS: usize = 256;
// Record count and bytes are independent bounds. Three KiB per slot admits the
// normal signed Profile envelope while oversized valid Profiles exhaust the
// byte budget before they exhaust record capacity.
const MAX_PRESENCE_STATE_BYTES: usize = MAX_PRESENCE_RECORDS * 3 * 1024;

#[derive(Clone)]
pub struct CollaborationPresenceProductPort {
    core: Arc<CollaborationCore>,
    read_model: Arc<PresenceReadModel>,
}

#[derive(Clone)]
pub struct PreparedCollaborationPresence {
    core: Arc<CollaborationCore>,
    envelope_bytes: Vec<u8>,
    envelope_sha256: String,
    sender_profile_did: String,
    display_name: String,
    handle: Option<String>,
    issued_at: u64,
    expires_at: u64,
}

pub struct CollaborationPresenceHandoff {
    core: Arc<CollaborationCore>,
    envelope_bytes: Vec<u8>,
    envelope_sha256: String,
    sender_profile_did: String,
    display_name: String,
    handle: Option<String>,
    issued_at: u64,
    expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollaborationPresenceProjectionOutcome {
    Applied,
    Unchanged,
    Superseded,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationPresenceSnapshot {
    records: Vec<CollaborationPresenceSnapshotRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationPresenceSnapshotRecord {
    sender_profile_did: String,
    display_name: String,
    handle: Option<String>,
    last_seen_at: u64,
    expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresencePayload {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceState {
    schema: String,
    binding: PresenceStateBinding,
    records: Vec<PresenceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceStateBinding {
    network_id: String,
    conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceRecord {
    envelope: String,
}

struct PresenceReadModel {
    core: Arc<CollaborationCore>,
    state_dir: PathBuf,
    mutation_mutex: Mutex<()>,
}

struct VerifiedPresence {
    envelope_bytes: Vec<u8>,
    envelope_sha256: String,
    sender_profile_did: String,
    display_name: String,
    handle: Option<String>,
    issued_at: u64,
    expires_at: u64,
}

impl CollaborationPresenceProductPort {
    pub(crate) fn new(core: Arc<CollaborationCore>) -> anyhow::Result<Self> {
        if core.sender_service() != CHAT_SERVICE {
            anyhow::bail!("default collaboration conversation is not owned by Chat");
        }
        let read_model = Arc::new(PresenceReadModel::new(core.clone()));
        Ok(Self { core, read_model })
    }

    #[cfg(test)]
    pub(crate) fn test_shares_core_with(&self, core: &Arc<CollaborationCore>) -> bool {
        Arc::ptr_eq(&self.core, core)
    }

    #[cfg(test)]
    pub(crate) fn test_core(&self) -> &Arc<CollaborationCore> {
        &self.core
    }

    pub(crate) fn prepare_presence(
        &self,
        operation: EspRequestBinding,
        profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<PreparedCollaborationPresence> {
        let payload = PresencePayload {};
        let expected =
            presence_request_binding(&operation.request_id, &operation.principal, profile)?;
        if operation != expected {
            anyhow::bail!("Chat presence request binding is invalid");
        }
        let prepared = self.core.prepare_profile_outgoing(
            operation,
            profile,
            PRESENCE_PAYLOAD_TYPE,
            serde_json::to_value(payload)?,
            now,
            PRESENCE_TTL_SECS,
        )?;
        prepared_presence(self.core.clone(), &prepared)
    }

    pub(crate) fn pending_outgoing_presences(
        &self,
        now: u64,
    ) -> anyhow::Result<Vec<PreparedCollaborationPresence>> {
        Ok(self
            .core
            .pending_outgoing_product_projections(now)?
            .iter()
            .filter_map(|pending| prepared_presence(self.core.clone(), pending.outgoing()).ok())
            .collect())
    }

    pub fn project_prepared_presence(
        &self,
        prepared: &PreparedCollaborationPresence,
        now: u64,
    ) -> anyhow::Result<CollaborationPresenceProjectionOutcome> {
        self.require_prepared(prepared)?;
        let outcome = self.read_model.project(&prepared.envelope_bytes, now)?;
        self.core
            .acknowledge_outgoing_product_projection(&prepared.envelope_sha256)?;
        Ok(outcome)
    }

    pub fn pending_presences(&self) -> anyhow::Result<Vec<CollaborationPresenceHandoff>> {
        Ok(self
            .core
            .pending_product_handoffs()?
            .into_iter()
            .filter_map(|pending| presence_handoff(self.core.clone(), pending))
            .collect())
    }

    pub fn project_handoff(
        &self,
        handoff: &CollaborationPresenceHandoff,
        now: u64,
    ) -> anyhow::Result<CollaborationPresenceProjectionOutcome> {
        self.require_handoff(handoff)?;
        let outcome = self.read_model.project(&handoff.envelope_bytes, now)?;
        self.core
            .acknowledge_product_handoff(&handoff.envelope_sha256)?;
        Ok(outcome)
    }

    pub fn snapshot(&self, now: u64) -> anyhow::Result<CollaborationPresenceSnapshot> {
        self.read_model.snapshot(now)
    }

    fn require_prepared(&self, prepared: &PreparedCollaborationPresence) -> anyhow::Result<()> {
        if !Arc::ptr_eq(&self.core, &prepared.core) {
            anyhow::bail!("prepared presence belongs to another collaboration product port");
        }
        Ok(())
    }

    fn require_handoff(&self, handoff: &CollaborationPresenceHandoff) -> anyhow::Result<()> {
        if !Arc::ptr_eq(&self.core, &handoff.core) {
            anyhow::bail!("presence handoff belongs to another collaboration product port");
        }
        Ok(())
    }
}

pub(crate) fn presence_request_binding(
    request_id: &str,
    principal: &str,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) -> anyhow::Result<EspRequestBinding> {
    let payload =
        crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
            profile,
            serde_json::json!({}),
        )?;
    let intent = serde_json::json!({
        "payload_type": PRESENCE_PAYLOAD_TYPE,
        "payload": payload,
        "ttl_secs": PRESENCE_TTL_SECS,
    });
    Ok(esp_request_binding(
        request_id,
        principal,
        CHAT_ROOM_CAPSULE,
        Some(CHAT_INTERFACE),
        DEFAULT_CONVERSATION_SEND_METHOD,
        [PRESENCE_RESOURCE.to_string()],
        &intent,
    ))
}

impl CollaborationPresenceSnapshot {
    pub fn records(&self) -> &[CollaborationPresenceSnapshotRecord] {
        &self.records
    }

    #[cfg(test)]
    pub(crate) fn for_test(records: Vec<CollaborationPresenceSnapshotRecord>) -> Self {
        Self { records }
    }
}

impl CollaborationPresenceSnapshotRecord {
    #[cfg(test)]
    pub(crate) fn for_test(sender_profile_did: &str, last_seen_at: u64, expires_at: u64) -> Self {
        Self {
            sender_profile_did: sender_profile_did.to_string(),
            display_name: "Remote".to_string(),
            handle: None,
            last_seen_at,
            expires_at,
        }
    }

    pub fn sender_profile_did(&self) -> &str {
        &self.sender_profile_did
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    pub fn last_seen_at(&self) -> u64 {
        self.last_seen_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl PreparedCollaborationPresence {
    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }

    pub fn sender_profile_did(&self) -> &str {
        &self.sender_profile_did
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    #[cfg(test)]
    pub(crate) fn test_envelope_bytes(&self) -> &[u8] {
        &self.envelope_bytes
    }
}

impl CollaborationPresenceHandoff {
    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }

    pub fn sender_profile_did(&self) -> &str {
        &self.sender_profile_did
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl PresenceReadModel {
    fn new(core: Arc<CollaborationCore>) -> Self {
        let (network_id, conversation_id) = core.conversation_scope();
        let namespace = presence_namespace(network_id, conversation_id);
        let state_dir = core
            .product_data_root()
            .join(PRESENCE_STATE_PARENT)
            .join(namespace);
        Self {
            core,
            state_dir,
            mutation_mutex: Mutex::new(()),
        }
    }

    fn snapshot(&self, now: u64) -> anyhow::Result<CollaborationPresenceSnapshot> {
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let mut records = state
            .records
            .iter()
            .map(|record| self.verify_record(record))
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .filter(|record| {
                record.expires_at > now
                    && record.issued_at <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
            })
            .map(|record| CollaborationPresenceSnapshotRecord {
                sender_profile_did: record.sender_profile_did,
                display_name: record.display_name,
                handle: record.handle,
                last_seen_at: record.issued_at,
                expires_at: record.expires_at,
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.sender_profile_did.cmp(&right.sender_profile_did));
        Ok(CollaborationPresenceSnapshot { records })
    }

    fn project(
        &self,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<CollaborationPresenceProjectionOutcome> {
        let incoming = self.verify_envelope(envelope_bytes)?;
        if incoming.issued_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) {
            anyhow::bail!("presence envelope is future-dated beyond the allowed clock skew");
        }
        if incoming.expires_at <= now {
            let _ = self.load_state()?;
            return Ok(CollaborationPresenceProjectionOutcome::Expired);
        }
        let envelope = String::from_utf8(envelope_bytes.to_vec())
            .context("canonical presence envelope is not UTF-8")?;

        let _in_process = self
            .mutation_mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("presence mutation lock is poisoned"))?;
        self.ensure_state_directory()?;
        let _file_lock = ExclusiveFileLock::acquire(&self.lock_path())?;
        let mut state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let before = state.records.len();
        state.records.retain(|record| {
            self.verify_record(record)
                .map(|existing| existing.expires_at > now)
                .unwrap_or(true)
        });
        let pruned = state.records.len() != before;

        let existing = state.records.iter().position(|record| {
            self.verify_record(record)
                .map(|existing| existing.sender_profile_did == incoming.sender_profile_did)
                .unwrap_or(false)
        });
        let (changed, outcome) = if let Some(index) = existing {
            let current = self.verify_record(&state.records[index])?;
            if state.records[index].envelope.as_bytes() == envelope_bytes {
                (false, CollaborationPresenceProjectionOutcome::Unchanged)
            } else if incoming.issued_at <= current.issued_at {
                (false, CollaborationPresenceProjectionOutcome::Superseded)
            } else {
                state.records[index] = PresenceRecord { envelope };
                (true, CollaborationPresenceProjectionOutcome::Applied)
            }
        } else {
            if state.records.len() >= MAX_PRESENCE_RECORDS {
                anyhow::bail!("presence read model record capacity is exhausted");
            }
            state.records.push(PresenceRecord { envelope });
            (true, CollaborationPresenceProjectionOutcome::Applied)
        };

        if changed || pruned {
            let mut keyed = state
                .records
                .drain(..)
                .map(|record| {
                    let sender = self.verify_record(&record)?.sender_profile_did;
                    Ok((sender, record))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            keyed.sort_by(|left, right| left.0.cmp(&right.0));
            state.records = keyed.into_iter().map(|(_, record)| record).collect();
            self.validate_state(&state)?;
            self.write_state(&state)?;
        }
        Ok(outcome)
    }

    fn verify_envelope(&self, envelope_bytes: &[u8]) -> anyhow::Result<VerifiedPresence> {
        let authorized = self.core.authorize_stored_product_message(envelope_bytes)?;
        let message = authorized.message().envelope();
        if message.payload.payload_type != PRESENCE_PAYLOAD_TYPE {
            anyhow::bail!("collaboration payload is not Chat presence");
        }
        presence_from_authorized(envelope_bytes, &authorized)
    }

    fn verify_record(&self, record: &PresenceRecord) -> anyhow::Result<VerifiedPresence> {
        self.verify_envelope(record.envelope.as_bytes())
    }

    fn load_state(&self) -> anyhow::Result<Option<PresenceState>> {
        if !self.validate_existing_ancestors()? {
            return Ok(None);
        }
        let path = self.state_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        validate_owner_only_regular_file(&path, &metadata)?;
        if metadata.len() as usize > MAX_PRESENCE_STATE_BYTES {
            anyhow::bail!("presence read model exceeds its byte limit");
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path)?;
        validate_owner_only_regular_file(&path, &file.metadata()?)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PRESENCE_STATE_BYTES {
            anyhow::bail!("presence read model exceeds its byte limit");
        }
        let state: PresenceState =
            serde_json::from_slice(&bytes).context("invalid presence read model")?;
        if canonical_state_bytes(&state)? != bytes {
            anyhow::bail!("presence read model is not canonical JSON");
        }
        self.validate_state(&state)?;
        Ok(Some(state))
    }

    fn validate_existing_ancestors(&self) -> anyhow::Result<bool> {
        let data_root = self.core.product_data_root();
        match fs::symlink_metadata(data_root) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => anyhow::bail!("presence data root must be a real directory"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        for path in [
            data_root.join("collaboration"),
            data_root.join(PRESENCE_STATE_PARENT),
            self.state_dir.clone(),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => validate_owner_only_directory(&path, &metadata)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(true)
    }

    fn validate_state(&self, state: &PresenceState) -> anyhow::Result<()> {
        if state.schema != PRESENCE_STATE_SCHEMA || state.binding != self.state_binding() {
            anyhow::bail!("presence read model binding or schema mismatch");
        }
        if state.records.len() > MAX_PRESENCE_RECORDS
            || canonical_state_bytes(state)?.len() > MAX_PRESENCE_STATE_BYTES
        {
            anyhow::bail!("presence read model exceeds its bounds");
        }
        let mut senders = HashSet::new();
        for record in &state.records {
            let presence = self.verify_record(record)?;
            if !senders.insert(presence.sender_profile_did) {
                anyhow::bail!("presence read model has duplicate Profile records");
            }
        }
        Ok(())
    }

    fn empty_state(&self) -> PresenceState {
        PresenceState {
            schema: PRESENCE_STATE_SCHEMA.to_string(),
            binding: self.state_binding(),
            records: Vec::new(),
        }
    }

    fn state_binding(&self) -> PresenceStateBinding {
        let (network_id, conversation_id) = self.core.conversation_scope();
        PresenceStateBinding {
            network_id: network_id.to_string(),
            conversation_id: conversation_id.to_string(),
        }
    }

    fn ensure_state_directory(&self) -> anyhow::Result<()> {
        let data_root = self.core.product_data_root();
        let root = fs::symlink_metadata(data_root).context("presence data root does not exist")?;
        if root.file_type().is_symlink() || !root.is_dir() {
            anyhow::bail!("presence data root must be a real directory");
        }
        ensure_owner_only_directory(&data_root.join("collaboration"))?;
        ensure_owner_only_directory(&data_root.join(PRESENCE_STATE_PARENT))?;
        ensure_owner_only_directory(&self.state_dir)
    }

    fn state_path(&self) -> PathBuf {
        self.state_dir.join(PRESENCE_STATE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.state_dir.join(PRESENCE_LOCK_FILE)
    }

    fn write_state(&self, state: &PresenceState) -> anyhow::Result<()> {
        let bytes = canonical_state_bytes(state)?;
        if bytes.len() > MAX_PRESENCE_STATE_BYTES {
            anyhow::bail!("presence read model exceeds its byte limit");
        }
        let temp = self
            .state_dir
            .join(format!(".{PRESENCE_STATE_FILE}.{}.tmp", random_hex_128()?));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut renamed = false;
        let result = (|| -> anyhow::Result<()> {
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            validate_owner_only_regular_file(&temp, &file.metadata()?)?;
            if let Ok(metadata) = fs::symlink_metadata(self.state_path()) {
                validate_owner_only_regular_file(&self.state_path(), &metadata)?;
            }
            fs::rename(&temp, self.state_path())?;
            renamed = true;
            File::open(&self.state_dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() && !renamed {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn prepared_presence(
    core: Arc<CollaborationCore>,
    prepared: &DurableOutgoingMessage,
) -> anyhow::Result<PreparedCollaborationPresence> {
    let authorized = core.authorize_stored_product_message(prepared.envelope_bytes())?;
    let presence = presence_from_authorized(prepared.envelope_bytes(), &authorized)?;
    Ok(PreparedCollaborationPresence {
        core,
        envelope_bytes: presence.envelope_bytes,
        envelope_sha256: presence.envelope_sha256,
        sender_profile_did: presence.sender_profile_did,
        display_name: presence.display_name,
        handle: presence.handle,
        issued_at: presence.issued_at,
        expires_at: presence.expires_at,
    })
}

fn presence_handoff(
    core: Arc<CollaborationCore>,
    pending: PendingProductHandoff,
) -> Option<CollaborationPresenceHandoff> {
    let authorized = pending.authorized_message();
    let message = authorized.message();
    let envelope_bytes = canonical_signed_collaboration_message_bytes(message.envelope()).ok()?;
    let presence = presence_from_authorized(&envelope_bytes, authorized).ok()?;
    Some(CollaborationPresenceHandoff {
        core,
        envelope_bytes: presence.envelope_bytes,
        envelope_sha256: presence.envelope_sha256,
        sender_profile_did: presence.sender_profile_did,
        display_name: presence.display_name,
        handle: presence.handle,
        issued_at: presence.issued_at,
        expires_at: presence.expires_at,
    })
}

fn presence_from_authorized(
    envelope_bytes: &[u8],
    authorized: &crate::collaboration_default_conversation::AuthorizedDefaultConversationMessage,
) -> anyhow::Result<VerifiedPresence> {
    let message = &authorized.message().envelope().payload;
    if message.payload_type != PRESENCE_PAYLOAD_TYPE {
        anyhow::bail!("collaboration payload is not Chat presence");
    }
    exact_presence_payload(authorized.product_payload())?;
    let profile = authorized.sender_profile();
    Ok(VerifiedPresence {
        envelope_bytes: envelope_bytes.to_vec(),
        envelope_sha256: authorized.message().envelope_sha256().to_string(),
        sender_profile_did: profile.document().profile_did.clone(),
        display_name: profile.document().display_name.clone(),
        handle: profile.document().handle.clone(),
        issued_at: message.created_at,
        expires_at: message.expires_at,
    })
}

fn exact_presence_payload(value: &serde_json::Value) -> anyhow::Result<PresencePayload> {
    let payload: PresencePayload =
        serde_json::from_value(value.clone()).context("Chat presence payload is malformed")?;
    if serde_json::to_value(&payload)? != *value {
        anyhow::bail!("Chat presence payload is not canonical");
    }
    Ok(payload)
}

fn presence_namespace(network_id: &str, conversation_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(PRESENCE_NAMESPACE_DOMAIN);
    digest.update([0]);
    digest.update(network_id.as_bytes());
    digest.update([0]);
    digest.update(conversation_id.as_bytes());
    hex::encode(digest.finalize())
}

fn canonical_state_bytes(state: &PresenceState) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(state)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;

    use elastos_common::collaboration_protocol::SignedCollaborationMessage;
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    use crate::collaboration_default_conversation::{
        canonical_default_conversation_grant_bytes, verify_default_conversation_grant,
        DefaultConversationAdmissionPolicy, DefaultConversationGrant,
        VerifiedDefaultConversationGrant, DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
    };
    use crate::collaboration_device_authority::DefaultConversationDeviceAuthority;
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, DefaultConversationGrantDescriptor,
        SignedCollaborationNetworkProfile, VerifiedCollaborationNetworkProfile,
        COLLABORATION_NETWORK_PROFILE_SCHEMA, COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_product::CollaborationChatProductPort;

    const NETWORK: &str = "presence-product-test";
    const CONVERSATION: &str = "default-conversation";
    const NOW: u64 = 1_800_000_000;

    struct Fixture {
        data_root: PathBuf,
        device_key: SigningKey,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
        person_profile:
            crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        core: Arc<CollaborationCore>,
        chat: CollaborationChatProductPort,
        presence: CollaborationPresenceProductPort,
    }

    fn fixture_at(data_root: &Path) -> Fixture {
        fs::create_dir_all(data_root).unwrap();
        let grant_bytes = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: CONVERSATION.to_string(),
            sender_service: CHAT_SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        })
        .unwrap();
        let digest = Sha256::digest(&grant_bytes);
        let grant_cid = cid::Cid::new_v1(
            0x55,
            cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap(),
        )
        .to_string();
        let (profile_signer, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&profile_signer.verifying_key());
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: NETWORK.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            &profile_signer,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let profile_bytes = serde_json::to_vec(
            &serde_json::to_value(SignedCollaborationNetworkProfile {
                payload,
                signature,
                signer_did: envelope_signer,
            })
            .unwrap(),
        )
        .unwrap();
        let CollaborationNetworkProfileMode::Configured(profile) =
            validate_collaboration_network_profile(
                Some(&profile_bytes),
                NETWORK,
                &[signer_did],
                None,
            )
            .unwrap()
        else {
            panic!("test profile must be configured");
        };
        let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
        let (device_key, _) = generate_keypair();
        let device_did = crate::crypto::encode_did_key(&device_key.verifying_key());
        let (person_key, _) = generate_keypair();
        let person_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &person_key,
                "Alice",
                Some("alice"),
                1,
                None,
                NOW,
                vec![device_did],
            )
            .unwrap();
        let core = Arc::new(
            CollaborationCore::new(
                data_root,
                device_key.clone(),
                profile.clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let chat = CollaborationChatProductPort::new(core.clone()).unwrap();
        let presence = CollaborationPresenceProductPort::new(core.clone()).unwrap();
        Fixture {
            data_root: data_root.to_path_buf(),
            device_key,
            profile,
            grant,
            person_profile,
            core,
            chat,
            presence,
        }
    }

    fn remote_authority(
        fixture: &Fixture,
        display_name: &str,
        handle: Option<&str>,
    ) -> (
        DefaultConversationDeviceAuthority,
        crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    ) {
        let (key, _) = generate_keypair();
        let device_did = crate::crypto::encode_did_key(&key.verifying_key());
        let (person_key, _) = generate_keypair();
        let person_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &person_key,
                display_name,
                handle,
                1,
                None,
                NOW,
                vec![device_did],
            )
            .unwrap();
        let authority = DefaultConversationDeviceAuthority::new(
            key,
            fixture.profile.clone(),
            fixture.grant.clone(),
        )
        .unwrap();
        (authority, person_profile)
    }

    fn remote_presence(
        fixture: &Fixture,
        display_name: &str,
        handle: Option<&str>,
        now: u64,
    ) -> crate::collaboration_device_authority::PreparedDefaultConversationMessage {
        let (remote, remote_profile) = remote_authority(fixture, display_name, handle);
        remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                now,
                PRESENCE_TTL_SECS,
            )
            .unwrap()
    }

    fn binding(fixture: &Fixture, request_id: &str) -> EspRequestBinding {
        presence_request_binding(request_id, "runtime-principal", &fixture.person_profile).unwrap()
    }

    #[test]
    fn presence_authority_binding_and_explicit_nested_root_are_exact() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("unusual/deep/runtime-data-root");
        let fixture = fixture_at(&nested);
        assert!(fixture
            .presence
            .test_shares_core_with(fixture.chat.test_core()));
        assert_eq!(fixture.core.product_data_root(), nested);
        assert!(fixture.presence.snapshot(NOW).unwrap().records().is_empty());
        assert!(!nested.join("collaboration").exists());

        let prepared = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "presence-one"),
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        assert_eq!(prepared.display_name(), "Alice");
        assert_eq!(prepared.handle(), Some("alice"));
        assert_eq!(prepared.issued_at(), NOW);
        assert_eq!(prepared.expires_at(), NOW + PRESENCE_TTL_SECS);
        assert_eq!(
            prepared.sender_profile_did(),
            fixture.person_profile.document().profile_did
        );
        let signed: SignedCollaborationMessage =
            serde_json::from_slice(&prepared.envelope_bytes).unwrap();
        assert_eq!(signed.payload.network_id, NETWORK);
        assert_eq!(signed.payload.conversation_id, CONVERSATION);
        assert_eq!(signed.payload.sender_service, CHAT_SERVICE);
        assert_eq!(signed.payload.payload_type, PRESENCE_PAYLOAD_TYPE);
        assert_eq!(
            signed.payload.expires_at - signed.payload.created_at,
            PRESENCE_TTL_SECS
        );
        assert!(signed.payload.payload.get("network_id").is_none());
        assert!(signed.payload.payload.get("conversation_id").is_none());
        assert!(signed.payload.payload.get("sender_profile_did").is_none());

        let replay = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "presence-one"),
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        assert_eq!(replay.envelope_sha256(), prepared.envelope_sha256());
        let (alternate_profile_key, _) = generate_keypair();
        let alternate_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &alternate_profile_key,
                "Changed",
                Some("alice"),
                1,
                None,
                NOW,
                vec![crate::crypto::encode_did_key(
                    &fixture.device_key.verifying_key(),
                )],
            )
            .unwrap();
        assert!(fixture
            .presence
            .prepare_presence(binding(&fixture, "presence-one"), &alternate_profile, NOW,)
            .is_err());
        let mut wrong = binding(&fixture, "wrong-context");
        wrong.resources = vec!["elastos://chat/message".to_string()];
        assert!(fixture
            .presence
            .prepare_presence(wrong, &fixture.person_profile, NOW)
            .is_err());

        let other_temp = tempfile::tempdir().unwrap();
        let other = fixture_at(other_temp.path());
        assert!(other
            .presence
            .project_prepared_presence(&prepared, NOW)
            .is_err());
        assert!(other.presence.snapshot(NOW).unwrap().records().is_empty());
    }

    #[test]
    fn chat_presence_and_unknown_handoffs_coexist_without_cross_consumption() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = fixture_at(temp.path());
        let (remote, remote_profile) = remote_authority(&fixture, "Remote", Some("remote"));
        let presence = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW,
                PRESENCE_TTL_SECS,
            )
            .unwrap();
        let chat = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"hello"}),
                NOW + 1,
                300,
            )
            .unwrap();
        assert!(remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                "elastos.chat.future/v1",
                serde_json::json!({"value":true}),
                NOW + 2,
                300,
            )
            .is_err());
        let malformed_presence = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({"extra":true}),
                NOW + 3,
                PRESENCE_TTL_SECS,
            )
            .unwrap();
        for message in [&presence, &chat, &malformed_presence] {
            fixture
                .core
                .accept_incoming_from_signed_source_for_test(message.envelope_bytes(), NOW + 4)
                .unwrap();
        }
        assert_eq!(fixture.chat.pending_messages().unwrap().len(), 1);
        assert_eq!(fixture.presence.pending_presences().unwrap().len(), 1);
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 3);

        let other_temp = tempfile::tempdir().unwrap();
        let other = fixture_at(other_temp.path());
        let handoffs = fixture.presence.pending_presences().unwrap();
        assert!(other
            .presence
            .project_handoff(&handoffs[0], NOW + 4)
            .is_err());
        fixture
            .presence
            .project_handoff(&handoffs[0], NOW + 4)
            .unwrap();
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 2);
        assert_eq!(fixture.chat.pending_messages().unwrap().len(), 1);

        let chat_handoffs = fixture.chat.pending_messages().unwrap();
        fixture
            .chat
            .project_handoff(&fixture.data_root, &chat_handoffs[0])
            .unwrap();
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 1);
        assert!(fixture.chat.pending_messages().unwrap().is_empty());
        assert!(fixture.presence.pending_presences().unwrap().is_empty());
        let snapshot = fixture.presence.snapshot(NOW + 4).unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(snapshot.records()[0].display_name(), "Remote");
        assert_eq!(snapshot.records()[0].handle(), Some("remote"));
        assert_eq!(snapshot.records()[0].last_seen_at(), NOW);

        let restarted_core = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_presence =
            CollaborationPresenceProductPort::new(restarted_core.clone()).unwrap();
        assert_eq!(restarted_presence.snapshot(NOW + 4).unwrap(), snapshot);
        assert_eq!(
            restarted_core.summary().unwrap().pending_product_handoffs,
            1
        );
    }

    #[test]
    fn local_presence_projection_is_restart_safe_reordered_safe_and_expires_without_leave() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = fixture_at(temp.path());
        let first = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "local-first"),
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        assert!(fixture
            .chat
            .pending_outgoing_messages(NOW)
            .unwrap()
            .is_empty());
        assert_eq!(
            fixture
                .presence
                .pending_outgoing_presences(NOW)
                .unwrap()
                .len(),
            1
        );
        assert!(fixture.core.pending_outgoing(NOW).unwrap().is_empty());
        fixture
            .presence
            .project_prepared_presence(&first, NOW)
            .unwrap();
        fixture
            .presence
            .project_prepared_presence(&first, NOW)
            .unwrap();
        assert_eq!(fixture.core.pending_outgoing(NOW).unwrap().len(), 1);

        let newer = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "local-newer"),
                &fixture.person_profile,
                NOW + 10,
            )
            .unwrap();
        fixture
            .presence
            .project_prepared_presence(&newer, NOW + 10)
            .unwrap();
        let before_reordered = fs::read(fixture.presence.read_model.state_path()).unwrap();
        let stale = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "local-stale"),
                &fixture.person_profile,
                NOW + 5,
            )
            .unwrap();
        assert_eq!(
            fixture
                .presence
                .project_prepared_presence(&stale, NOW + 10)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Superseded
        );
        let equal_conflict = fixture
            .presence
            .prepare_presence(
                binding(&fixture, "local-equal"),
                &fixture.person_profile,
                NOW + 10,
            )
            .unwrap();
        assert_eq!(
            fixture
                .presence
                .project_prepared_presence(&equal_conflict, NOW + 10)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Superseded
        );
        assert_eq!(
            fs::read(fixture.presence.read_model.state_path()).unwrap(),
            before_reordered
        );
        assert!(fixture
            .core
            .pending_outgoing_product_projections(NOW + 10)
            .unwrap()
            .is_empty());
        let snapshot = fixture.presence.snapshot(NOW + 10).unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(snapshot.records()[0].display_name(), "Alice");

        let restarted_core = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key,
                fixture.profile,
                fixture.grant,
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted = CollaborationPresenceProductPort::new(restarted_core).unwrap();
        assert_eq!(restarted.snapshot(NOW + 10).unwrap(), snapshot);
        assert!(restarted
            .snapshot(NOW + 10 + PRESENCE_TTL_SECS)
            .unwrap()
            .records()
            .is_empty());
    }

    #[test]
    fn reordered_and_expired_incoming_presence_is_terminal_without_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = fixture_at(&temp.path().join("reordered"));
        let (remote, remote_profile) = remote_authority(&fixture, "Newest", None);
        let newer = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW + 10,
                PRESENCE_TTL_SECS,
            )
            .unwrap();
        let older = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW + 5,
                PRESENCE_TTL_SECS,
            )
            .unwrap();
        let equal_conflict = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW + 10,
                PRESENCE_TTL_SECS,
            )
            .unwrap();
        for message in [&newer, &older, &equal_conflict] {
            fixture
                .core
                .accept_incoming_from_signed_source_for_test(message.envelope_bytes(), NOW + 11)
                .unwrap();
        }
        let handoffs = fixture.presence.pending_presences().unwrap();
        assert_eq!(handoffs.len(), 3);
        assert_eq!(
            fixture
                .presence
                .project_handoff(&handoffs[0], NOW + 11)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Applied
        );
        let newest_bytes = fs::read(fixture.presence.read_model.state_path()).unwrap();
        assert_eq!(
            fixture
                .presence
                .project_handoff(&handoffs[1], NOW + 11)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Superseded
        );
        assert_eq!(
            fixture
                .presence
                .project_handoff(&handoffs[2], NOW + 11)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Superseded
        );
        assert_eq!(
            fs::read(fixture.presence.read_model.state_path()).unwrap(),
            newest_bytes
        );
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 0);
        let snapshot = fixture.presence.snapshot(NOW + 11).unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(snapshot.records()[0].display_name(), "Newest");

        let expired_fixture = fixture_at(&temp.path().join("expired"));
        let expired = remote_presence(&expired_fixture, "Too late", None, NOW);
        expired_fixture
            .core
            .accept_incoming_from_signed_source_for_test(expired.envelope_bytes(), NOW + 1)
            .unwrap();
        let handoff = expired_fixture
            .presence
            .pending_presences()
            .unwrap()
            .remove(0);
        assert_eq!(
            expired_fixture
                .presence
                .project_handoff(&handoff, NOW + PRESENCE_TTL_SECS)
                .unwrap(),
            CollaborationPresenceProjectionOutcome::Expired
        );
        assert_eq!(
            expired_fixture
                .core
                .summary()
                .unwrap()
                .pending_product_handoffs,
            0
        );
        assert!(expired_fixture
            .presence
            .snapshot(NOW + PRESENCE_TTL_SECS)
            .unwrap()
            .records()
            .is_empty());
        assert!(!expired_fixture
            .data_root
            .join(PRESENCE_STATE_PARENT)
            .exists());
    }

    #[test]
    fn presence_capacity_prunes_expired_records_and_byte_overflow_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = fixture_at(temp.path());
        for index in 0..MAX_PRESENCE_RECORDS {
            let message = remote_presence(&fixture, &format!("Peer {index}"), None, NOW);
            if let Err(error) = fixture
                .presence
                .read_model
                .project(message.envelope_bytes(), NOW)
            {
                let state_bytes = fs::metadata(fixture.presence.read_model.state_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                panic!(
                    "presence record {index} exceeded the state after {state_bytes} bytes: {error}"
                );
            }
        }
        assert_eq!(
            fixture.presence.snapshot(NOW).unwrap().records().len(),
            MAX_PRESENCE_RECORDS
        );
        let overflow = remote_presence(&fixture, "Overflow", None, NOW);
        let before = fs::read(fixture.presence.read_model.state_path()).unwrap();
        assert!(fixture
            .presence
            .read_model
            .project(overflow.envelope_bytes(), NOW)
            .is_err());
        assert_eq!(
            fs::read(fixture.presence.read_model.state_path()).unwrap(),
            before
        );

        let replacement = remote_presence(&fixture, "After expiry", None, NOW + PRESENCE_TTL_SECS);
        fixture
            .presence
            .read_model
            .project(replacement.envelope_bytes(), NOW + PRESENCE_TTL_SECS)
            .unwrap();
        let snapshot = fixture.presence.snapshot(NOW + PRESENCE_TTL_SECS).unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(snapshot.records()[0].display_name(), "After expiry");

        let oversized = vec![b'x'; MAX_PRESENCE_STATE_BYTES + 1];
        fs::write(fixture.presence.read_model.state_path(), &oversized).unwrap();
        assert!(fixture.presence.snapshot(NOW + PRESENCE_TTL_SECS).is_err());
        assert_eq!(
            fs::read(fixture.presence.read_model.state_path()).unwrap(),
            oversized
        );
    }

    #[test]
    fn presence_read_model_is_owner_only_symlink_safe_and_corruption_preserving() {
        let temp = tempfile::tempdir().unwrap();
        let empty = fixture_at(&temp.path().join("empty"));
        assert!(empty.presence.snapshot(NOW).unwrap().records().is_empty());
        assert!(!empty.data_root.join("collaboration").exists());

        let symlink_root = temp.path().join("symlink-root");
        let target = temp.path().join("symlink-target");
        fs::create_dir_all(&symlink_root).unwrap();
        fs::create_dir_all(&target).unwrap();
        symlink(&target, symlink_root.join("collaboration")).unwrap();
        let symlink_fixture = fixture_at(&symlink_root);
        assert!(symlink_fixture.presence.snapshot(NOW).is_err());
        assert!(!target.join("default-conversation-presence").exists());

        let managed = fixture_at(&temp.path().join("managed"));
        let prepared = managed
            .presence
            .prepare_presence(binding(&managed, "managed"), &managed.person_profile, NOW)
            .unwrap();
        managed
            .presence
            .project_prepared_presence(&prepared, NOW)
            .unwrap();
        let state_path = managed.presence.read_model.state_path();
        let lock_path = managed.presence.read_model.lock_path();
        assert_eq!(
            fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(state_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let original = fs::read(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(managed.presence.snapshot(NOW).is_err());
        assert_eq!(fs::read(&state_path).unwrap(), original);
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::write(&state_path, b"{}").unwrap();
        assert!(managed.presence.snapshot(NOW).is_err());
        assert_eq!(fs::read(&state_path).unwrap(), b"{}");
        fs::write(&state_path, &original).unwrap();

        let state_target = temp.path().join("presence-state-target");
        fs::write(&state_target, &original).unwrap();
        fs::remove_file(&state_path).unwrap();
        symlink(&state_target, &state_path).unwrap();
        assert!(managed.presence.snapshot(NOW).is_err());
        fs::remove_file(&state_path).unwrap();
        fs::write(&state_path, &original).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();

        let lock_target = temp.path().join("presence-lock-target");
        fs::write(&lock_target, b"").unwrap();
        fs::remove_file(&lock_path).unwrap();
        symlink(&lock_target, &lock_path).unwrap();
        let newer = managed
            .presence
            .prepare_presence(
                binding(&managed, "managed-newer"),
                &managed.person_profile,
                NOW + 1,
            )
            .unwrap();
        assert!(managed
            .presence
            .project_prepared_presence(&newer, NOW + 1)
            .is_err());
        assert_eq!(fs::read(&state_path).unwrap(), original);
    }
}
