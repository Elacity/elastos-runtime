//! Durable provider-neutral state for one verified open default conversation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    collaboration_message_envelope_sha256, SignedCollaborationMessage,
    COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1, COLLABORATION_MESSAGE_SCHEMA_V1,
    MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES, MAX_COLLABORATION_CLOCK_SKEW_SECS,
    MAX_COLLABORATION_ENVELOPE_BYTES, MAX_COLLABORATION_MESSAGE_LIFETIME_SECS,
};
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::collaboration_default_conversation::{
    authorize_default_conversation_message, authorize_default_conversation_transport_message,
    AuthorizedDefaultConversationMessage, VerifiedDefaultConversationGrant,
};
use crate::collaboration_device_authority::DefaultConversationDeviceAuthority;
use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_protocol::{
    verify_collaboration_acceptance_receipt, verify_collaboration_transport_frame,
    verify_stored_acceptance_receipt_envelope, verify_stored_collaboration_acceptance_receipt,
    verify_stored_collaboration_message, VerifiedCollaborationAcceptanceReceipt,
    VerifiedCollaborationMessage,
};
use crate::collaboration_rate_limit::{
    CommunitySendRateLimited, HeldFrame, HeldFrames, HoldOutcome, ProfileRateLimiter,
    COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE, COMMUNITY_RATE_WINDOW_SECS,
    COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW, COMMUNITY_SENDS_PER_WINDOW,
    PRESENCE_RECEIVES_PER_SENDER_PER_WINDOW,
};
use crate::esp_binding::{esp_request_binding, EspRequestBinding, ESP_REQUEST_BINDING_SCHEMA};
use crate::host_lock::FileLock;

const CORE_STATE_SCHEMA: &str = "elastos.collaboration.default-conversation-state/v1";
const CORE_STATE_DIR: &str = "collaboration/default-conversation";
const CORE_STATE_FILE: &str = "state-v1.json";
const CORE_LOCK_FILE: &str = "state-v1.lock";
const HISTORY_STATE_FILE: &str = "history-v1.json";
const HISTORY_STATE_SCHEMA: &str = "elastos.collaboration.conversation-history/v1";
const CORE_NAMESPACE_DOMAIN: &[u8] = b"elastos.collaboration.default-conversation-state.v1";
pub(crate) const DEFAULT_CONVERSATION_SEND_METHOD: &str = "message.send";
const MAX_CORE_STATE_BYTES: usize = 24 * 1024 * 1024;
const MAX_UNRESOLVED_OUTGOING: usize = 64;
const MAX_OUTGOING_RECORDS: usize = 4_096;
const MAX_PENDING_INCOMING: usize = 32;
pub(crate) const MAX_PENDING_INCOMING_PER_SENDER: usize = 8;
pub(crate) const MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING: usize = 32;
const MAX_INCOMING_RECORDS_AND_TOMBSTONES: usize = 4_096;
const MAX_RECENT_PRESENCE: usize = 4_096;
type IncomingIdentityIndex = HashMap<(String, String), String>;
const MAX_VERIFICATION_CACHE_ENTRIES: usize = MAX_OUTGOING_RECORDS
    * MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING
    + MAX_INCOMING_RECORDS_AND_TOMBSTONES
    + MAX_PENDING_INCOMING;
pub(crate) const MAX_CONVERSATION_HISTORY_MESSAGES: usize = 200;
pub(crate) const MAX_CONVERSATION_HISTORY_BYTES: usize = 1024 * 1024;
pub(crate) const CONVERSATION_HISTORY_RETENTION_SECS: u64 = 24 * 60 * 60;

pub(crate) struct CollaborationCore {
    authority: DefaultConversationDeviceAuthority,
    operation_capsule: String,
    data_root: PathBuf,
    state_dir: PathBuf,
    community_membership: std::sync::Arc<crate::collaboration_release_network::CommunityMembership>,
    mutation_mutex: Mutex<()>,
    /// Counted from stored outgoing messages, so only a saved message counts.
    sends_per_window: usize,
    receive_rate: ProfileRateLimiter,
    presence_receive_rate: ProfileRateLimiter,
    /// Serializes receive-rate admission through its durable commit and count.
    receive_admission_mutex: Mutex<()>,
    held: HeldFrames,
    /// Acknowledged presence by envelope hash: its receipt and `expires_at`.
    /// Presence is admissible only until its own expiry (at most 45 s), so it
    /// needs no persisted 24-hour tombstone; this suppresses duplicates.
    recent_presence: Mutex<HashMap<String, (String, u64)>>,
    message_verifications: Mutex<SignedBytesCache<AuthorizedDefaultConversationMessage>>,
    receipt_verifications: Mutex<SignedBytesCache<VerifiedCollaborationAcceptanceReceipt>>,
    bound_receipt_verifications: Mutex<SignedBytesCache<VerifiedCollaborationAcceptanceReceipt>>,
    verified_state: Mutex<Option<(Vec<u8>, CoreState)>>,
    #[cfg(test)]
    verification_misses: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    directory_syncs: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    write_fault: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    history_write_fault: std::sync::atomic::AtomicU8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationCoreSummary {
    pub(crate) live_unresolved_outgoing: usize,
    pub(crate) expired_unaccepted_outgoing: usize,
    pub(crate) remotely_accepted_outgoing: usize,
    pub(crate) pending_product_handoffs: usize,
    pub(crate) replay_tombstones: usize,
}

#[derive(Debug)]
pub(crate) struct DurableOutgoingMessage {
    envelope_bytes: Vec<u8>,
    envelope_sha256: String,
    created_at: u64,
    shared_chat: bool,
    accepted_endpoints: Vec<String>,
    /// A presence announcement's principal and sender Profile DID; the send
    /// boundary re-validates that principal's grant before each send.
    presence_sender: Option<(String, String)>,
}

pub(crate) struct PendingOutgoingProductProjection {
    outgoing: DurableOutgoingMessage,
}

impl PendingOutgoingProductProjection {
    pub(crate) fn outgoing(&self) -> &DurableOutgoingMessage {
        &self.outgoing
    }
}

impl DurableOutgoingMessage {
    pub(crate) fn shared_chat(&self) -> bool {
        self.shared_chat
    }

    pub(crate) fn accepted_by(&self, endpoint: &str) -> bool {
        self.accepted_endpoints
            .iter()
            .any(|accepted| accepted == endpoint)
    }

    pub(crate) fn envelope_bytes(&self) -> &[u8] {
        &self.envelope_bytes
    }

    pub(crate) fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }

    pub(crate) fn created_at(&self) -> u64 {
        self.created_at
    }

    pub(crate) fn remote_acceptance_recorded(&self) -> bool {
        !self.accepted_endpoints.is_empty()
    }

    pub(crate) fn presence_sender(&self) -> Option<(&str, &str)> {
        self.presence_sender
            .as_ref()
            .map(|(principal, profile_did)| (principal.as_str(), profile_did.as_str()))
    }
}

/// Product read evidence from the verified, bounded outgoing store. It includes
/// expired originals while Core retains them; it does not change retry selection.
pub(crate) struct OutgoingChatDeliveryEvidence {
    pub(crate) envelope_sha256: String,
    pub(crate) expires_at: u64,
    pub(crate) accepted: bool,
}

#[derive(Debug)]
pub(crate) struct DurableIncomingAcceptance {
    authorized: AuthorizedDefaultConversationMessage,
    acceptance_receipt_bytes: Vec<u8>,
    product_handoff_pending: bool,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "COLLAB-06 product integration will consume durable incoming acceptance state"
    )
)]
impl DurableIncomingAcceptance {
    pub(crate) fn authorized_message(&self) -> &AuthorizedDefaultConversationMessage {
        &self.authorized
    }

    pub(crate) fn acceptance_receipt_bytes(&self) -> &[u8] {
        &self.acceptance_receipt_bytes
    }

    pub(crate) fn product_handoff_pending(&self) -> bool {
        self.product_handoff_pending
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CollaborationTransportIncomingAcceptance {
    acceptance_receipt_bytes: Vec<u8>,
    product_handoff_pending: bool,
}

impl CollaborationTransportIncomingAcceptance {
    pub(crate) fn acceptance_receipt_bytes(&self) -> &[u8] {
        &self.acceptance_receipt_bytes
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "COLLAB-06 product integration will consume pending handoff state"
        )
    )]
    pub(crate) fn product_handoff_pending(&self) -> bool {
        self.product_handoff_pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollaborationTransportRemoteAcceptance {
    Applied,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollaborationTransportRejection {
    MalformedOrNoncanonicalFrame,
    UnsupportedSchema,
    InvalidMessage,
    MessageSelfEcho,
    MessageIdentityConflict,
    InvalidAcceptanceReceipt,
    AcceptanceFromLocalDevice,
    AcceptanceWithoutOutgoingMessage,
    AcceptanceBeforeProductProjection,
    AcceptanceRecipientConflict,
    /// The sender is over the receive limit. The Home holds the frame and
    /// retries it once the sender's window slides.
    SenderRateLimited,
    /// The sender already has the most messages waiting for Chat. Held and
    /// retried like `SenderRateLimited`, so one sender never holds the batch.
    SenderBacklogFull,
    /// This Core is a Community bootstrap node, which admits no frame, or a
    /// bootstrap endpoint sent or receipted a Shared post. Bootstrap
    /// endpoints forward gossip and carry discovery only.
    BootstrapEndpoint,
    /// The original is already accepted and this further receipt could not be
    /// stored (receipt or byte capacity, storage). It adds nothing, so it is
    /// consumed; capacity would not free while the original lives.
    AcceptanceNotStored,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CollaborationTransportIngestion {
    Incoming(CollaborationTransportIncomingAcceptance),
    RemoteAcceptance(CollaborationTransportRemoteAcceptance),
    Rejected(CollaborationTransportRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CollaborationTransportRetryableError;

#[derive(Clone, Copy)]
enum CollaborationTransportFrameKind {
    Message,
    AcceptanceReceipt,
}

pub(crate) struct PendingProductHandoff {
    authorized: AuthorizedDefaultConversationMessage,
}

impl PendingProductHandoff {
    pub(crate) fn authorized_message(&self) -> &AuthorizedDefaultConversationMessage {
        &self.authorized
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreState {
    schema: String,
    binding: CoreStateBinding,
    outgoing: Vec<OutgoingRecord>,
    incoming: Vec<IncomingRecord>,
    incoming_tombstones: Vec<IncomingTombstone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationHistoryState {
    schema: String,
    binding: CoreStateBinding,
    envelopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_envelope_hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_projection_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreStateBinding {
    network_id: String,
    grant_cid: String,
    local_device_did: String,
    operation_capsule: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutgoingRecord {
    operation: EspRequestBinding,
    envelope: String,
    #[serde(
        default,
        skip_serializing_if = "OutgoingProductProjectionState::is_pending"
    )]
    local_product_projection: OutgoingProductProjectionState,
    remote_acceptance_receipts: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutgoingProductProjectionState {
    #[default]
    Pending,
    Complete,
    /// A verified legacy intent with no consumable Shared product. Keep its
    /// request binding for the original replay window, without retrying it.
    Retired,
}

impl OutgoingProductProjectionState {
    fn is_pending(&self) -> bool {
        *self == Self::Pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProductHandoffState {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncomingRecord {
    envelope: String,
    acceptance_receipt: String,
    product_handoff: ProductHandoffState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncomingTombstone {
    acceptance_receipt: String,
    retain_until: u64,
}

struct Mutation<T> {
    value: T,
    changed: bool,
}

/// Success-only verification reuse within one immutable Core authority. Keys
/// retain exact bytes; the digest only indexes them. FIFO eviction bounds both
/// signed-byte storage and the number of entries independently of file state.
struct SignedBytesCache<T> {
    entries: HashMap<[u8; 32], (Vec<u8>, T)>,
    order: VecDeque<[u8; 32]>,
    bytes: usize,
}

impl<T: Clone> SignedBytesCache<T> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }

    fn get(&self, bytes: &[u8]) -> Option<T> {
        let key: [u8; 32] = Sha256::digest(bytes).into();
        self.entries
            .get(&key)
            .filter(|(exact, _)| exact == bytes)
            .map(|(_, value)| value.clone())
    }

    fn insert(&mut self, bytes: &[u8], value: T) {
        if bytes.len() > MAX_CORE_STATE_BYTES {
            return;
        }
        let key: [u8; 32] = Sha256::digest(bytes).into();
        if self
            .entries
            .get(&key)
            .is_some_and(|(exact, _)| exact == bytes)
        {
            return;
        }
        if let Some((previous, _)) = self.entries.remove(&key) {
            self.bytes -= previous.len();
            self.order.retain(|old| *old != key);
        }
        while self.bytes.saturating_add(bytes.len()) > MAX_CORE_STATE_BYTES
            || self.entries.len() >= MAX_VERIFICATION_CACHE_ENTRIES
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((previous, _)) = self.entries.remove(&oldest) {
                self.bytes -= previous.len();
            }
        }
        self.bytes += bytes.len();
        self.order.push_back(key);
        self.entries.insert(key, (bytes.to_vec(), value));
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteFault {
    BeforeWrite = 1,
    AfterFileSync = 2,
    AfterRename = 3,
    BeforeAncestorSync = 4,
}

impl CollaborationCore {
    pub(crate) fn new(
        data_root: &Path,
        signing_key: SigningKey,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
        operation_capsule: &str,
    ) -> anyhow::Result<Self> {
        crate::collaboration_protocol::validate_service(operation_capsule)
            .context("collaboration operation capsule")?;
        let authority = DefaultConversationDeviceAuthority::new(signing_key, profile, grant)?;
        let state_dir = data_root
            .join(CORE_STATE_DIR)
            .join(state_namespace(&authority));
        let community_membership = std::sync::Arc::new(
            crate::collaboration_release_network::CommunityMembership::load(
                data_root,
                authority.network_id(),
            )?,
        );
        Ok(Self {
            authority,
            operation_capsule: operation_capsule.to_string(),
            data_root: data_root.to_path_buf(),
            state_dir,
            community_membership,
            mutation_mutex: Mutex::new(()),
            sends_per_window: COMMUNITY_SENDS_PER_WINDOW,
            receive_rate: ProfileRateLimiter::new(
                COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW,
                COMMUNITY_RATE_WINDOW_SECS,
            ),
            presence_receive_rate: ProfileRateLimiter::new(
                PRESENCE_RECEIVES_PER_SENDER_PER_WINDOW,
                COMMUNITY_RATE_WINDOW_SECS,
            ),
            receive_admission_mutex: Mutex::new(()),
            held: HeldFrames::default(),
            recent_presence: Mutex::new(HashMap::new()),
            message_verifications: Mutex::new(SignedBytesCache::new()),
            receipt_verifications: Mutex::new(SignedBytesCache::new()),
            bound_receipt_verifications: Mutex::new(SignedBytesCache::new()),
            verified_state: Mutex::new(None),
            #[cfg(test)]
            verification_misses: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            directory_syncs: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            write_fault: std::sync::atomic::AtomicU8::new(0),
            #[cfg(test)]
            history_write_fault: std::sync::atomic::AtomicU8::new(0),
        })
    }

    /// Seed the signed, receipt-bound legacy shape that lifecycle readers
    /// repair. Product admission deliberately refuses these malformed payloads.
    #[cfg(test)]
    pub(crate) fn test_store_legacy_product_handoffs(
        &self,
        remote: &Self,
        payload_types: &[&str],
        now: u64,
    ) {
        let (state, _) = self.load_state_raw().unwrap();
        let mut state = state.unwrap_or_else(|| self.empty_state());
        for payload_type in payload_types {
            let original = remote
                .authority
                .prepare_outgoing(
                    remote.sender_service(),
                    payload_type,
                    serde_json::json!({"extra": true}),
                    now,
                    45,
                )
                .unwrap();
            let incoming = self
                .authority
                .authorize_incoming(original.envelope_bytes(), &remote.local_device_did(), now)
                .unwrap();
            let receipt = self
                .authority
                .prepare_acceptance_receipt(&incoming, now)
                .unwrap();
            state.incoming.push(IncomingRecord {
                envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
                acceptance_receipt: String::from_utf8(receipt).unwrap(),
                product_handoff: ProductHandoffState::Pending,
            });
        }
        self.validate_state(&state).unwrap();
        self.ensure_state_directory().unwrap();
        self.write_state(&state).unwrap();
    }

    pub(crate) fn sender_service(&self) -> &str {
        self.authority.sender_service()
    }

    pub(crate) fn community_membership(
        &self,
    ) -> &std::sync::Arc<crate::collaboration_release_network::CommunityMembership> {
        &self.community_membership
    }

    /// Serialize the Home-wide choice with durable Community admission. A send
    /// accepted before Leave may remain queued; a send cannot commit across it.
    pub(crate) fn set_community_joined(&self, joined: bool) -> anyhow::Result<()> {
        let _guard = self
            .mutation_mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration mutation mutex is poisoned"))?;
        self.community_membership.set_joined(joined)
    }

    pub(crate) fn conversation_scope(&self) -> (&str, &str) {
        (
            self.authority.network_id(),
            &self.authority.grant().grant().conversation_id,
        )
    }

    #[cfg(test)]
    pub(crate) fn test_local_device_did(&self) -> String {
        self.authority.local_device_did()
    }

    pub(crate) fn product_data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) fn authorize_stored_product_message(
        &self,
        envelope_bytes: &[u8],
    ) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
        if let Some(verified) = self
            .message_verifications
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration message verification cache poisoned"))?
            .get(envelope_bytes)
        {
            return Ok(verified);
        }
        self.note_verification_miss();
        let verified = verify_stored_collaboration_message(
            envelope_bytes,
            self.authority.profile(),
            self.authority.sender_service(),
        )?;
        let authorized = authorize_default_conversation_message(self.authority.grant(), &verified)?;
        self.message_verifications
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration message verification cache poisoned"))?
            .insert(envelope_bytes, authorized.clone());
        Ok(authorized)
    }

    fn note_verification_miss(&self) {
        #[cfg(test)]
        self.verification_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn incoming_identity_conflicts(
        &self,
        state: &CoreState,
        message: &VerifiedCollaborationMessage,
    ) -> anyhow::Result<bool> {
        if incoming_identity_conflicts(state, message, |bytes| self.verify_receipt_envelope(bytes))?
        {
            return Ok(true);
        }
        // History and live admission share the Core writer lock. Use the raw
        // reader here because this check also runs inside with_mutation.
        let (history, _) = self.load_history_raw()?;
        self.history_identity_conflicts(history.as_ref(), message)
    }

    fn history_identity_conflicts(
        &self,
        history: Option<&ConversationHistoryState>,
        candidate: &VerifiedCollaborationMessage,
    ) -> anyhow::Result<bool> {
        let Some(history) = history else {
            return Ok(false);
        };
        let payload = &candidate.envelope().payload;
        for original in &history.envelopes {
            let authorized = self.authorize_stored_product_message(original.as_bytes())?;
            let stored = &authorized.message().envelope().payload;
            if stored.sender_profile_did == payload.sender_profile_did
                && (stored.message_id == payload.message_id || stored.nonce == payload.nonce)
                && authorized.message().envelope_sha256() != candidate.envelope_sha256()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn reject_incoming_conflicts(
        &self,
        state: &CoreState,
        message: &VerifiedCollaborationMessage,
    ) -> anyhow::Result<()> {
        if self.incoming_identity_conflicts(state, message)? {
            anyhow::bail!("collaboration sender reused a message ID or nonce");
        }
        Ok(())
    }

    fn outgoing_handle(
        &self,
        entry: &OutgoingRecord,
        verified: VerifiedCollaborationMessage,
    ) -> DurableOutgoingMessage {
        outgoing_handle(entry, verified, |bytes| self.verify_receipt_envelope(bytes))
    }

    fn verify_receipt_envelope(
        &self,
        bytes: &[u8],
    ) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt> {
        if let Some(verified) = self
            .receipt_verifications
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration receipt verification cache poisoned"))?
            .get(bytes)
        {
            return Ok(verified);
        }
        self.note_verification_miss();
        let verified = verify_stored_acceptance_receipt_envelope(bytes)?;
        self.receipt_verifications
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration receipt verification cache poisoned"))?
            .insert(bytes, verified.clone());
        Ok(verified)
    }

    fn verify_receipt_for_message(
        &self,
        bytes: &[u8],
        message: &VerifiedCollaborationMessage,
    ) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt> {
        let mut key = Vec::with_capacity(message.envelope_sha256().len() + bytes.len());
        key.extend_from_slice(message.envelope_sha256().as_bytes());
        key.extend_from_slice(bytes);
        if let Some(verified) = self
            .bound_receipt_verifications
            .lock()
            .map_err(|_| {
                anyhow::anyhow!("collaboration bound receipt verification cache poisoned")
            })?
            .get(&key)
        {
            return Ok(verified);
        }
        self.note_verification_miss();
        let verified = verify_stored_collaboration_acceptance_receipt(bytes, message)?;
        self.bound_receipt_verifications
            .lock()
            .map_err(|_| {
                anyhow::anyhow!("collaboration bound receipt verification cache poisoned")
            })?
            .insert(&key, verified.clone());
        self.receipt_verifications
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration receipt verification cache poisoned"))?
            .insert(bytes, verified.clone());
        Ok(verified)
    }

    /// Historical admission checks original authorship and the exact grant,
    /// independently of the short live-delivery lifetime and replay path.
    pub(crate) fn authorize_history_message(
        &self,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
        let authorized = self.authorize_stored_product_message(envelope_bytes)?;
        crate::collaboration_product::validate_chat_projection(&authorized)?;
        let message = &authorized.message().envelope().payload;
        if message.created_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
            || message
                .created_at
                .saturating_add(CONVERSATION_HISTORY_RETENTION_SECS)
                <= now
        {
            anyhow::bail!("collaboration history message is outside its retention window");
        }
        Ok(authorized)
    }

    /// Read bounded canonical originals without changing the live replay state.
    pub(crate) fn conversation_history(&self, now: u64) -> anyhow::Result<Vec<Vec<u8>>> {
        let state = self.load_history()?.unwrap_or_else(|| self.empty_history());
        state
            .envelopes
            .iter()
            .filter(|envelope| {
                let hash = collaboration_message_envelope_sha256(envelope.as_bytes());
                !state.retired_envelope_hashes.contains(&hash)
                    && !state.pending_projection_hashes.contains(&hash)
            })
            .filter_map(|envelope| {
                let authorized = self.authorize_stored_product_message(envelope.as_bytes());
                match authorized {
                    Ok(message)
                        if message
                            .message()
                            .envelope()
                            .payload
                            .created_at
                            .saturating_add(CONVERSATION_HISTORY_RETENTION_SECS)
                            > now =>
                    {
                        Some(
                            self.authorize_history_message(envelope.as_bytes(), now)
                                .map(|_| envelope.as_bytes().to_vec()),
                        )
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    /// Return only newly retained originals in the same age/count/byte window.
    /// An older peer cache cannot repeatedly reinsert rows the product evicted.
    #[cfg(test)]
    pub(crate) fn history_projection_candidates(
        &self,
        envelopes: &[Vec<u8>],
        now: u64,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        self.with_mutation(None, |live| {
            let (history, repaired) = self.load_history_raw()?;
            let history = history.unwrap_or_else(|| self.empty_history());
            if repaired {
                self.write_history(&history)?;
            }
            let (_, value) = self.history_projection_state(live, history, envelopes, now)?;
            Ok(Mutation {
                value,
                changed: false,
            })
        })
    }

    /// Product publishes only these candidates while the same Core writer lock
    /// protects their history identities. The callback may use Room, not Core.
    pub(crate) fn with_history_projection<T>(
        &self,
        envelopes: &[Vec<u8>],
        now: u64,
        project: impl FnOnce(&[Vec<u8>]) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.with_mutation(None, |live| {
            let (history, repaired) = self.load_history_raw()?;
            let history = history.unwrap_or_else(|| self.empty_history());
            if repaired {
                self.write_history(&history)?;
            }
            let before = canonical_history_bytes(&history)?;
            let (mut history, candidates) =
                self.history_projection_state(live, history, envelopes, now)?;
            // Reserve signed identity before any Room row can become visible.
            // Rewriting an existing pending reservation also proves publication
            // after an earlier indeterminate rename/directory-sync failure.
            if !candidates.is_empty() || canonical_history_bytes(&history)? != before {
                self.write_history(&history)?;
            }
            let value = project(&candidates)?;
            if !candidates.is_empty() {
                let completed = candidates
                    .iter()
                    .map(|original| collaboration_message_envelope_sha256(original))
                    .collect::<HashSet<_>>();
                history
                    .pending_projection_hashes
                    .retain(|hash| !completed.contains(hash));
                self.write_history(&history)?;
            }
            Ok(Mutation {
                value,
                changed: false,
            })
        })
    }

    /// Retry verified reserved originals locally, within the unchanged history
    /// age/count/byte window, even when the sending peer is unavailable.
    pub(crate) fn pending_history_projection(&self, now: u64) -> anyhow::Result<Vec<Vec<u8>>> {
        self.with_mutation(None, |_| {
            let (history, repaired) = self.load_history_raw()?;
            let mut history = history.unwrap_or_else(|| self.empty_history());
            let before = canonical_history_bytes(&history)?;
            prune_history(&mut history, now)?;
            if repaired || canonical_history_bytes(&history)? != before {
                self.write_history(&history)?;
            }
            let value = history
                .envelopes
                .iter()
                .filter(|original| {
                    history
                        .pending_projection_hashes
                        .contains(&collaboration_message_envelope_sha256(original.as_bytes()))
                })
                .map(|original| original.as_bytes().to_vec())
                .collect();
            Ok(Mutation {
                value,
                changed: false,
            })
        })
    }

    fn history_projection_state(
        &self,
        live: &CoreState,
        mut history: ConversationHistoryState,
        envelopes: &[Vec<u8>],
        now: u64,
    ) -> anyhow::Result<(ConversationHistoryState, Vec<Vec<u8>>)> {
        anyhow::ensure!(
            envelopes.len() <= MAX_CONVERSATION_HISTORY_MESSAGES,
            "collaboration history batch exceeds its message bound"
        );
        let (live_ids, live_nonces) = self.live_history_identities(live)?;
        let mut ids = HashMap::new();
        let mut nonces = HashMap::new();
        let mut known = HashSet::new();
        for envelope in &history.envelopes {
            let original = self.authorize_stored_product_message(envelope.as_bytes())?;
            let message = original.message();
            known.insert(message.envelope_sha256().to_string());
            insert_incoming_identity(
                &mut ids,
                &mut nonces,
                &message.envelope().payload,
                message.envelope_sha256(),
            )?;
        }
        let prior = known.clone();
        let mut eligible = HashSet::new();
        for envelope in envelopes {
            let authorized = self.authorize_stored_product_message(envelope)?;
            crate::collaboration_product::validate_chat_projection(&authorized)?;
            let message = authorized.message();
            if identity_index_conflicts(&live_ids, &live_nonces, message)
                || identity_index_conflicts(&ids, &nonces, message)
            {
                continue;
            }
            eligible.insert(message.envelope_sha256().to_string());
            if known.insert(message.envelope_sha256().to_string()) {
                insert_incoming_identity(
                    &mut ids,
                    &mut nonces,
                    &message.envelope().payload,
                    message.envelope_sha256(),
                )?;
                history.envelopes.push(String::from_utf8(envelope.clone())?);
                history
                    .pending_projection_hashes
                    .push(message.envelope_sha256().to_string());
            }
        }
        prune_history(&mut history, now)?;
        self.validate_history(&history)?;
        let candidates = history
            .envelopes
            .iter()
            .filter(|envelope| {
                let hash = collaboration_message_envelope_sha256(envelope.as_bytes());
                eligible.contains(&hash)
                    && (!prior.contains(&hash) || history.pending_projection_hashes.contains(&hash))
            })
            .map(|envelope| envelope.as_bytes().to_vec())
            .collect();
        Ok((history, candidates))
    }

    /// Called by a product only after its durable projection succeeds. Old
    /// projections can finish normally while their originals age out of history.
    pub(crate) fn retain_history_message(
        &self,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<()> {
        self.retain_history_messages(&[envelope_bytes.to_vec()], now)
    }

    pub(crate) fn retain_history_messages(
        &self,
        envelopes: &[Vec<u8>],
        now: u64,
    ) -> anyhow::Result<()> {
        if envelopes.len() > MAX_CONVERSATION_HISTORY_MESSAGES {
            anyhow::bail!("collaboration history batch exceeds its message bound");
        }
        for envelope in envelopes {
            crate::collaboration_product::validate_chat_projection(
                &self.authorize_stored_product_message(envelope)?,
            )?;
        }
        self.with_mutation(None, |live| {
            self.validate_history_live_identities_in_state(live, envelopes)?;
            let (history, repaired) = self.load_history_raw()?;
            let mut state = history.unwrap_or_else(|| self.empty_history());
            if repaired {
                self.write_history(&state)?;
            }
            let before = canonical_history_bytes(&state)?;
            append_history_envelopes(&mut state, envelopes)?;
            self.validate_history_identities(&state)?;
            prune_history(&mut state, now)?;
            self.validate_history(&state)?;
            if canonical_history_bytes(&state)? != before {
                self.write_history(&state)?;
            }
            Ok(Mutation {
                value: (),
                changed: false,
            })
        })
    }

    pub(crate) fn local_device_did(&self) -> String {
        self.authority.local_device_did()
    }

    pub(crate) fn is_bootstrap_endpoint(&self, endpoint: &str) -> bool {
        let Some(key) = crate::carrier::did_to_public_key(endpoint) else {
            return true;
        };
        self.authority
            .profile()
            .profile()
            .bootstrap_peers
            .iter()
            .any(|peer| peer.node_id == key.to_string())
    }

    /// Gossip carries Presence metadata and Shared posts with their receipts.
    /// Shared posts also use the authenticated Home-to-Home provider exchange.
    /// A bootstrap node only forwards the topic: its Core takes no frame, so
    /// gossip costs it no verification or storage.
    pub(crate) fn allows_gossip_frame(&self, frame: &[u8]) -> anyhow::Result<bool> {
        if self.is_bootstrap_endpoint(&self.local_device_did()) {
            return Ok(false);
        }
        let Ok(frame) = verify_collaboration_transport_frame(frame) else {
            return Ok(false);
        };
        match collaboration_transport_frame_kind(frame.envelope_bytes()) {
            Ok(CollaborationTransportFrameKind::Message) => {
                let Ok(message) = self.authorize_stored_product_message(frame.envelope_bytes())
                else {
                    return Ok(false);
                };
                Ok(gossip_payload_type(
                    &message.message().envelope().payload.payload_type,
                ))
            }
            Ok(CollaborationTransportFrameKind::AcceptanceReceipt) => {
                let Ok(receipt) = self.verify_receipt_envelope(frame.envelope_bytes()) else {
                    return Ok(false);
                };
                let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
                let hash = &receipt.envelope().payload.message_envelope_sha256;
                let Some(outgoing) = state.outgoing.iter().find(|entry| {
                    collaboration_message_envelope_sha256(entry.envelope.as_bytes()) == *hash
                }) else {
                    return Ok(false);
                };
                Ok(gossip_payload_type(
                    &self
                        .verify_outgoing_record(outgoing)?
                        .envelope()
                        .payload
                        .payload_type,
                ))
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn allows_gossip_receipt(
        &self,
        receipt_frame: &[u8],
        original_frame: &[u8],
    ) -> anyhow::Result<bool> {
        let original = verify_collaboration_transport_frame(original_frame)?;
        let message = self.authorize_stored_product_message(original.envelope_bytes())?;
        if !gossip_payload_type(&message.message().envelope().payload.payload_type) {
            return Ok(false);
        }
        let receipt = verify_collaboration_transport_frame(receipt_frame)?;
        self.verify_receipt_for_message(receipt.envelope_bytes(), message.message())?;
        Ok(true)
    }

    pub(crate) fn accept_home_chat(
        &self,
        frame: &[u8],
        source: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        self.community_membership.require_joined()?;
        if self.is_bootstrap_endpoint(source)
            || self.is_bootstrap_endpoint(&self.local_device_did())
        {
            anyhow::bail!("Community bootstrap endpoints carry discovery metadata only");
        }
        let verified = verify_collaboration_transport_frame(frame)?;
        if verified.source_endpoint_did() != source
            || self
                .authorize_stored_product_message(verified.envelope_bytes())?
                .message()
                .envelope()
                .payload
                .payload_type
                != COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
        {
            anyhow::bail!("Shared delivery does not match its authenticated Home source");
        }
        match self.ingest_transport_frame(frame, now) {
            Ok(CollaborationTransportIngestion::Incoming(accepted)) => {
                Ok(accepted.acceptance_receipt_bytes().to_vec())
            }
            _ => anyhow::bail!("Shared delivery was refused"),
        }
    }

    pub(crate) fn record_home_chat_acceptance(
        &self,
        receipt: &[u8],
        message_hash: &str,
        endpoint: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        self.community_membership.require_joined()?;
        let verified = self.verify_receipt_envelope(receipt)?;
        if self.is_bootstrap_endpoint(endpoint)
            || verified.accepting_endpoint_did() != endpoint
            || verified.envelope().payload.message_envelope_sha256 != message_hash
        {
            anyhow::bail!("Shared acceptance does not name the selected Home");
        }
        self.record_remote_acceptance(receipt, now)
    }

    pub(crate) fn prepare_transport_frame(&self, envelope_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.authority.prepare_transport_frame(envelope_bytes)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "COLLAB-06 product integration will consume collaboration core summaries"
        )
    )]
    pub(crate) fn summary(&self) -> anyhow::Result<CollaborationCoreSummary> {
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let now = now_secs();
        Ok(CollaborationCoreSummary {
            live_unresolved_outgoing: state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.local_product_projection != OutgoingProductProjectionState::Retired
                        && entry.remote_acceptance_receipts.is_empty()
                        && stored_message_expiry(&entry.envelope)
                            .is_ok_and(|expires_at| expires_at > now)
                })
                .count(),
            expired_unaccepted_outgoing: state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.local_product_projection != OutgoingProductProjectionState::Retired
                        && entry.remote_acceptance_receipts.is_empty()
                        && stored_message_expiry(&entry.envelope)
                            .is_ok_and(|expires_at| expires_at <= now)
                })
                .count(),
            remotely_accepted_outgoing: state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.local_product_projection != OutgoingProductProjectionState::Retired
                        && !entry.remote_acceptance_receipts.is_empty()
                })
                .count(),
            pending_product_handoffs: state.incoming.len(),
            replay_tombstones: state.incoming_tombstones.len(),
        })
    }

    pub(crate) fn ingest_transport_frame(
        &self,
        frame: &[u8],
        now: u64,
    ) -> Result<CollaborationTransportIngestion, CollaborationTransportRetryableError> {
        if !self.community_membership.joined() {
            return Err(CollaborationTransportRetryableError);
        }
        if self.is_bootstrap_endpoint(&self.authority.local_device_did()) {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::BootstrapEndpoint,
            ));
        }
        let verified_transport = match verify_collaboration_transport_frame(frame) {
            Ok(verified) => verified,
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::MalformedOrNoncanonicalFrame,
                ))
            }
        };
        let kind = match collaboration_transport_frame_kind(verified_transport.envelope_bytes()) {
            Ok(kind) => kind,
            Err(reason) => return Ok(CollaborationTransportIngestion::Rejected(reason)),
        };
        match kind {
            CollaborationTransportFrameKind::Message => self.ingest_transport_message(
                frame,
                verified_transport.envelope_bytes(),
                verified_transport.source_endpoint_did(),
                now,
            ),
            CollaborationTransportFrameKind::AcceptanceReceipt => self
                .ingest_transport_acceptance_receipt(
                    verified_transport.envelope_bytes(),
                    verified_transport.source_endpoint_did(),
                    now,
                ),
        }
    }

    fn ingest_transport_message(
        &self,
        transport_frame: &[u8],
        frame: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> Result<CollaborationTransportIngestion, CollaborationTransportRetryableError> {
        let verified = match self.authorize_stored_product_message(frame) {
            Ok(authorized) => authorized.message().clone(),
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::InvalidMessage,
                ))
            }
        };
        let authorized = match authorize_default_conversation_transport_message(
            self.authority.grant(),
            &verified,
            source_endpoint_did,
        ) {
            Ok(authorized) => authorized,
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::InvalidMessage,
                ))
            }
        };
        if validate_shared_product_projection(&authorized).is_err() {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::InvalidMessage,
            ));
        }
        if verified.envelope().signer_did == self.authority.local_device_did() {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::MessageSelfEcho,
            ));
        }
        if verified.envelope().payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
            && self.is_bootstrap_endpoint(source_endpoint_did)
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::BootstrapEndpoint,
            ));
        }

        let _receive_guard = self
            .receive_admission_mutex
            .lock()
            .map_err(|_| CollaborationTransportRetryableError)?;
        let envelope_hash = verified.envelope_sha256().to_string();
        let mut state = self
            .load_state()
            .map_err(|_| CollaborationTransportRetryableError)?
            .unwrap_or_else(|| self.empty_state());
        if let Some(existing) = self
            .exact_incoming_replay(&state, frame, &envelope_hash, now)
            .map_err(|_| CollaborationTransportRetryableError)?
        {
            return Ok(CollaborationTransportIngestion::Incoming(
                transport_incoming_acceptance(existing),
            ));
        }
        self.prune_terminal_state(&mut state, now)
            .map_err(|_| CollaborationTransportRetryableError)?;
        let incoming = match self
            .authority
            .authorize_incoming(frame, source_endpoint_did, now)
        {
            Ok(incoming) => incoming,
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::InvalidMessage,
                ))
            }
        };
        if self
            .incoming_identity_conflicts(&state, incoming.message())
            .map_err(|_| CollaborationTransportRetryableError)?
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::MessageIdentityConflict,
            ));
        }
        // Count only an authorized, new message, so a forged or replayed frame
        // cannot spend another sender's budget. A per-sender refusal lets the
        // rest of the batch move on; this Home holds the frame and retries it,
        // because the sender stops resending once any other Home accepts it.
        let rate_limited_sender = (incoming.message().envelope().payload.payload_type
            == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE)
            .then(|| {
                incoming
                    .message()
                    .envelope()
                    .payload
                    .sender_profile_did
                    .clone()
            });
        // Presence replaces itself every 15 s: a sender over its presence
        // limit loses the frame quietly, and a presence that claims a longer
        // life than an honest one is refused, so presence replay tombstones
        // stay a few per sender and drain within minutes.
        let payload = &incoming.message().envelope().payload;
        let presence_sender = (payload.payload_type
            == crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE)
            .then(|| payload.sender_profile_did.clone());
        if presence_sender.is_some()
            && payload.expires_at.saturating_sub(payload.created_at)
                > crate::collaboration_presence::PRESENCE_TTL_SECS
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::InvalidMessage,
            ));
        }
        if let Some(sender_profile_did) = &presence_sender {
            if self
                .presence_receive_rate
                .check(sender_profile_did, now)
                .is_err()
            {
                tracing::debug!("Community presence dropped: sender is over its limit");
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::SenderRateLimited,
                ));
            }
        }
        if let Some(sender_profile_did) = &rate_limited_sender {
            if self.receive_rate.check(sender_profile_did, now).is_err() {
                return self.hold_refused(
                    transport_frame,
                    incoming.message(),
                    CollaborationTransportRejection::SenderRateLimited,
                    now,
                );
            }
        }
        if self.sender_backlog_full(&state, incoming.message()) {
            return self.hold_refused(
                transport_frame,
                incoming.message(),
                CollaborationTransportRejection::SenderBacklogFull,
                now,
            );
        }
        // Home-wide limits stay retryable: the whole Home must catch up first.
        self.ensure_incoming_capacity(&state, incoming.message(), frame)
            .map_err(|_| CollaborationTransportRetryableError)?;

        let accepted = self
            .accept_incoming(frame, source_endpoint_did, now)
            .map_err(|_| CollaborationTransportRetryableError)?;
        if let Some(sender_profile_did) = &rate_limited_sender {
            self.receive_rate.record(sender_profile_did, now);
        }
        if let Some(sender_profile_did) = &presence_sender {
            self.presence_receive_rate.record(sender_profile_did, now);
        }
        Ok(CollaborationTransportIngestion::Incoming(
            transport_incoming_acceptance(accepted),
        ))
    }

    fn ingest_transport_acceptance_receipt(
        &self,
        frame: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> Result<CollaborationTransportIngestion, CollaborationTransportRetryableError> {
        let receipt = match self.verify_receipt_envelope(frame) {
            Ok(receipt) => receipt,
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::InvalidAcceptanceReceipt,
                ))
            }
        };
        let receipt_payload = &receipt.envelope().payload;
        if receipt_payload.recipient_endpoint_did == self.authority.local_device_did() {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::AcceptanceFromLocalDevice,
            ));
        }
        if receipt.accepting_endpoint_did() != source_endpoint_did {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::InvalidAcceptanceReceipt,
            ));
        }

        let mut state = self
            .load_state()
            .map_err(|_| CollaborationTransportRetryableError)?
            .unwrap_or_else(|| self.empty_state());
        self.prune_terminal_state(&mut state, now)
            .map_err(|_| CollaborationTransportRetryableError)?;
        let Some(entry) = state.outgoing.iter().find(|entry| {
            collaboration_message_envelope_sha256(entry.envelope.as_bytes())
                == receipt_payload.message_envelope_sha256
        }) else {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::AcceptanceWithoutOutgoingMessage,
            ));
        };
        let verified_message = self
            .verify_outgoing_record(entry)
            .map_err(|_| CollaborationTransportRetryableError)?;
        if verified_message.envelope().payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
            && self.is_bootstrap_endpoint(receipt.accepting_endpoint_did())
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::BootstrapEndpoint,
            ));
        }
        for existing_bytes in &entry.remote_acceptance_receipts {
            let existing = self
                .verify_receipt_envelope(existing_bytes.as_bytes())
                .map_err(|_| CollaborationTransportRetryableError)?;
            if existing.envelope().payload.recipient_endpoint_did
                == receipt_payload.recipient_endpoint_did
            {
                if existing_bytes.as_bytes() == frame && existing.envelope() == receipt.envelope() {
                    return Ok(CollaborationTransportIngestion::RemoteAcceptance(
                        CollaborationTransportRemoteAcceptance::Replayed,
                    ));
                }
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::AcceptanceRecipientConflict,
                ));
            }
        }
        if verify_collaboration_acceptance_receipt(frame, &verified_message, now).is_err() {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::InvalidAcceptanceReceipt,
            ));
        }
        if entry.local_product_projection != OutgoingProductProjectionState::Complete {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::AcceptanceBeforeProductProjection,
            ));
        }
        let redundant = !entry.remote_acceptance_receipts.is_empty();
        match self.record_remote_acceptance(frame, now) {
            Ok(()) => Ok(CollaborationTransportIngestion::RemoteAcceptance(
                CollaborationTransportRemoteAcceptance::Applied,
            )),
            Err(_) if redundant => Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::AcceptanceNotStored,
            )),
            Err(_) => Err(CollaborationTransportRetryableError),
        }
    }

    pub(crate) fn prepare_profile_outgoing(
        &self,
        operation: EspRequestBinding,
        sender_profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        payload_type: &str,
        payload: serde_json::Value,
        now: u64,
        ttl_secs: u64,
    ) -> anyhow::Result<DurableOutgoingMessage> {
        self.community_membership.require_joined()?;
        let authenticated_payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                sender_profile,
                payload.clone(),
            )?;
        validate_operation_for_intent(
            &operation,
            &self.operation_capsule,
            payload_type,
            &authenticated_payload,
            ttl_secs,
        )?;

        let rate_limited_sender = (payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE)
            .then(|| sender_profile.document().profile_did.as_str());
        self.with_mutation(Some(now), |state| {
            self.community_membership.require_joined()?;
            if let Some(existing) = state
                .outgoing
                .iter()
                .find(|entry| entry.operation.request_id == operation.request_id)
            {
                if existing.operation != operation {
                    anyhow::bail!(
                        "collaboration operation request_id was reused with another binding"
                    );
                }
                let verified = self.verify_outgoing_record(existing)?;
                if existing.local_product_projection == OutgoingProductProjectionState::Retired {
                    anyhow::bail!(
                        "collaboration operation was retired because its Shared product is invalid"
                    );
                }
                validate_operation_for_intent(
                    &operation,
                    &self.operation_capsule,
                    payload_type,
                    &authenticated_payload,
                    ttl_secs,
                )?;
                return Ok(Mutation {
                    value: self.outgoing_handle(existing, verified),
                    changed: false,
                });
            }

            // A retry of a stored request_id returned above and is not counted.
            if let Some(sender_profile_did) = rate_limited_sender {
                self.check_community_send_window(state, sender_profile_did, now)?;
            }
            let unresolved = state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.local_product_projection != OutgoingProductProjectionState::Retired
                        && entry.remote_acceptance_receipts.is_empty()
                })
                .count();
            if unresolved >= MAX_UNRESOLVED_OUTGOING || state.outgoing.len() >= MAX_OUTGOING_RECORDS
            {
                anyhow::bail!("collaboration outgoing capacity is exhausted");
            }

            let prepared = self.authority.prepare_profile_outgoing(
                sender_profile,
                self.authority.sender_service(),
                payload_type,
                payload.clone(),
                now,
                ttl_secs,
            )?;
            validate_shared_product_projection(
                &self.authorize_stored_product_message(prepared.envelope_bytes())?,
            )?;
            let envelope = String::from_utf8(prepared.envelope_bytes().to_vec())
                .context("canonical collaboration envelope is not UTF-8")?;
            if prepared.envelope_sha256() != prepared.verified_message().envelope_sha256() {
                anyhow::bail!("prepared collaboration message hash is inconsistent");
            }
            let record = OutgoingRecord {
                operation: operation.clone(),
                envelope,
                local_product_projection: OutgoingProductProjectionState::Pending,
                remote_acceptance_receipts: Vec::new(),
            };
            let value = self.outgoing_handle(&record, prepared.verified_message().clone());
            state.outgoing.push(record);
            ensure_state_fits(state)?;
            Ok(Mutation {
                value,
                changed: true,
            })
        })
    }

    /// Counts this Profile's stored Community messages created in the last
    /// window. Stored state is the count, so an unsaved send never counts and
    /// a saved one always does, across restarts too.
    fn check_community_send_window(
        &self,
        state: &CoreState,
        sender_profile_did: &str,
        now: u64,
    ) -> Result<(), CommunitySendRateLimited> {
        let window_start = now.saturating_sub(COMMUNITY_RATE_WINDOW_SECS);
        let recent = state
            .outgoing
            .iter()
            .filter(|entry| {
                entry.local_product_projection != OutgoingProductProjectionState::Retired
            })
            .filter_map(|entry| {
                serde_json::from_str::<SignedCollaborationMessage>(&entry.envelope).ok()
            })
            .filter(|message| {
                message.payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
                    && message.payload.sender_profile_did == sender_profile_did
                    && message.payload.created_at > window_start
            })
            .map(|message| message.payload.created_at);
        let (count, oldest) = recent.fold((0, u64::MAX), |(count, oldest), created_at| {
            (count + 1, oldest.min(created_at))
        });
        if count < self.sends_per_window {
            return Ok(());
        }
        Err(CommunitySendRateLimited {
            retry_after_secs: oldest
                .saturating_add(COMMUNITY_RATE_WINDOW_SECS)
                .saturating_sub(now)
                .max(1),
        })
    }

    #[cfg(test)]
    pub(crate) fn prepare_outgoing(
        &self,
        operation: EspRequestBinding,
        runtime_service: &str,
        payload_type: &str,
        payload: serde_json::Value,
        now: u64,
        ttl_secs: u64,
    ) -> anyhow::Result<DurableOutgoingMessage> {
        if runtime_service != self.authority.sender_service() {
            anyhow::bail!("Runtime-bound service does not match collaboration operation");
        }
        let profile = self.authority.sender_profile_for_test()?;
        self.prepare_profile_outgoing(operation, &profile, payload_type, payload, now, ttl_secs)
    }

    fn outgoing_pending(
        &self,
        entry: &OutgoingRecord,
        verified: &VerifiedCollaborationMessage,
        now: u64,
    ) -> bool {
        let payload = &verified.envelope().payload;
        entry.local_product_projection == OutgoingProductProjectionState::Complete
            && payload.expires_at > now
            && payload.created_at <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
            && (entry.remote_acceptance_receipts.is_empty()
                || payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE)
    }

    pub(crate) fn pending_outgoing(&self, now: u64) -> anyhow::Result<Vec<DurableOutgoingMessage>> {
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let mut pending = Vec::new();
        for entry in &state.outgoing {
            let verified = self.verify_outgoing_record(entry)?;
            if self.outgoing_pending(entry, &verified, now) {
                pending.push(self.outgoing_handle(entry, verified));
            }
        }
        Ok(pending)
    }

    /// Pure recovery of verified own-presence operation bindings. Repair stays
    /// in memory here; lifecycle/effect readers own its durable write.
    pub(crate) fn own_presence_operations(
        &self,
        now: u64,
    ) -> anyhow::Result<HashMap<String, EspRequestBinding>> {
        let (state, _) = self.load_state_raw()?;
        let mut operations = HashMap::new();
        if let Some(state) = state {
            for entry in &state.outgoing {
                let verified = self.verify_outgoing_record(entry)?;
                let payload = &verified.envelope().payload;
                if entry.local_product_projection != OutgoingProductProjectionState::Retired
                    && payload.payload_type == crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE
                    && payload.expires_at > now
                    && payload.created_at <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
                    && self
                        .authorize_stored_product_message(entry.envelope.as_bytes())?
                        .sender_profile()
                        .authorizes_endpoint(&self.local_device_did())
                {
                    operations.insert(
                        verified.envelope_sha256().to_string(),
                        entry.operation.clone(),
                    );
                }
            }
        }
        Ok(operations)
    }

    /// Terminal admission after live grant validation, immediately before
    /// Carrier dispatch. A copied handle loses admission when its queue entry
    /// is retired. This owns no lock during the network reply.
    pub(crate) fn pending_outgoing_contains(
        &self,
        envelope_hash: &str,
        now: u64,
    ) -> anyhow::Result<bool> {
        let (state, _) = self.load_state_raw()?;
        let Some(state) = state else { return Ok(false) };
        for entry in &state.outgoing {
            if collaboration_message_envelope_sha256(entry.envelope.as_bytes()) == envelope_hash {
                let verified = self.verify_outgoing_record(entry)?;
                return Ok(self.outgoing_pending(entry, &verified, now));
            }
        }
        Ok(false)
    }

    pub(crate) fn outgoing_chat_delivery(
        &self,
        reader_profile_did: &str,
    ) -> anyhow::Result<Vec<OutgoingChatDeliveryEvidence>> {
        // load_state verifies the namespace, original signatures, request
        // bindings and every stored receipt once for this response.
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let mut delivery = Vec::new();
        for entry in state.outgoing {
            if entry.local_product_projection == OutgoingProductProjectionState::Retired {
                continue;
            }
            let message: SignedCollaborationMessage = serde_json::from_str(&entry.envelope)?;
            if message.payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
                && message.payload.sender_profile_did == reader_profile_did
            {
                delivery.push(OutgoingChatDeliveryEvidence {
                    envelope_sha256: collaboration_message_envelope_sha256(
                        entry.envelope.as_bytes(),
                    ),
                    expires_at: message.payload.expires_at,
                    accepted: !entry.remote_acceptance_receipts.is_empty(),
                });
            }
        }
        Ok(delivery)
    }

    pub(crate) fn pending_outgoing_product_projections(
        &self,
        now: u64,
    ) -> anyhow::Result<Vec<PendingOutgoingProductProjection>> {
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        state
            .outgoing
            .iter()
            .filter(|entry| {
                entry.local_product_projection == OutgoingProductProjectionState::Pending
            })
            .filter_map(|entry| match self.verify_outgoing_record(entry) {
                Ok(verified)
                    if verified.envelope().payload.created_at
                        <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) =>
                {
                    Some(Ok(PendingOutgoingProductProjection {
                        outgoing: self.outgoing_handle(entry, verified),
                    }))
                }
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// Drops every queued presence announcement of `principal_id`, so a
    /// revoked, refused or hidden grant is never sent again. Presence is a
    /// short-lived announcement and keeps no replay record.
    pub(crate) fn retire_outgoing_presence(&self, principal_id: &str) -> anyhow::Result<usize> {
        self.with_mutation(None, |state| {
            let before = state.outgoing.len();
            state.outgoing.retain(|entry| {
                entry.operation.principal != principal_id
                    || serde_json::from_str::<SignedCollaborationMessage>(&entry.envelope).map_or(
                        true,
                        |message| {
                            message.payload.payload_type
                                != crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE
                        },
                    )
            });
            let retired = before - state.outgoing.len();
            Ok(Mutation {
                value: retired,
                changed: retired > 0,
            })
        })
    }

    /// Denial retires the exact own effect, preserving newer grant work.
    pub(crate) fn retire_outgoing_presence_operation(
        &self,
        envelope_hash: &str,
    ) -> anyhow::Result<()> {
        self.with_mutation(None, |state| {
            let before = state.outgoing.len();
            state.outgoing.retain(|entry| {
                collaboration_message_envelope_sha256(entry.envelope.as_bytes()) != envelope_hash
                    || self.verify_outgoing_record(entry).map_or(true, |verified| {
                        verified.envelope().payload.payload_type
                            != crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE
                    })
            });
            Ok(Mutation {
                value: (),
                changed: before != state.outgoing.len(),
            })
        })
    }

    pub(crate) fn acknowledge_outgoing_product_projection(
        &self,
        envelope_hash: &str,
    ) -> anyhow::Result<()> {
        validate_sha256_label(envelope_hash)?;
        self.with_mutation(None, |state| {
            let entry = state
                .outgoing
                .iter_mut()
                .find(|entry| {
                    collaboration_message_envelope_sha256(entry.envelope.as_bytes())
                        == envelope_hash
                })
                .context("pending outgoing collaboration product projection was not found")?;
            self.verify_outgoing_record(entry)?;
            if entry.local_product_projection == OutgoingProductProjectionState::Retired {
                anyhow::bail!("retired collaboration intent cannot complete a product projection");
            }
            if entry.local_product_projection == OutgoingProductProjectionState::Complete {
                return Ok(Mutation {
                    value: (),
                    changed: false,
                });
            }
            entry.local_product_projection = OutgoingProductProjectionState::Complete;
            ensure_state_fits(state)?;
            Ok(Mutation {
                value: (),
                changed: true,
            })
        })
    }

    pub(crate) fn record_remote_acceptance(
        &self,
        receipt_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<()> {
        let receipt = self.verify_receipt_envelope(receipt_bytes)?;
        let message_hash = receipt.envelope().payload.message_envelope_sha256.clone();
        let recipient_did = receipt.envelope().payload.recipient_endpoint_did.clone();
        if recipient_did == self.authority.local_device_did() {
            anyhow::bail!("local device cannot remotely accept its own outgoing message");
        }

        self.with_mutation(Some(now), |state| {
            let entry = state
                .outgoing
                .iter_mut()
                .find(|entry| {
                    collaboration_message_envelope_sha256(entry.envelope.as_bytes()) == message_hash
                })
                .context("acceptance receipt does not match a persisted outgoing message")?;
            let verified_message = self.verify_outgoing_record(entry)?;
            if verified_message.envelope().payload.payload_type
                == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
            {
                self.community_membership.require_joined()?;
                if self.is_bootstrap_endpoint(&recipient_did) {
                    anyhow::bail!("a Community bootstrap endpoint cannot accept a Shared post");
                }
            }
            verify_collaboration_acceptance_receipt(receipt_bytes, &verified_message, now)?;

            for existing in &entry.remote_acceptance_receipts {
                let existing = self.verify_receipt_envelope(existing.as_bytes())?;
                if existing.envelope().payload.recipient_endpoint_did == recipient_did {
                    if existing.envelope() == receipt.envelope()
                        && entry
                            .remote_acceptance_receipts
                            .iter()
                            .any(|bytes| bytes.as_bytes() == receipt_bytes)
                    {
                        return Ok(Mutation {
                            value: (),
                            changed: false,
                        });
                    }
                    anyhow::bail!("recipient device reused an acceptance receipt inconsistently");
                }
            }
            if entry.remote_acceptance_receipts.len() >= MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING {
                anyhow::bail!("remote acceptance receipt capacity is exhausted");
            }
            if entry.local_product_projection != OutgoingProductProjectionState::Complete {
                anyhow::bail!("remote acceptance cannot precede the local product projection");
            }
            entry.remote_acceptance_receipts.push(
                String::from_utf8(receipt_bytes.to_vec())
                    .context("canonical collaboration acceptance receipt is not UTF-8")?,
            );
            ensure_state_fits(state)?;
            Ok(Mutation {
                value: (),
                changed: true,
            })
        })
    }

    pub(crate) fn accept_incoming(
        &self,
        envelope_bytes: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<DurableIncomingAcceptance> {
        let authorized = self.authorize_stored_product_message(envelope_bytes)?;
        validate_shared_product_projection(&authorized)?;
        let envelope_hash = collaboration_message_envelope_sha256(envelope_bytes);
        if let Some(state) = self.load_state()? {
            if let Some(existing) =
                self.exact_incoming_replay(&state, envelope_bytes, &envelope_hash, now)?
            {
                return Ok(existing);
            }
        }
        let incoming =
            self.authority
                .authorize_incoming(envelope_bytes, source_endpoint_did, now)?;
        if incoming.message().envelope().signer_did == self.authority.local_device_did() {
            anyhow::bail!("local collaboration message echo is not an incoming acceptance");
        }

        self.with_mutation(Some(now), |state| {
            if incoming.message().envelope().payload.payload_type
                == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
            {
                self.community_membership.require_joined()?;
            }
            if let Some(existing) =
                self.exact_incoming_replay(state, envelope_bytes, &envelope_hash, now)?
            {
                return Ok(Mutation {
                    value: existing,
                    changed: false,
                });
            }
            self.reject_incoming_conflicts(state, incoming.message())?;
            self.ensure_incoming_capacity(state, incoming.message(), envelope_bytes)?;

            let receipt_bytes = self.authority.prepare_acceptance_receipt(&incoming, now)?;
            let record = IncomingRecord {
                envelope: String::from_utf8(envelope_bytes.to_vec())
                    .context("canonical collaboration envelope is not UTF-8")?,
                acceptance_receipt: String::from_utf8(receipt_bytes.clone())
                    .context("canonical collaboration acceptance receipt is not UTF-8")?,
                product_handoff: ProductHandoffState::Pending,
            };
            state.incoming.push(record);
            ensure_state_fits(state)?;
            Ok(Mutation {
                value: DurableIncomingAcceptance {
                    authorized: incoming,
                    acceptance_receipt_bytes: receipt_bytes,
                    product_handoff_pending: true,
                },
                changed: true,
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn accept_incoming_from_signed_source_for_test(
        &self,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<DurableIncomingAcceptance> {
        let source_endpoint_did =
            serde_json::from_slice::<SignedCollaborationMessage>(envelope_bytes)
                .context("test collaboration message is malformed")?
                .signer_did;
        self.accept_incoming(envelope_bytes, &source_endpoint_did, now)
    }

    pub(crate) fn pending_product_handoffs(&self) -> anyhow::Result<Vec<PendingProductHandoff>> {
        self.with_mutation(None, |state| {
            // A separately changed history sidecar can invalidate a legacy
            // handoff even when the exact Core-state cache still hits.
            let changed = self.retire_history_conflicting_handoffs(state)?;
            let value = state
                .incoming
                .iter()
                .map(|entry| {
                    let authorized = self.verify_incoming_record(entry)?;
                    Ok(PendingProductHandoff { authorized })
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Mutation { value, changed })
        })
    }

    fn retire_history_conflicting_handoffs(&self, state: &mut CoreState) -> anyhow::Result<bool> {
        if state.incoming.is_empty() {
            return Ok(false);
        }
        let (history, _) = self.load_history_raw()?;
        let mut conflicting = Vec::new();
        for (index, entry) in state.incoming.iter().enumerate() {
            let authorized = self.verify_incoming_record(entry)?;
            if self.history_identity_conflicts(history.as_ref(), authorized.message())? {
                let receipt = self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
                conflicting.push((
                    index,
                    tombstone_retention_deadline(receipt.envelope().payload.accepted_at),
                ));
            }
        }
        let changed = !conflicting.is_empty();
        for (index, retain_until) in conflicting.into_iter().rev() {
            let entry = state.incoming.remove(index);
            state.incoming_tombstones.push(IncomingTombstone {
                acceptance_receipt: entry.acceptance_receipt,
                retain_until,
            });
        }
        Ok(changed)
    }

    fn repair_malformed_product_handoffs(&self, state: &mut CoreState) -> anyhow::Result<bool> {
        let mut changed = false;
        for entry in &mut state.outgoing {
            let invalid = validate_shared_product_projection(
                &self.authorize_stored_product_message(entry.envelope.as_bytes())?,
            )
            .is_err();
            if entry.local_product_projection == OutgoingProductProjectionState::Retired && !invalid
            {
                anyhow::bail!("retired collaboration intent has a valid Shared product");
            }
            if entry.local_product_projection != OutgoingProductProjectionState::Retired && invalid
            {
                entry.local_product_projection = OutgoingProductProjectionState::Retired;
                changed = true;
            }
        }
        let mut malformed = Vec::new();
        for (index, entry) in state.incoming.iter().enumerate() {
            let authorized = self.verify_incoming_record(entry)?;
            if validate_shared_product_projection(&authorized).is_err() {
                let receipt = self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
                malformed.push((
                    index,
                    tombstone_retention_deadline(receipt.envelope().payload.accepted_at),
                ));
            }
        }
        changed |= !malformed.is_empty();
        for (index, retain_until) in malformed.into_iter().rev() {
            let entry = state.incoming.remove(index);
            state.incoming_tombstones.push(IncomingTombstone {
                acceptance_receipt: entry.acceptance_receipt,
                retain_until,
            });
        }
        Ok(changed)
    }

    pub(crate) fn acknowledge_product_handoff(&self, envelope_hash: &str) -> anyhow::Result<()> {
        validate_sha256_label(envelope_hash)?;
        // A persisted pending record always wins over the in-memory presence
        // set, and presence is remembered only after its removal is durable.
        let acknowledged_presence = self.with_mutation(None, |state| {
            let Some(index) = state.incoming.iter().position(|entry| {
                collaboration_message_envelope_sha256(entry.envelope.as_bytes()) == envelope_hash
            }) else {
                if self.recent_presence_entry(envelope_hash).is_some()
                    || state.incoming_tombstones.iter().any(|entry| {
                        self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())
                            .map(|receipt| {
                                receipt.envelope().payload.message_envelope_sha256 == envelope_hash
                            })
                            .unwrap_or(false)
                    })
                {
                    return Ok(Mutation {
                        value: None,
                        changed: false,
                    });
                }
                anyhow::bail!("pending collaboration product handoff was not found");
            };
            if state.incoming.len() + state.incoming_tombstones.len()
                > MAX_INCOMING_RECORDS_AND_TOMBSTONES
            {
                anyhow::bail!("collaboration replay tombstone capacity is exhausted");
            }
            let record = state.incoming.remove(index);
            let verified = self.verify_incoming_record(&record)?;
            let receipt = self.verify_receipt_for_message(
                record.acceptance_receipt.as_bytes(),
                verified.message(),
            )?;
            let accepted_at = receipt.envelope().payload.accepted_at;
            let payload = &verified.message().envelope().payload;
            let value =
                if payload.payload_type == crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE {
                    Some((record.acceptance_receipt, payload.expires_at, accepted_at))
                } else {
                    state.incoming_tombstones.push(IncomingTombstone {
                        acceptance_receipt: record.acceptance_receipt,
                        retain_until: tombstone_retention_deadline(accepted_at),
                    });
                    None
                };
            ensure_state_fits(state)?;
            Ok(Mutation {
                value,
                changed: true,
            })
        })?;
        if let Some((receipt, expires_at, accepted_at)) = acknowledged_presence {
            self.remember_presence(envelope_hash, receipt, expires_at, accepted_at);
        }
        Ok(())
    }

    fn recent_presence_entry(&self, envelope_hash: &str) -> Option<(String, u64)> {
        self.recent_presence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(envelope_hash)
            .cloned()
    }

    /// Keeps an acknowledged presence until its expiry plus skew. The receive
    /// limit keeps this to a few per sender; a full table forgets the oldest,
    /// which at worst projects a still-live presence once more.
    fn remember_presence(&self, envelope_hash: &str, receipt: String, expires_at: u64, now: u64) {
        let mut recent = self
            .recent_presence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.retain(|_, (_, until)| *until >= now);
        if recent.len() >= MAX_RECENT_PRESENCE {
            if let Some(oldest) = recent
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(hash, _)| hash.clone())
            {
                recent.remove(&oldest);
            }
        }
        recent.insert(
            envelope_hash.to_string(),
            (
                receipt,
                expires_at.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS * 2),
            ),
        );
    }

    fn exact_incoming_replay(
        &self,
        state: &CoreState,
        envelope_bytes: &[u8],
        envelope_hash: &str,
        now: u64,
    ) -> anyhow::Result<Option<DurableIncomingAcceptance>> {
        if let Some(entry) = state
            .incoming
            .iter()
            .find(|entry| entry.envelope.as_bytes() == envelope_bytes)
        {
            let authorized = self.verify_incoming_record(entry)?;
            return Ok(Some(DurableIncomingAcceptance {
                authorized,
                acceptance_receipt_bytes: entry.acceptance_receipt.as_bytes().to_vec(),
                product_handoff_pending: true,
            }));
        }
        if let Some((receipt, until)) = self.recent_presence_entry(envelope_hash) {
            if until >= now {
                let authorized = self.authorize_stored_product_message(envelope_bytes)?;
                self.verify_receipt_for_message(receipt.as_bytes(), authorized.message())?;
                return Ok(Some(DurableIncomingAcceptance {
                    authorized,
                    acceptance_receipt_bytes: receipt.into_bytes(),
                    product_handoff_pending: false,
                }));
            }
        }
        if let Some(entry) = state.incoming_tombstones.iter().find(|entry| {
            entry.retain_until >= now
                && self
                    .verify_receipt_envelope(entry.acceptance_receipt.as_bytes())
                    .map(|receipt| {
                        receipt.envelope().payload.message_envelope_sha256 == envelope_hash
                    })
                    .unwrap_or(false)
        }) {
            let authorized = self.authorize_stored_product_message(envelope_bytes)?;
            let verified = authorized.message();
            if verified.envelope().signer_did == self.authority.local_device_did() {
                anyhow::bail!("persisted incoming replay is a local self-echo");
            }
            self.verify_receipt_for_message(
                entry.acceptance_receipt.as_bytes(),
                authorized.message(),
            )?;
            return Ok(Some(DurableIncomingAcceptance {
                authorized,
                acceptance_receipt_bytes: entry.acceptance_receipt.as_bytes().to_vec(),
                product_handoff_pending: false,
            }));
        }
        Ok(None)
    }

    /// Holds a frame refused by a per-sender limit for this Home's own retry.
    /// A sender past its held allowance is flooding and loses the frame; a
    /// Home full of fair holders keeps it in transport instead.
    fn hold_refused(
        &self,
        transport_frame: &[u8],
        message: &VerifiedCollaborationMessage,
        reason: CollaborationTransportRejection,
        now: u64,
    ) -> Result<CollaborationTransportIngestion, CollaborationTransportRetryableError> {
        let payload = &message.envelope().payload;
        match self.held.hold(
            HeldFrame::new(
                transport_frame,
                message.envelope_sha256(),
                &payload.sender_profile_did,
                payload.expires_at,
            ),
            now,
        ) {
            HoldOutcome::Held => {}
            HoldOutcome::Dropped => {
                tracing::debug!(
                    ?reason,
                    "Community message dropped: sender is over its limits"
                );
            }
            HoldOutcome::HomeFull => return Err(CollaborationTransportRetryableError),
        }
        Ok(CollaborationTransportIngestion::Rejected(reason))
    }

    /// Frames refused by a per-sender limit, oldest first, for the driver to
    /// retry before it reads new ones.
    pub(crate) fn take_held_frames(&self, now: u64) -> Vec<HeldFrame> {
        self.held.take(now)
    }

    /// Returns held frames that a retry did not settle.
    pub(crate) fn restore_held_frames(&self, held: Vec<HeldFrame>) {
        self.held.restore(held);
    }

    /// Release or requeue the reservation for one settled retry attempt.
    pub(crate) fn complete_held_frame(&self, frame: HeldFrame) {
        self.held.complete(frame);
    }

    fn sender_backlog_full(
        &self,
        state: &CoreState,
        message: &VerifiedCollaborationMessage,
    ) -> bool {
        let sender = &message.envelope().payload.sender_profile_did;
        state
            .incoming
            .iter()
            .filter_map(|entry| self.verify_incoming_record(entry).ok())
            .filter(|entry| entry.message().envelope().payload.sender_profile_did == *sender)
            .count()
            >= MAX_PENDING_INCOMING_PER_SENDER
    }

    fn ensure_incoming_capacity(
        &self,
        state: &CoreState,
        message: &VerifiedCollaborationMessage,
        envelope_bytes: &[u8],
    ) -> anyhow::Result<()> {
        if state.incoming.len() >= MAX_PENDING_INCOMING
            || state.incoming.len() + state.incoming_tombstones.len()
                >= MAX_INCOMING_RECORDS_AND_TOMBSTONES
        {
            anyhow::bail!("collaboration incoming capacity is exhausted");
        }
        if self.sender_backlog_full(state, message) {
            anyhow::bail!("collaboration incoming sender capacity is exhausted");
        }
        let current_bytes = canonical_state_bytes(state)?.len();
        let reserved = envelope_bytes
            .len()
            .saturating_mul(6)
            .saturating_add(MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES.saturating_mul(6))
            .saturating_add(1_024);
        if current_bytes.saturating_add(reserved) > MAX_CORE_STATE_BYTES {
            anyhow::bail!("collaboration durable state byte capacity is exhausted");
        }
        Ok(())
    }

    fn verify_outgoing_record(
        &self,
        entry: &OutgoingRecord,
    ) -> anyhow::Result<VerifiedCollaborationMessage> {
        let authorized = self.authorize_stored_product_message(entry.envelope.as_bytes())?;
        let verified = authorized.message().clone();
        if verified.envelope().signer_did != self.authority.local_device_did() {
            anyhow::bail!("persisted outgoing message is not signed by the local device");
        }
        validate_operation_for_message(&entry.operation, &self.operation_capsule, &verified)?;
        let mut recipients = HashSet::new();
        for receipt_bytes in &entry.remote_acceptance_receipts {
            let receipt = self.verify_receipt_for_message(receipt_bytes.as_bytes(), &verified)?;
            let recipient = &receipt.envelope().payload.recipient_endpoint_did;
            if recipient == &self.authority.local_device_did() {
                anyhow::bail!("persisted remote acceptance is signed by the local device");
            }
            if !recipients.insert(recipient.clone()) {
                anyhow::bail!("persisted outgoing message has duplicate recipient acceptances");
            }
        }
        Ok(verified)
    }

    fn verify_incoming_record(
        &self,
        entry: &IncomingRecord,
    ) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
        if entry.product_handoff != ProductHandoffState::Pending {
            anyhow::bail!("persisted incoming product handoff is invalid");
        }
        let receipt = self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
        let accepted_at = receipt.envelope().payload.accepted_at;
        let authorized = self.authorize_stored_product_message(entry.envelope.as_bytes())?;
        let verified = authorized.message();
        if verified.envelope().signer_did == self.authority.local_device_did() {
            anyhow::bail!("persisted incoming message is a local self-echo");
        }
        self.verify_receipt_for_message(entry.acceptance_receipt.as_bytes(), authorized.message())?;
        if receipt.envelope().payload.recipient_endpoint_did != self.authority.local_device_did()
            || accepted_at > verified.envelope().payload.expires_at
        {
            anyhow::bail!("persisted incoming acceptance is not the local durable acceptance");
        }
        Ok(authorized)
    }

    fn with_mutation<T>(
        &self,
        prune_now: Option<u64>,
        f: impl FnOnce(&mut CoreState) -> anyhow::Result<Mutation<T>>,
    ) -> anyhow::Result<T> {
        let _process_guard = self
            .mutation_mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration mutation mutex is poisoned"))?;
        self.ensure_state_directory()?;
        let _file_guard = lock_owner_only_file(&self.lock_path())?;
        // This reader does not reacquire the process/file locks. On a cold or
        // externally changed snapshot it verifies and repairs only once.
        let (state, repaired) = self.load_state_raw()?;
        let mut state = state.unwrap_or_else(|| self.empty_state());
        if repaired {
            // Retirement is a completed repair, even if the requested operation
            // below refuses. Keep the original replay bindings durable first.
            self.persist_verified_state(&state)?;
        }
        let pruned = prune_now
            .map(|now| self.prune_terminal_state(&mut state, now))
            .transpose()?
            .unwrap_or(false);
        let mutation = f(&mut state)?;
        if mutation.changed || pruned {
            self.persist_verified_state(&state)?;
        }
        Ok(mutation.value)
    }

    fn persist_verified_state(&self, state: &CoreState) -> anyhow::Result<()> {
        self.validate_state(state)?;
        // An indeterminate rename must be read and validated again.
        *self
            .verified_state
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration state verification cache poisoned"))? =
            None;
        self.write_state(state)?;
        *self
            .verified_state
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration state verification cache poisoned"))? =
            Some((canonical_state_bytes(state)?, state.clone()));
        Ok(())
    }

    fn prune_terminal_state(&self, state: &mut CoreState, now: u64) -> anyhow::Result<bool> {
        let mut delivery_by_profile = HashMap::<String, Vec<OutgoingChatDeliveryEvidence>>::new();
        for entry in &state.outgoing {
            let authorized = self.authorize_stored_product_message(entry.envelope.as_bytes())?;
            let message = authorized.message();
            let payload = &message.envelope().payload;
            if entry.local_product_projection == OutgoingProductProjectionState::Complete
                && payload.payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
                && payload
                    .expires_at
                    .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
                    < now
            {
                self.verify_outgoing_record(entry)?;
                delivery_by_profile
                    .entry(payload.sender_profile_did.clone())
                    .or_default()
                    .push(OutgoingChatDeliveryEvidence {
                        envelope_sha256: message.envelope_sha256().to_string(),
                        expires_at: payload.expires_at,
                        accepted: !entry.remote_acceptance_receipts.is_empty(),
                    });
            }
        }
        for (profile, delivery) in delivery_by_profile {
            crate::room_service::persist_collaboration_delivery(
                &self.data_root,
                self.conversation_scope(),
                &profile,
                &delivery,
                now,
            )?;
        }
        prune_terminal_state(state, now)
    }

    fn load_state(&self) -> anyhow::Result<Option<CoreState>> {
        let (state, repaired) = self.load_state_raw()?;
        if repaired {
            // Re-read under both writer locks; only the fsynced repair becomes
            // an exact verified-state cache entry.
            return self.with_mutation(None, |state| {
                Ok(Mutation {
                    value: Some(state.clone()),
                    changed: false,
                })
            });
        }
        Ok(state)
    }

    fn load_state_raw(&self) -> anyhow::Result<(Option<CoreState>, bool)> {
        if !self.validate_existing_state_ancestors()? {
            return Ok((None, false));
        }
        let path = self.state_path();
        let named_before = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((None, false)),
            Err(err) => return Err(err.into()),
        };
        validate_owner_only_regular_file(&path, &named_before)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&path)
            .with_context(|| format!("failed to open collaboration state {}", path.display()))?;
        let metadata = file.metadata()?;
        validate_owner_only_regular_file(&path, &metadata)?;
        anyhow::ensure!(
            same_state_file(&named_before, &metadata),
            "collaboration state changed during open"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                metadata.nlink() == 1,
                "collaboration state has multiple links"
            );
        }
        if metadata.len() as usize > MAX_CORE_STATE_BYTES {
            anyhow::bail!("collaboration durable state exceeds its byte limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::take(&mut file, MAX_CORE_STATE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= MAX_CORE_STATE_BYTES,
            "collaboration durable state exceeds its byte limit"
        );
        let fd_after = file.metadata()?;
        let named_after = fs::symlink_metadata(&path)?;
        validate_owner_only_regular_file(&path, &fd_after)?;
        validate_owner_only_regular_file(&path, &named_after)?;
        anyhow::ensure!(
            same_state_file(&metadata, &fd_after)
                && same_state_file(&fd_after, &named_after)
                && bytes.len() as u64 == fd_after.len()
                && self.validate_existing_state_ancestors()?,
            "collaboration state changed during read"
        );
        {
            let mut cached = self
                .verified_state
                .lock()
                .map_err(|_| anyhow::anyhow!("collaboration state verification cache poisoned"))?;
            if let Some((exact, state)) = cached.as_ref() {
                if exact == &bytes {
                    return Ok((Some(state.clone()), false));
                }
            }
            *cached = None;
        }
        let mut state: CoreState =
            serde_json::from_slice(&bytes).context("invalid collaboration durable state")?;
        if canonical_state_bytes(&state)? != bytes {
            anyhow::bail!("collaboration durable state is not canonical JSON");
        }
        self.validate_state(&state)?;
        let repaired = self.repair_malformed_product_handoffs(&mut state)?;
        if repaired {
            return Ok((Some(state), true));
        }
        // A cold read can find exact bytes from an indeterminate rename.
        // Persist their directory entries once before the cache admits retries.
        self.sync_owned_state_directories(
            #[cfg(test)]
            None,
        )?;
        *self
            .verified_state
            .lock()
            .map_err(|_| anyhow::anyhow!("collaboration state verification cache poisoned"))? =
            Some((bytes, state.clone()));
        Ok((Some(state), false))
    }

    fn empty_history(&self) -> ConversationHistoryState {
        ConversationHistoryState {
            schema: HISTORY_STATE_SCHEMA.to_string(),
            binding: self.state_binding(),
            envelopes: Vec::new(),
            retired_envelope_hashes: Vec::new(),
            pending_projection_hashes: Vec::new(),
        }
    }

    fn history_path(&self) -> PathBuf {
        self.state_dir.join(HISTORY_STATE_FILE)
    }

    fn load_history(&self) -> anyhow::Result<Option<ConversationHistoryState>> {
        let (state, repaired) = self.load_history_raw()?;
        if repaired {
            return self.with_mutation(None, |_| {
                let (state, repaired) = self.load_history_raw()?;
                if repaired {
                    self.write_history(state.as_ref().context("history repair lost its source")?)?;
                }
                Ok(Mutation {
                    value: state,
                    changed: false,
                })
            });
        }
        Ok(state)
    }

    fn load_history_raw(&self) -> anyhow::Result<(Option<ConversationHistoryState>, bool)> {
        if !self.validate_existing_state_ancestors()? {
            return Ok((None, false));
        }
        let path = self.history_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, false)),
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
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        validate_owner_only_regular_file(&path, &metadata)?;
        if metadata.len() as usize > MAX_CONVERSATION_HISTORY_BYTES {
            anyhow::bail!("collaboration retained history exceeds its byte limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_CONVERSATION_HISTORY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CONVERSATION_HISTORY_BYTES {
            anyhow::bail!("collaboration retained history exceeds its byte limit");
        }
        let mut state: ConversationHistoryState = serde_json::from_slice(&bytes)?;
        if canonical_history_bytes(&state)? != bytes {
            anyhow::bail!("collaboration retained history is not canonical JSON");
        }
        self.validate_history(&state)?;
        let mut repaired = false;
        for envelope in &state.envelopes {
            let hash = collaboration_message_envelope_sha256(envelope.as_bytes());
            if !state.retired_envelope_hashes.contains(&hash)
                && crate::collaboration_product::validate_chat_projection(
                    &self.authorize_stored_product_message(envelope.as_bytes())?,
                )
                .is_err()
            {
                state.retired_envelope_hashes.push(hash);
                repaired = true;
            }
        }
        if repaired {
            prune_history(&mut state, now_secs())?;
        }
        Ok((Some(state), repaired))
    }

    /// The bounded original cache can evict a message while its verified live
    /// acceptance remains. Historical admission preserves that sender identity;
    /// only the exact accepted hash can replay it.
    fn validate_history_live_identities_in_state(
        &self,
        live: &CoreState,
        envelopes: &[Vec<u8>],
    ) -> anyhow::Result<()> {
        let (ids, nonces) = self.live_history_identities(live)?;
        for envelope in envelopes {
            let authorized = self.authorize_stored_product_message(envelope)?;
            if identity_index_conflicts(&ids, &nonces, authorized.message()) {
                anyhow::bail!("collaboration history sender reused a live message ID or nonce");
            }
        }
        Ok(())
    }

    fn live_history_identities(
        &self,
        live: &CoreState,
    ) -> anyhow::Result<(IncomingIdentityIndex, IncomingIdentityIndex)> {
        let mut ids = HashMap::new();
        let mut nonces = HashMap::new();
        for entry in &live.incoming {
            let authorized = self.verify_incoming_record(entry)?;
            let message = authorized.message();
            insert_incoming_identity(
                &mut ids,
                &mut nonces,
                &message.envelope().payload,
                message.envelope_sha256(),
            )?;
        }
        for entry in &live.incoming_tombstones {
            let receipt = self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
            insert_receipt_identity(&mut ids, &mut nonces, &receipt.envelope().payload)?;
        }
        Ok((ids, nonces))
    }

    fn validate_history_identities(&self, state: &ConversationHistoryState) -> anyhow::Result<()> {
        let mut ids = HashMap::new();
        let mut nonces = HashMap::new();
        for envelope in &state.envelopes {
            let authorized = self.authorize_stored_product_message(envelope.as_bytes())?;
            let message = authorized.message();
            insert_incoming_identity(
                &mut ids,
                &mut nonces,
                &message.envelope().payload,
                message.envelope_sha256(),
            )?;
        }
        Ok(())
    }

    fn validate_history(&self, state: &ConversationHistoryState) -> anyhow::Result<()> {
        if state.schema != HISTORY_STATE_SCHEMA
            || state.binding != self.state_binding()
            || state.envelopes.len() > MAX_CONVERSATION_HISTORY_MESSAGES
            || canonical_history_bytes(state)?.len() > MAX_CONVERSATION_HISTORY_BYTES
        {
            anyhow::bail!("collaboration retained history has an invalid binding or bounds");
        }
        let retired = state
            .retired_envelope_hashes
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let pending = state
            .pending_projection_hashes
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if retired.len() != state.retired_envelope_hashes.len()
            || retired.len() > state.envelopes.len()
            || pending.len() != state.pending_projection_hashes.len()
            || pending.len() > state.envelopes.len()
            || pending.iter().any(|hash| retired.contains(hash))
        {
            anyhow::bail!("collaboration retained history has invalid retirement markers");
        }
        let mut matched = 0;
        let mut pending_matched = 0;
        for envelope in &state.envelopes {
            let hash = collaboration_message_envelope_sha256(envelope.as_bytes());
            if retired.contains(hash.as_str()) {
                matched += 1;
                let authorized = self.authorize_stored_product_message(envelope.as_bytes())?;
                if crate::collaboration_product::validate_chat_projection(&authorized).is_ok() {
                    anyhow::bail!("retired history original has a valid Shared product");
                }
            }
            if pending.contains(hash.as_str()) {
                pending_matched += 1;
                crate::collaboration_product::validate_chat_projection(
                    &self.authorize_stored_product_message(envelope.as_bytes())?,
                )?;
            }
        }
        if matched != retired.len() {
            anyhow::bail!("retired history hash has no matching original");
        }
        anyhow::ensure!(
            pending_matched == pending.len(),
            "pending history hash has no matching original"
        );
        self.validate_history_identities(state)
    }

    fn write_history(&self, state: &ConversationHistoryState) -> anyhow::Result<()> {
        self.validate_history(state)?;
        let bytes = canonical_history_bytes(state)?;
        #[cfg(test)]
        let fault = self
            .history_write_fault
            .swap(0, std::sync::atomic::Ordering::SeqCst);
        #[cfg(test)]
        if fault == WriteFault::BeforeWrite as u8 {
            anyhow::bail!("injected history failure before write");
        }
        let temp = self
            .state_dir
            .join(format!(".{HISTORY_STATE_FILE}.{}.tmp", random_hex_128()?));
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
            #[cfg(test)]
            if fault == WriteFault::AfterFileSync as u8 {
                anyhow::bail!("injected history failure after file sync");
            }
            validate_owner_only_regular_file(&temp, &file.metadata()?)?;
            if let Ok(metadata) = fs::symlink_metadata(self.history_path()) {
                validate_owner_only_regular_file(&self.history_path(), &metadata)?;
            }
            fs::rename(&temp, self.history_path())?;
            renamed = true;
            #[cfg(test)]
            if fault == WriteFault::AfterRename as u8 {
                anyhow::bail!("history durability is indeterminate after rename");
            }
            self.sync_owned_state_directories(
                #[cfg(test)]
                (fault == WriteFault::BeforeAncestorSync as u8)
                    .then_some(WriteFault::BeforeAncestorSync),
            )?;
            Ok(())
        })();
        if result.is_err() && !renamed {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn validate_existing_state_ancestors(&self) -> anyhow::Result<bool> {
        let data_root = self.data_root.as_path();
        match fs::symlink_metadata(data_root) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => anyhow::bail!("collaboration data root must be a real directory"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err.into()),
        }
        for path in [
            data_root.join("collaboration"),
            data_root.join("collaboration/default-conversation"),
            self.state_dir.clone(),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => validate_owner_only_directory(&path, &metadata)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err.into()),
            }
        }
        Ok(true)
    }

    fn validate_state(&self, state: &CoreState) -> anyhow::Result<()> {
        if state.schema != CORE_STATE_SCHEMA || state.binding != self.state_binding() {
            anyhow::bail!("collaboration durable state binding or schema mismatch");
        }
        if state.outgoing.len() > MAX_OUTGOING_RECORDS
            || state.incoming.len() > MAX_PENDING_INCOMING
            || state.incoming.len() + state.incoming_tombstones.len()
                > MAX_INCOMING_RECORDS_AND_TOMBSTONES
            || state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.local_product_projection != OutgoingProductProjectionState::Retired
                        && entry.remote_acceptance_receipts.is_empty()
                })
                .count()
                > MAX_UNRESOLVED_OUTGOING
        {
            anyhow::bail!("collaboration durable state exceeds its entry limits");
        }

        let mut request_ids = HashSet::new();
        for entry in &state.outgoing {
            if !request_ids.insert(entry.operation.request_id.as_str()) {
                anyhow::bail!("collaboration durable state has duplicate operation request IDs");
            }
            if entry.remote_acceptance_receipts.len() > MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING {
                anyhow::bail!("collaboration durable state has too many remote acceptances");
            }
            if entry.local_product_projection == OutgoingProductProjectionState::Pending
                && !entry.remote_acceptance_receipts.is_empty()
            {
                anyhow::bail!(
                    "collaboration durable state has acceptance before local product projection"
                );
            }
            self.verify_outgoing_record(entry)?;
        }

        let mut message_ids = HashMap::new();
        let mut nonces = HashMap::new();
        let mut per_sender = HashMap::<String, usize>::new();
        for entry in &state.incoming {
            let authorized = self.verify_incoming_record(entry)?;
            let message = &authorized.message().envelope().payload;
            let hash = authorized.message().envelope_sha256().to_string();
            insert_incoming_identity(&mut message_ids, &mut nonces, message, &hash)?;
            *per_sender
                .entry(message.sender_profile_did.clone())
                .or_default() += 1;
        }
        if per_sender
            .values()
            .any(|count| *count > MAX_PENDING_INCOMING_PER_SENDER)
        {
            anyhow::bail!("collaboration durable state exceeds its per-sender limit");
        }
        for entry in &state.incoming_tombstones {
            let receipt = self.verify_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
            let payload = &receipt.envelope().payload;
            if payload.network_id != self.authority.network_id()
                || payload.conversation_id != self.authority.grant().grant().conversation_id
                || payload.recipient_endpoint_did != self.authority.local_device_did()
                || entry.retain_until != tombstone_retention_deadline(payload.accepted_at)
            {
                anyhow::bail!("collaboration replay tombstone binding is invalid");
            }
            insert_receipt_identity(&mut message_ids, &mut nonces, payload)?;
        }
        Ok(())
    }

    fn empty_state(&self) -> CoreState {
        CoreState {
            schema: CORE_STATE_SCHEMA.to_string(),
            binding: self.state_binding(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
            incoming_tombstones: Vec::new(),
        }
    }

    fn state_binding(&self) -> CoreStateBinding {
        CoreStateBinding {
            network_id: self.authority.network_id().to_string(),
            grant_cid: self.authority.grant_cid().to_string(),
            local_device_did: self.authority.local_device_did(),
            operation_capsule: self.operation_capsule.clone(),
        }
    }

    fn ensure_state_directory(&self) -> anyhow::Result<()> {
        let data_root = self.data_root.as_path();
        let root_metadata =
            fs::symlink_metadata(data_root).context("collaboration data root does not exist")?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            anyhow::bail!("collaboration data root must be a real directory");
        }
        let collaboration = data_root.join("collaboration");
        let conversation = collaboration.join("default-conversation");
        ensure_owner_only_directory(&collaboration)?;
        ensure_owner_only_directory(&conversation)?;
        ensure_owner_only_directory(&self.state_dir)?;
        Ok(())
    }

    fn sync_owned_state_directories(
        &self,
        #[cfg(test)] fault: Option<WriteFault>,
    ) -> anyhow::Result<()> {
        let mut directory = self.state_dir.as_path();
        loop {
            anyhow::ensure!(
                directory.starts_with(&self.data_root),
                "collaboration directory sync escaped its data root"
            );
            File::open(directory)?.sync_all()?;
            #[cfg(test)]
            self.directory_syncs
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(test)]
            if directory == self.state_dir && fault == Some(WriteFault::BeforeAncestorSync) {
                anyhow::bail!(
                    "collaboration directory durability is indeterminate before ancestor sync"
                );
            }
            if directory == self.data_root {
                return Ok(());
            }
            directory = directory
                .parent()
                .context("collaboration directory sync has no data root ancestor")?;
        }
    }

    fn state_path(&self) -> PathBuf {
        self.state_dir.join(CORE_STATE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.state_dir.join(CORE_LOCK_FILE)
    }

    fn write_state(&self, state: &CoreState) -> anyhow::Result<()> {
        let bytes = canonical_state_bytes(state)?;
        if bytes.len() > MAX_CORE_STATE_BYTES {
            anyhow::bail!("collaboration durable state exceeds its byte limit");
        }
        #[cfg(test)]
        let write_fault = self.take_write_fault();
        #[cfg(test)]
        if write_fault == Some(WriteFault::BeforeWrite) {
            anyhow::bail!("injected collaboration state failure before write");
        }

        let temp = self
            .state_dir
            .join(format!(".{CORE_STATE_FILE}.{}.tmp", random_hex_128()?));
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
            #[cfg(test)]
            if write_fault == Some(WriteFault::AfterFileSync) {
                anyhow::bail!("injected collaboration state failure after file sync");
            }
            if let Ok(metadata) = fs::symlink_metadata(self.state_path()) {
                validate_owner_only_regular_file(&self.state_path(), &metadata)?;
            }
            fs::rename(&temp, self.state_path())?;
            renamed = true;
            #[cfg(test)]
            if write_fault == Some(WriteFault::AfterRename) {
                anyhow::bail!("collaboration state durability is indeterminate after rename");
            }
            self.sync_owned_state_directories(
                #[cfg(test)]
                write_fault,
            )?;
            Ok(())
        })();
        if result.is_err() && !renamed {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    #[cfg(test)]
    pub(crate) fn inject_write_fault(&self, fault: WriteFault) {
        self.write_fault
            .store(fault as u8, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_history_write_fault(&self, fault: WriteFault) {
        self.history_write_fault
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
            4 => Some(WriteFault::BeforeAncestorSync),
            _ => None,
        }
    }
}

fn validate_operation_for_message(
    operation: &EspRequestBinding,
    operation_capsule: &str,
    message: &VerifiedCollaborationMessage,
) -> anyhow::Result<()> {
    let payload = &message.envelope().payload;
    validate_operation_for_intent(
        operation,
        operation_capsule,
        &payload.payload_type,
        &payload.payload,
        payload.expires_at - payload.created_at,
    )
}

fn validate_operation_for_intent(
    operation: &EspRequestBinding,
    operation_capsule: &str,
    payload_type: &str,
    payload: &serde_json::Value,
    ttl_secs: u64,
) -> anyhow::Result<()> {
    if operation.schema != ESP_REQUEST_BINDING_SCHEMA
        || !valid_request_id(&operation.request_id)
        || operation.capsule != operation_capsule
        || operation.method != DEFAULT_CONVERSATION_SEND_METHOD
    {
        anyhow::bail!("collaboration operation binding context is invalid");
    }
    let intent = serde_json::json!({
        "payload_type": payload_type,
        "payload": payload,
        "ttl_secs": ttl_secs,
    });
    let expected = esp_request_binding(
        &operation.request_id,
        &operation.principal,
        &operation.capsule,
        operation.interface.as_deref(),
        &operation.method,
        operation.resources.clone(),
        &intent,
    );
    if &expected != operation {
        anyhow::bail!("collaboration operation binding does not match its canonical intent");
    }
    Ok(())
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn gossip_payload_type(payload_type: &str) -> bool {
    payload_type == crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE
        || payload_type == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE
}

fn collaboration_transport_frame_kind(
    frame: &[u8],
) -> Result<CollaborationTransportFrameKind, CollaborationTransportRejection> {
    if frame.is_empty() || frame.len() > MAX_COLLABORATION_ENVELOPE_BYTES {
        return Err(CollaborationTransportRejection::MalformedOrNoncanonicalFrame);
    }
    let value: serde_json::Value = serde_json::from_slice(frame)
        .map_err(|_| CollaborationTransportRejection::MalformedOrNoncanonicalFrame)?;
    if serde_json::to_vec(&value).ok().as_deref() != Some(frame) {
        return Err(CollaborationTransportRejection::MalformedOrNoncanonicalFrame);
    }
    let schema = value
        .as_object()
        .and_then(|envelope| envelope.get("payload"))
        .and_then(serde_json::Value::as_object)
        .and_then(|payload| payload.get("schema"))
        .and_then(serde_json::Value::as_str)
        .ok_or(CollaborationTransportRejection::MalformedOrNoncanonicalFrame)?;
    match schema {
        COLLABORATION_MESSAGE_SCHEMA_V1 => Ok(CollaborationTransportFrameKind::Message),
        COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1 => {
            Ok(CollaborationTransportFrameKind::AcceptanceReceipt)
        }
        _ => Err(CollaborationTransportRejection::UnsupportedSchema),
    }
}

fn validate_shared_product_projection(
    authorized: &AuthorizedDefaultConversationMessage,
) -> anyhow::Result<()> {
    match authorized
        .message()
        .envelope()
        .payload
        .payload_type
        .as_str()
    {
        COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE => {
            crate::collaboration_product::validate_chat_projection(authorized)?;
        }
        crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE => {
            crate::collaboration_presence::validate_presence_payload(authorized.product_payload())?;
        }
        _ => anyhow::bail!("Shared collaboration payload type has no registered product consumer"),
    }
    Ok(())
}

fn transport_incoming_acceptance(
    accepted: DurableIncomingAcceptance,
) -> CollaborationTransportIncomingAcceptance {
    CollaborationTransportIncomingAcceptance {
        acceptance_receipt_bytes: accepted.acceptance_receipt_bytes,
        product_handoff_pending: accepted.product_handoff_pending,
    }
}

fn identity_index_conflicts(
    ids: &IncomingIdentityIndex,
    nonces: &IncomingIdentityIndex,
    message: &VerifiedCollaborationMessage,
) -> bool {
    let payload = &message.envelope().payload;
    [
        ids.get(&(
            payload.sender_profile_did.clone(),
            payload.message_id.clone(),
        )),
        nonces.get(&(payload.sender_profile_did.clone(), payload.nonce.clone())),
    ]
    .into_iter()
    .flatten()
    .any(|hash| hash != message.envelope_sha256())
}

fn incoming_identity_conflicts(
    state: &CoreState,
    message: &VerifiedCollaborationMessage,
    verify_receipt: impl Fn(&[u8]) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt>,
) -> anyhow::Result<bool> {
    let candidate = &message.envelope().payload;
    for entry in &state.incoming {
        let envelope: SignedCollaborationMessage = serde_json::from_str(&entry.envelope)?;
        if envelope.payload.sender_profile_did == candidate.sender_profile_did
            && (envelope.payload.message_id == candidate.message_id
                || envelope.payload.nonce == candidate.nonce)
        {
            return Ok(true);
        }
    }
    for entry in &state.incoming_tombstones {
        let receipt = verify_receipt(entry.acceptance_receipt.as_bytes())?;
        let receipt = &receipt.envelope().payload;
        if receipt.sender_profile_did == candidate.sender_profile_did
            && (receipt.message_id == candidate.message_id
                || receipt.message_nonce == candidate.nonce)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn insert_incoming_identity(
    message_ids: &mut HashMap<(String, String), String>,
    nonces: &mut HashMap<(String, String), String>,
    message: &elastos_common::collaboration_protocol::CollaborationMessage,
    envelope_hash: &str,
) -> anyhow::Result<()> {
    if message_ids
        .insert(
            (
                message.sender_profile_did.clone(),
                message.message_id.clone(),
            ),
            envelope_hash.to_string(),
        )
        .is_some()
        || nonces
            .insert(
                (message.sender_profile_did.clone(), message.nonce.clone()),
                envelope_hash.to_string(),
            )
            .is_some()
    {
        anyhow::bail!("collaboration durable state has conflicting sender identities");
    }
    Ok(())
}

fn insert_receipt_identity(
    message_ids: &mut HashMap<(String, String), String>,
    nonces: &mut HashMap<(String, String), String>,
    receipt: &elastos_common::collaboration_protocol::CollaborationAcceptanceReceipt,
) -> anyhow::Result<()> {
    if message_ids
        .insert(
            (
                receipt.sender_profile_did.clone(),
                receipt.message_id.clone(),
            ),
            receipt.message_envelope_sha256.clone(),
        )
        .is_some()
        || nonces
            .insert(
                (
                    receipt.sender_profile_did.clone(),
                    receipt.message_nonce.clone(),
                ),
                receipt.message_envelope_sha256.clone(),
            )
            .is_some()
    {
        anyhow::bail!("collaboration durable state has conflicting replay tombstones");
    }
    Ok(())
}

fn outgoing_handle(
    entry: &OutgoingRecord,
    verified: VerifiedCollaborationMessage,
    verify_receipt: impl Fn(&[u8]) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt>,
) -> DurableOutgoingMessage {
    DurableOutgoingMessage {
        envelope_bytes: entry.envelope.as_bytes().to_vec(),
        envelope_sha256: verified.envelope_sha256().to_string(),
        created_at: verified.envelope().payload.created_at,
        shared_chat: verified.envelope().payload.payload_type
            == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
        accepted_endpoints: entry
            .remote_acceptance_receipts
            .iter()
            .filter_map(|bytes| {
                verify_receipt(bytes.as_bytes())
                    .ok()
                    .map(|receipt| receipt.accepting_endpoint_did().to_string())
            })
            .collect(),
        presence_sender: (verified.envelope().payload.payload_type
            == crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE)
            .then(|| {
                (
                    entry.operation.principal.clone(),
                    verified.envelope().payload.sender_profile_did.clone(),
                )
            }),
    }
}

fn state_namespace(authority: &DefaultConversationDeviceAuthority) -> String {
    let mut hasher = Sha256::new();
    for field in [
        CORE_NAMESPACE_DOMAIN,
        authority.network_id().as_bytes(),
        authority.grant_cid().as_bytes(),
        authority.local_device_did().as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hex::encode(hasher.finalize())
}

fn same_state_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.uid() != after.uid()
            || before.mode() != after.mode()
            || before.nlink() != after.nlink()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return false;
        }
    }
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

fn canonical_state_bytes(state: &CoreState) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(state)?)?)
}

fn ensure_state_fits(state: &CoreState) -> anyhow::Result<()> {
    if canonical_state_bytes(state)?.len() > MAX_CORE_STATE_BYTES {
        anyhow::bail!("collaboration durable state exceeds its byte limit");
    }
    Ok(())
}

fn validate_sha256_label(value: &str) -> anyhow::Result<()> {
    if value.len() != "sha256:".len() + 64
        || !value.starts_with("sha256:")
        || !value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("collaboration envelope hash is invalid");
    }
    Ok(())
}

fn prune_terminal_state(state: &mut CoreState, now: u64) -> anyhow::Result<bool> {
    let outgoing_before = state.outgoing.len();
    let tombstones_before = state.incoming_tombstones.len();
    let mut retained_outgoing = Vec::with_capacity(state.outgoing.len());
    for entry in state.outgoing.drain(..) {
        let expires_at = stored_message_expiry(&entry.envelope)?;
        // Transport expiry cannot discard a saved intent before its consumer
        // records the local projection or acknowledges an expired Presence.
        if entry.local_product_projection == OutgoingProductProjectionState::Pending
            || expires_at.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) >= now
        {
            retained_outgoing.push(entry);
        }
    }
    state.outgoing = retained_outgoing;
    state
        .incoming_tombstones
        .retain(|entry| entry.retain_until >= now);
    Ok(state.outgoing.len() != outgoing_before
        || state.incoming_tombstones.len() != tombstones_before)
}

fn canonical_history_bytes(state: &ConversationHistoryState) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(state)?)?)
}

fn append_history_envelopes(
    state: &mut ConversationHistoryState,
    envelopes: &[Vec<u8>],
) -> anyhow::Result<()> {
    let mut hashes = state
        .envelopes
        .iter()
        .map(|entry| collaboration_message_envelope_sha256(entry.as_bytes()))
        .collect::<HashSet<_>>();
    for envelope in envelopes {
        if hashes.insert(collaboration_message_envelope_sha256(envelope)) {
            state
                .envelopes
                .push(std::str::from_utf8(envelope)?.to_string());
        }
    }
    Ok(())
}

fn prune_history(state: &mut ConversationHistoryState, now: u64) -> anyhow::Result<()> {
    let mut retained = Vec::with_capacity(state.envelopes.len());
    for envelope in state.envelopes.drain(..) {
        let message: SignedCollaborationMessage = serde_json::from_str(&envelope)?;
        if message
            .payload
            .created_at
            .saturating_add(CONVERSATION_HISTORY_RETENTION_SECS)
            > now
        {
            retained.push((
                message.payload.created_at,
                collaboration_message_envelope_sha256(envelope.as_bytes()),
                envelope,
            ));
        }
    }
    retained.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    state.envelopes = retained
        .into_iter()
        .map(|(_, _, envelope)| envelope)
        .collect();
    let retained_hashes = state
        .envelopes
        .iter()
        .map(|envelope| collaboration_message_envelope_sha256(envelope.as_bytes()))
        .collect::<HashSet<_>>();
    state
        .retired_envelope_hashes
        .retain(|hash| retained_hashes.contains(hash));
    state
        .pending_projection_hashes
        .retain(|hash| retained_hashes.contains(hash));
    while state.envelopes.len() > MAX_CONVERSATION_HISTORY_MESSAGES
        || canonical_history_bytes(state)?.len() > MAX_CONVERSATION_HISTORY_BYTES
    {
        if state.envelopes.is_empty() {
            anyhow::bail!("collaboration history binding exceeds its byte limit");
        }
        let removed = state.envelopes.remove(0);
        let hash = collaboration_message_envelope_sha256(removed.as_bytes());
        state
            .retired_envelope_hashes
            .retain(|retired| retired != &hash);
        state
            .pending_projection_hashes
            .retain(|pending| pending != &hash);
    }
    Ok(())
}

fn stored_message_expiry(envelope: &str) -> anyhow::Result<u64> {
    Ok(
        serde_json::from_str::<SignedCollaborationMessage>(envelope)?
            .payload
            .expires_at,
    )
}

fn tombstone_retention_deadline(accepted_at: u64) -> u64 {
    accepted_at
        .saturating_add(MAX_COLLABORATION_MESSAGE_LIFETIME_SECS)
        .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS * 2)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn random_hex_128() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .context("OS randomness unavailable for collaboration state")?;
    Ok(hex::encode(bytes))
}

pub(crate) fn ensure_owner_only_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_owner_only_directory(path, &metadata),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err.into()),
            }
            let metadata = fs::symlink_metadata(path)?;
            validate_owner_only_directory(path, &metadata)
        }
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn validate_owner_only_directory(
    path: &Path,
    metadata: &fs::Metadata,
) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "collaboration state directory is not a real directory: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, metadata, 0o077)
}

pub(crate) fn validate_owner_only_regular_file(
    path: &Path,
    metadata: &fs::Metadata,
) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "collaboration state path is not a regular file: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, metadata, 0o077)
}

fn validate_owner_and_mode(
    path: &Path,
    metadata: &fs::Metadata,
    forbidden_mode: u32,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & forbidden_mode != 0 {
            anyhow::bail!(
                "collaboration state path is not owner-only: {}",
                path.display()
            );
        }
    }
    Ok(())
}

/// Opens or creates an owner-only lock file and waits for its exclusive lock.
pub(crate) fn lock_owner_only_file(path: &Path) -> anyhow::Result<FileLock> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    validate_owner_only_regular_file(path, &file.metadata()?)?;
    Ok(FileLock::exclusive(file)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use elastos_common::collaboration_protocol::{
        canonical_collaboration_acceptance_receipt_bytes, canonical_collaboration_message_bytes,
        canonical_signed_collaboration_acceptance_receipt_bytes,
        canonical_signed_collaboration_message_bytes, CollaborationAcceptanceReceipt,
        CollaborationMessage, SignedCollaborationAcceptanceReceipt, SignedCollaborationMessage,
        COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
    };
    use elastos_runtime::signature::generate_keypair;

    use crate::collaboration_default_conversation::{
        canonical_default_conversation_grant_bytes, verify_default_conversation_grant,
        DefaultConversationAdmissionPolicy, DefaultConversationGrant,
        DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
    };
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, DefaultConversationGrantDescriptor,
        SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };

    const NETWORK: &str = "collaboration-core-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const OPERATION_CAPSULE: &str = "chat-room";
    const NOW: u64 = 1_800_000_000;
    const TTL: u64 = 300;

    struct Fixture {
        _temp: tempfile::TempDir,
        data_root: PathBuf,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
        device_key: SigningKey,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data_root = temp.path().join("data");
            fs::create_dir(&data_root).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&data_root, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let (profile_signer, _) = generate_keypair();
            let grant_bytes =
                canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
                    schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
                    network_id: NETWORK.to_string(),
                    conversation_id: CONVERSATION.to_string(),
                    sender_service: SERVICE.to_string(),
                    admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
                })
                .unwrap();
            let grant_cid = raw_sha256_cid(&grant_bytes);
            let profile = verified_profile(&profile_signer, NETWORK, grant_cid);
            let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
            let (device_key, _) = generate_keypair();
            Self {
                _temp: temp,
                data_root,
                profile,
                grant,
                device_key,
            }
        }

        fn core(&self) -> CollaborationCore {
            CollaborationCore::new(
                &self.data_root,
                self.device_key.clone(),
                self.profile.clone(),
                self.grant.clone(),
                OPERATION_CAPSULE,
            )
            .unwrap()
        }

        fn authority(&self, key: SigningKey) -> DefaultConversationDeviceAuthority {
            DefaultConversationDeviceAuthority::new(key, self.profile.clone(), self.grant.clone())
                .unwrap()
        }
    }

    fn raw_sha256_cid(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, multihash).to_string()
    }

    fn verified_profile(
        signing_key: &SigningKey,
        network_id: &str,
        grant_cid: String,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_signing_key_did(signing_key);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did: envelope_signer,
        };
        let bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        match validate_collaboration_network_profile(Some(&bytes), network_id, &[signer_did], None)
            .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn alternate_grant_core(
        fixture: &Fixture,
        device_key: SigningKey,
        conversation_id: &str,
    ) -> CollaborationCore {
        let (profile_signer, _) = generate_keypair();
        let grant_bytes = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: conversation_id.to_string(),
            sender_service: SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        })
        .unwrap();
        let profile = verified_profile(&profile_signer, NETWORK, raw_sha256_cid(&grant_bytes));
        let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
        CollaborationCore::new(
            &fixture.data_root,
            device_key,
            profile,
            grant,
            OPERATION_CAPSULE,
        )
        .unwrap()
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

    fn intent(payload_type: &str, payload: &serde_json::Value, ttl_secs: u64) -> serde_json::Value {
        serde_json::json!({
            "payload_type": payload_type,
            "payload": payload,
            "ttl_secs": ttl_secs,
        })
    }

    fn operation(
        core: &CollaborationCore,
        request_id: &str,
        payload_type: &str,
        payload: &serde_json::Value,
        ttl_secs: u64,
    ) -> EspRequestBinding {
        let profile = core.authority.sender_profile_for_test().unwrap();
        let payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                &profile,
                payload.clone(),
            )
            .unwrap();
        esp_request_binding(
            request_id,
            "runtime-principal",
            OPERATION_CAPSULE,
            Some("elastos.chat.room"),
            DEFAULT_CONVERSATION_SEND_METHOD,
            ["elastos://chat/message".to_string()],
            &intent(payload_type, &payload, ttl_secs),
        )
    }

    fn prepare(
        core: &CollaborationCore,
        request_id: &str,
        payload: serde_json::Value,
        now: u64,
        ttl_secs: u64,
    ) -> anyhow::Result<DurableOutgoingMessage> {
        let payload_type = "elastos.chat.message/v1";
        core.prepare_outgoing(
            operation(core, request_id, payload_type, &payload, ttl_secs),
            SERVICE,
            payload_type,
            payload,
            now,
            ttl_secs,
        )
    }

    #[test]
    fn community_leave_serializes_with_durable_admission_and_refuses_left_intents() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let fixture = Fixture::new();
        let core = Arc::new(fixture.core());
        prepare(
            &core,
            "accepted-before-leave",
            serde_json::json!({"body":"before"}),
            NOW,
            TTL,
        )
        .unwrap();
        let original = fs::read(core.state_path()).unwrap();
        let mutation = core.mutation_mutex.lock().unwrap();
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (left_tx, left_rx) = mpsc::sync_channel(0);
        let leaving = core.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = leaving.set_community_joined(false);
            left_tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            left_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(core.community_membership().joined());
        drop(mutation);
        left_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert!(!core.community_membership().joined());
        assert!(!fixture.core().community_membership().joined());
        assert_eq!(fs::read(core.state_path()).unwrap(), original);
        assert!(prepare(
            &core,
            "refused-while-left",
            serde_json::json!({"body":"left"}),
            NOW + 1,
            TTL,
        )
        .unwrap_err()
        .to_string()
        .contains(crate::collaboration_release_network::COMMUNITY_LEFT_DETAIL));
        assert_eq!(fs::read(core.state_path()).unwrap(), original);
        core.set_community_joined(true).unwrap();
        prepare(
            &core,
            "accepted-after-rejoin",
            serde_json::json!({"body":"rejoined"}),
            NOW + 2,
            TTL,
        )
        .unwrap();
        let state = core.load_state().unwrap().unwrap();
        assert_eq!(state.outgoing.len(), 2);
        assert!(state
            .outgoing
            .iter()
            .all(|record| record.operation.request_id != "refused-while-left"));
    }

    #[test]
    fn entered_home_delivery_and_receipt_writes_finish_before_leave() {
        use std::sync::{mpsc, Arc, TryLockError};
        use std::time::{Duration, Instant};

        for settlement in [false, true] {
            let fixture = Fixture::new();
            let core = Arc::new(fixture.core());
            let outgoing = prepare(
                &core,
                "local",
                serde_json::json!({"body":"local"}),
                NOW,
                TTL,
            )
            .unwrap();
            complete_projection(&core, &outgoing);
            let (remote_key, _) = generate_keypair();
            let remote = fixture.authority(remote_key.clone());
            let remote_did = remote.local_device_did();
            let bytes = if settlement {
                remote_receipt(&fixture, &outgoing, remote_key.clone(), NOW + 1)
            } else {
                remote_transport_message(&fixture, remote_key.clone(), NOW).1
            };
            let file = lock_owner_only_file(&core.lock_path()).unwrap();
            let (done_tx, done_rx) = mpsc::channel();
            let admission = core.clone();
            let bytes_copy = bytes.clone();
            let endpoint = remote_did.clone();
            let hash = outgoing.envelope_sha256().to_string();
            let write = std::thread::spawn(move || {
                let result = if settlement {
                    admission.record_home_chat_acceptance(&bytes_copy, &hash, &endpoint, NOW + 1)
                } else {
                    admission
                        .accept_home_chat(&bytes_copy, &endpoint, NOW + 1)
                        .map(|_| ())
                };
                done_tx.send(result).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match core.mutation_mutex.try_lock() {
                    Err(TryLockError::WouldBlock) => break,
                    Err(error) => panic!("mutation lock failed: {error}"),
                    Ok(guard) => drop(guard),
                }
                assert!(
                    Instant::now() < deadline,
                    "Home admission did not enter Core"
                );
                std::thread::yield_now();
            }
            let (left_tx, left_rx) = mpsc::channel();
            let leaving = core.clone();
            let leave = std::thread::spawn(move || {
                left_tx.send(leaving.set_community_joined(false)).unwrap();
            });
            assert!(matches!(
                left_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            assert!(core.community_membership().joined());
            drop(file);
            done_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            left_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            write.join().unwrap();
            leave.join().unwrap();
            let committed = fs::read(core.state_path()).unwrap();
            let retry = if settlement {
                core.record_home_chat_acceptance(
                    &bytes,
                    outgoing.envelope_sha256(),
                    &remote_did,
                    NOW + 1,
                )
            } else {
                core.accept_home_chat(&bytes, &remote_did, NOW + 1)
                    .map(|_| ())
            };
            assert!(retry.is_err());
            // The inner durable boundary also refuses an operation whose outer
            // Home admission already ran before Leave.
            let inner = if settlement {
                core.record_remote_acceptance(&bytes, NOW + 2)
            } else {
                let (_, fresh) = remote_message(&fixture, remote_key, NOW + 2);
                core.accept_incoming(&fresh, &remote_did, NOW + 2)
                    .map(|_| ())
            };
            assert!(inner.is_err());
            assert_eq!(fs::read(core.state_path()).unwrap(), committed);
        }
    }

    fn complete_projection(core: &CollaborationCore, outgoing: &DurableOutgoingMessage) {
        core.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
    }

    fn remote_message(fixture: &Fixture, key: SigningKey, now: u64) -> (SigningKey, Vec<u8>) {
        let authority = fixture.authority(key.clone());
        let prepared = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"remote"}),
                now,
                TTL,
            )
            .unwrap();
        (key, prepared.envelope_bytes().to_vec())
    }

    fn message_source_endpoint_did(envelope_bytes: &[u8]) -> String {
        serde_json::from_slice::<SignedCollaborationMessage>(envelope_bytes)
            .unwrap()
            .signer_did
    }

    fn transport_frame_envelope_bytes(frame_bytes: &[u8]) -> Vec<u8> {
        verify_collaboration_transport_frame(frame_bytes)
            .unwrap()
            .envelope_bytes()
            .to_vec()
    }

    fn wrap_transport_frame(fixture: &Fixture, key: &SigningKey, envelope_bytes: &[u8]) -> Vec<u8> {
        fixture
            .authority(key.clone())
            .prepare_transport_frame(envelope_bytes)
            .unwrap()
    }

    fn resign_message(key: &SigningKey, message: CollaborationMessage) -> Vec<u8> {
        let payload_bytes = canonical_collaboration_message_bytes(&message).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
            payload: message,
            signature,
            signer_did,
        })
        .unwrap()
    }

    fn shared_test_profile(
        authority: &DefaultConversationDeviceAuthority,
        device_key: &SigningKey,
    ) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        let original = authority.sender_profile_for_test().unwrap();
        let mut document = original.document().clone();
        for signer in &mut document.collaboration_signers {
            if signer.service == SERVICE {
                for delegation in &mut signer.delegations {
                    delegation
                        .object_types
                        .push("elastos.chat.unregistered/v1".to_string());
                    delegation.object_types.sort();
                }
            }
        }
        let signed = resign_test_profile(device_key, &document);
        crate::collaboration_profile_authority::verify_signed_profile_document(&signed).unwrap()
    }

    fn resign_test_profile(
        device_key: &SigningKey,
        document: &crate::collaboration_profile_authority::CollaborationProfileDocument,
    ) -> crate::collaboration_profile_authority::SignedCollaborationProfileDocument {
        let mut seed = device_key.to_bytes();
        seed[0] ^= 0xa5;
        seed[31] ^= 0x5a;
        let profile_key = SigningKey::from_bytes(&seed);
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &profile_key,
            crate::collaboration_profile_authority::COLLABORATION_PROFILE_DOCUMENT_SIGNATURE_DOMAIN_V1,
            &serde_json::to_vec(document).unwrap(),
        );
        serde_json::from_value(serde_json::json!({
            "payload": document, "signature": signature, "signer_did": signer_did,
        }))
        .unwrap()
    }

    fn profile_operation(
        request_id: &str,
        profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        payload_type: &str,
        product: &serde_json::Value,
    ) -> EspRequestBinding {
        let payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                profile,
                product.clone(),
            )
            .unwrap();
        esp_request_binding(
            request_id,
            "runtime-principal",
            OPERATION_CAPSULE,
            Some("elastos.chat.room"),
            DEFAULT_CONVERSATION_SEND_METHOD,
            ["elastos://chat/message".to_string()],
            &intent(payload_type, &payload, TTL),
        )
    }

    fn remote_receipt(
        fixture: &Fixture,
        outgoing: &DurableOutgoingMessage,
        key: SigningKey,
        accepted_at: u64,
    ) -> Vec<u8> {
        let authority = fixture.authority(key);
        let authorized = authority
            .authorize_incoming(
                outgoing.envelope_bytes(),
                &message_source_endpoint_did(outgoing.envelope_bytes()),
                accepted_at,
            )
            .unwrap();
        authority
            .prepare_acceptance_receipt(&authorized, accepted_at)
            .unwrap()
    }

    fn remote_transport_message(
        fixture: &Fixture,
        key: SigningKey,
        now: u64,
    ) -> (SigningKey, Vec<u8>) {
        let (key, envelope_bytes) = remote_message(fixture, key, now);
        let frame = wrap_transport_frame(fixture, &key, &envelope_bytes);
        (key, frame)
    }

    fn remote_transport_receipt(
        fixture: &Fixture,
        outgoing: &DurableOutgoingMessage,
        key: SigningKey,
        accepted_at: u64,
    ) -> Vec<u8> {
        let receipt = remote_receipt(fixture, outgoing, key.clone(), accepted_at);
        wrap_transport_frame(fixture, &key, &receipt)
    }

    fn sign_acceptance_receipt(
        key: &SigningKey,
        payload: CollaborationAcceptanceReceipt,
    ) -> Vec<u8> {
        let payload_bytes = canonical_collaboration_acceptance_receipt_bytes(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            key,
            COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_acceptance_receipt_bytes(
            &SignedCollaborationAcceptanceReceipt {
                payload,
                signature,
                signer_did,
            },
        )
        .unwrap()
    }

    // Synthetic signed state only. Construction/signing and writes stay outside
    // the measured reads; no installed Runtime data is accepted by this fixture.
    fn signed_validation_fixture(
        fixture: &Fixture,
        core: &CollaborationCore,
        tombstones: usize,
        outgoing: usize,
        payload_bytes: usize,
    ) -> CoreState {
        assert!(tombstones <= MAX_INCOMING_RECORDS_AND_TOMBSTONES);
        assert!(outgoing <= MAX_OUTGOING_RECORDS);
        assert!(payload_bytes <= crate::room_service::MAX_OBJECT_BODY_LEN);
        assert!(outgoing * (payload_bytes + 8192) + tombstones * 2048 <= MAX_CORE_STATE_BYTES);
        let (remote_key, _) = generate_keypair();
        let remote = fixture.authority(remote_key);
        let local_profile = core.authority.sender_profile_for_test().unwrap();
        let remote_profile = remote.sender_profile_for_test().unwrap();
        let payload = serde_json::json!({"body":"x".repeat(payload_bytes.max(1))});
        let mut state = core.empty_state();
        for index in 0..outgoing {
            let prepared = core
                .authority
                .prepare_profile_outgoing(
                    &local_profile,
                    SERVICE,
                    "elastos.chat.message/v1",
                    payload.clone(),
                    NOW,
                    TTL,
                )
                .unwrap();
            let authorized = remote
                .authorize_incoming(
                    prepared.envelope_bytes(),
                    &core.authority.local_device_did(),
                    NOW,
                )
                .unwrap();
            state.outgoing.push(OutgoingRecord {
                operation: operation(
                    core,
                    &format!("validation-bench:{index}"),
                    "elastos.chat.message/v1",
                    &payload,
                    TTL,
                ),
                envelope: String::from_utf8(prepared.envelope_bytes().to_vec()).unwrap(),
                local_product_projection: OutgoingProductProjectionState::Complete,
                remote_acceptance_receipts: vec![String::from_utf8(
                    remote.prepare_acceptance_receipt(&authorized, NOW).unwrap(),
                )
                .unwrap()],
            });
        }
        for _ in 0..tombstones {
            let prepared = remote
                .prepare_profile_outgoing(
                    &remote_profile,
                    SERVICE,
                    "elastos.chat.message/v1",
                    payload.clone(),
                    NOW,
                    TTL,
                )
                .unwrap();
            let authorized = core
                .authority
                .authorize_incoming(prepared.envelope_bytes(), &remote.local_device_did(), NOW)
                .unwrap();
            state.incoming_tombstones.push(IncomingTombstone {
                acceptance_receipt: String::from_utf8(
                    core.authority
                        .prepare_acceptance_receipt(&authorized, NOW)
                        .unwrap(),
                )
                .unwrap(),
                retain_until: tombstone_retention_deadline(NOW),
            });
        }
        core.validate_state(&state).unwrap();
        core.ensure_state_directory().unwrap();
        core.write_state(&state).unwrap();
        state
    }

    fn quiet_worker_validation_reads(core: &CollaborationCore) -> anyhow::Result<()> {
        // Chat and presence independently request the same outgoing projection
        // list, then transport retry reads outgoing, then both read handoffs.
        for _ in 0..2 {
            anyhow::ensure!(core.pending_outgoing_product_projections(NOW)?.is_empty());
        }
        anyhow::ensure!(core
            .pending_outgoing(NOW)?
            .iter()
            .all(DurableOutgoingMessage::remote_acceptance_recorded));
        for _ in 0..2 {
            anyhow::ensure!(core.pending_product_handoffs()?.is_empty());
        }
        Ok(())
    }

    fn assert_validation_fixture_rejects_tampering(
        fixture: &Fixture,
        core: &CollaborationCore,
        state: &CoreState,
    ) {
        let original = canonical_state_bytes(state).unwrap();
        // Change only one valid hex signature byte. Preserve JSON canonicality,
        // sizes, identity fields and authority bindings, including the last row.
        for outgoing_receipt in [false, true] {
            let mut damaged = state.clone();
            let receipt = if outgoing_receipt {
                let Some(entry) = damaged.outgoing.last_mut() else {
                    continue;
                };
                &mut entry.remote_acceptance_receipts[0]
            } else {
                let Some(entry) = damaged.incoming_tombstones.last_mut() else {
                    continue;
                };
                &mut entry.acceptance_receipt
            };
            let mut signed: SignedCollaborationAcceptanceReceipt =
                serde_json::from_str(receipt).unwrap();
            let replacement = if signed.signature.starts_with('0') {
                "1"
            } else {
                "0"
            };
            signed.signature.replace_range(..1, replacement);
            *receipt = String::from_utf8(
                canonical_signed_collaboration_acceptance_receipt_bytes(&signed).unwrap(),
            )
            .unwrap();
            write_owner_only(
                &core.state_path(),
                &canonical_state_bytes(&damaged).unwrap(),
            );
            assert!(quiet_worker_validation_reads(core).is_err());
            assert!(core.summary().is_err());
            assert!(fixture.core().summary().is_err());
            write_owner_only(&core.state_path(), &original);
            quiet_worker_validation_reads(core).unwrap();
        }
        if !state.outgoing.is_empty() {
            let mut damaged = state.clone();
            let envelope = &mut damaged.outgoing.last_mut().unwrap().envelope;
            let mut signed: SignedCollaborationMessage = serde_json::from_str(envelope).unwrap();
            let replacement = if signed.signature.starts_with('0') {
                "1"
            } else {
                "0"
            };
            signed.signature.replace_range(..1, replacement);
            *envelope =
                String::from_utf8(canonical_signed_collaboration_message_bytes(&signed).unwrap())
                    .unwrap();
            write_owner_only(
                &core.state_path(),
                &canonical_state_bytes(&damaged).unwrap(),
            );
            assert!(core.load_state().is_err());
            assert!(fixture.core().load_state().is_err());
            write_owner_only(&core.state_path(), &original);
            quiet_worker_validation_reads(core).unwrap();
        }
        if !state.incoming_tombstones.is_empty()
            && state.incoming_tombstones.len() < MAX_INCOMING_RECORDS_AND_TOMBSTONES
        {
            let mut duplicate = state.clone();
            duplicate
                .incoming_tombstones
                .push(state.incoming_tombstones[0].clone());
            write_owner_only(
                &core.state_path(),
                &canonical_state_bytes(&duplicate).unwrap(),
            );
            assert!(core.load_state().is_err()); // Cached signatures do not admit duplicate identities.
            write_owner_only(&core.state_path(), &original);
            quiet_worker_validation_reads(core).unwrap();
        }
    }

    #[test]
    fn verified_state_reuses_signatures_and_verifies_only_new_records() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let first = prepare(
            &core,
            "cache-first",
            serde_json::json!({"body":"first"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&core, &first);
        let first_misses = core
            .verification_misses
            .load(std::sync::atomic::Ordering::Relaxed);
        let first_directory_syncs = core
            .directory_syncs
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(first_misses, 1);
        for _ in 0..3 {
            core.load_state().unwrap();
            core.pending_outgoing(NOW).unwrap();
            core.pending_product_handoffs().unwrap();
        }
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            first_misses
        );
        assert_eq!(
            core.directory_syncs
                .load(std::sync::atomic::Ordering::Relaxed),
            first_directory_syncs
        );
        let second = prepare(
            &core,
            "cache-second",
            serde_json::json!({"body":"second"}),
            NOW + 11,
            TTL,
        )
        .unwrap();
        complete_projection(&core, &second);
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            first_misses + 1
        );
        let receipt = remote_receipt(&fixture, &second, generate_keypair().0, NOW + 12);
        core.record_remote_acceptance(&receipt, NOW + 12).unwrap();
        let receipt_misses = core
            .verification_misses
            .load(std::sync::atomic::Ordering::Relaxed);
        let receipt_directory_syncs = core
            .directory_syncs
            .load(std::sync::atomic::Ordering::Relaxed);
        assert!(receipt_misses > first_misses + 1);
        core.record_remote_acceptance(&receipt, NOW + 12).unwrap();
        core.load_state().unwrap();
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            receipt_misses
        );
        assert_eq!(
            core.directory_syncs
                .load(std::sync::atomic::Ordering::Relaxed),
            receipt_directory_syncs
        );
        let writer = fixture.core();
        let third = prepare(
            &writer,
            "cache-external-writer",
            serde_json::json!({"body":"external"}),
            NOW + 22,
            TTL,
        )
        .unwrap();
        complete_projection(&writer, &third);
        let state = core.load_state().unwrap().unwrap();
        assert_eq!(state.outgoing.len(), 3);
        assert!(state
            .outgoing
            .iter()
            .any(|entry| entry.envelope.as_bytes() == third.envelope_bytes()));
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            receipt_misses + 1
        );
        let original = canonical_state_bytes(&state).unwrap();
        for operation_tamper in [true, false] {
            let mut changed = state.clone();
            if operation_tamper {
                changed.outgoing[2].operation.sha256 = "00".repeat(32);
            } else {
                changed.binding.operation_capsule = "foreign-capsule".to_string();
            }
            // Signed envelopes remain byte-identical to the warm cache entries.
            assert_eq!(changed.outgoing[2].envelope, state.outgoing[2].envelope);
            write_owner_only(
                &core.state_path(),
                &canonical_state_bytes(&changed).unwrap(),
            );
            assert!(core.load_state().is_err());
            assert!(fixture.core().load_state().is_err());
            write_owner_only(&core.state_path(), &original);
            core.load_state().unwrap();
        }
        let cold = fixture.core();
        let state = cold.load_state().unwrap().unwrap();
        assert_eq!(state.outgoing.len(), 3);
        assert!(state
            .outgoing
            .iter()
            .any(|entry| entry.envelope.as_bytes() == first.envelope_bytes()));
        assert!(
            cold.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        );
    }

    #[test]
    fn verified_state_is_seeded_only_after_successful_fsync() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let first = prepare(
            &core,
            "before-rename-fault",
            serde_json::json!({"body":"first"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&core, &first);
        assert!(core.verified_state.lock().unwrap().is_some());
        core.inject_write_fault(WriteFault::AfterRename);
        assert!(prepare(
            &core,
            "after-rename-fault",
            serde_json::json!({"body":"second"}),
            NOW + 11,
            TTL
        )
        .is_err());
        assert!(core.verified_state.lock().unwrap().is_none());
        assert_eq!(core.load_state().unwrap().unwrap().outgoing.len(), 2);
        assert!(core.verified_state.lock().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn warm_state_cache_preserves_permission_symlink_and_hardlink_refusals() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = Fixture::new();
        let core = fixture.core();
        let first = prepare(
            &core,
            "warm-path-guards",
            serde_json::json!({"body":"first"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&core, &first);
        let path = core.state_path();
        let preserved = core.state_dir.join("preserved-state.json");
        fs::write(&preserved, fs::read(&path).unwrap()).unwrap();
        fs::set_permissions(&preserved, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&preserved, &path).unwrap();
        core.load_state().unwrap(); // Exact verified bytes in a new owned inode.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(core.load_state().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&path, &preserved).unwrap();
        symlink(&preserved, &path).unwrap();
        assert!(core.load_state().is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&preserved, &path).unwrap();
        assert!(core.load_state().is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(&preserved, &path).unwrap();
        core.load_state().unwrap();
    }

    #[test]
    fn signed_verification_cache_evicts_oldest_success_at_entry_bound() {
        let mut cache = SignedBytesCache::new();
        for i in 0..=MAX_VERIFICATION_CACHE_ENTRIES {
            cache.insert(&i.to_le_bytes(), i);
        }
        assert_eq!(cache.entries.len(), MAX_VERIFICATION_CACHE_ENTRIES);
        assert!(cache.get(&0usize.to_le_bytes()).is_none());
        assert_eq!(
            cache.get(&MAX_VERIFICATION_CACHE_ENTRIES.to_le_bytes()),
            Some(MAX_VERIFICATION_CACHE_ENTRIES)
        );
        assert!(cache.bytes <= MAX_CORE_STATE_BYTES);
        let largest = vec![1u8; MAX_CORE_STATE_BYTES];
        cache.insert(&largest, 1);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_CORE_STATE_BYTES);
        assert_eq!(cache.get(&largest), Some(1));
        assert!(cache
            .get(&MAX_VERIFICATION_CACHE_ENTRIES.to_le_bytes())
            .is_none());
        cache.insert(b"new", 2);
        assert!(cache.get(&largest).is_none());
        assert_eq!(cache.bytes, 3);
        let oversized = vec![3u8; MAX_CORE_STATE_BYTES + 1];
        cache.insert(&oversized, 3);
        assert!(cache.get(&oversized).is_none());
        assert_eq!(cache.get(b"new"), Some(2));
        assert_eq!(cache.bytes, 3);
    }

    #[test]
    fn signed_validation_fixture_keeps_noop_reads_and_ack_read_only_and_rejects_tampering() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let state = signed_validation_fixture(&fixture, &core, 3, 2, 128);
        let original = fs::read(core.state_path()).unwrap();
        core.inject_write_fault(WriteFault::BeforeWrite);
        quiet_worker_validation_reads(&core).unwrap();
        let hash = collaboration_message_envelope_sha256(state.outgoing[0].envelope.as_bytes());
        core.acknowledge_outgoing_product_projection(&hash).unwrap();
        core.acknowledge_outgoing_product_projection(&hash).unwrap();
        assert!(
            core.take_write_fault() == Some(WriteFault::BeforeWrite),
            "no-op paths attempted a state write"
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), original);
        assert_validation_fixture_rejects_tampering(&fixture, &core, &state);
    }

    #[test]
    fn new_incoming_frames_verify_only_new_records_beside_retained_replay_state() {
        let fixture = Fixture::new();
        let seeded = fixture.core();
        signed_validation_fixture(
            &fixture,
            &seeded,
            MAX_INCOMING_RECORDS_AND_TOMBSTONES - 2,
            0,
            0,
        );
        let core = fixture.core();
        core.load_state().unwrap();
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            MAX_INCOMING_RECORDS_AND_TOMBSTONES - 2
        );
        let remote_key = generate_keypair().0;
        for i in 0..2 {
            let (_, frame) = remote_transport_message(&fixture, remote_key.clone(), NOW + i);
            let before = core
                .verification_misses
                .load(std::sync::atomic::Ordering::Relaxed);
            assert!(matches!(
                core.ingest_transport_frame(&frame, NOW + i).unwrap(),
                CollaborationTransportIngestion::Incoming(_)
            ));
            let after = core
                .verification_misses
                .load(std::sync::atomic::Ordering::Relaxed);
            assert!(
                after > before && after - before <= 3,
                "old replay records were reverified for a new frame"
            );
            core.load_state().unwrap();
            core.summary().unwrap();
            assert_eq!(
                core.verification_misses
                    .load(std::sync::atomic::Ordering::Relaxed),
                after
            );
        }
        let state = core.load_state().unwrap().unwrap();
        assert_eq!(
            (state.incoming.len(), state.incoming_tombstones.len()),
            (2, MAX_INCOMING_RECORDS_AND_TOMBSTONES - 2)
        );
    }

    #[test]
    fn receipt_verification_cache_preserves_exact_original_binding() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let first = prepare(
            &core,
            "receipt-first",
            serde_json::json!({"body":"first"}),
            NOW,
            TTL,
        )
        .unwrap();
        let second = prepare(
            &core,
            "receipt-second",
            serde_json::json!({"body":"second"}),
            NOW,
            TTL,
        )
        .unwrap();
        let receipt = remote_receipt(&fixture, &first, generate_keypair().0, NOW);
        let first = core
            .authorize_stored_product_message(first.envelope_bytes())
            .unwrap();
        let second = core
            .authorize_stored_product_message(second.envelope_bytes())
            .unwrap();
        core.verify_receipt_envelope(&receipt).unwrap();
        core.verify_receipt_for_message(&receipt, first.message())
            .unwrap();
        let misses = core
            .verification_misses
            .load(std::sync::atomic::Ordering::Relaxed);
        core.verify_receipt_for_message(&receipt, first.message())
            .unwrap();
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            misses
        );
        for _ in 0..2 {
            assert!(core
                .verify_receipt_for_message(&receipt, second.message())
                .is_err());
        }
        assert_eq!(
            core.verification_misses
                .load(std::sync::atomic::Ordering::Relaxed),
            misses + 2
        );
    }

    // Run alone, after the shared-cache owner releases it. Use the same bounded
    // parameters with cargo test's normal profile and --release. No timing gate:
    // this reports validation cost, not an installed-idle or network benchmark.
    #[test]
    #[ignore = "opt-in signed-state validation CPU benchmark; reserve host compute first"]
    fn benchmark_signed_collaboration_idle_validation() {
        fn parameter(name: &str, default: usize, maximum: usize) -> usize {
            let value = match std::env::var(name) {
                Ok(raw) => raw
                    .parse::<usize>()
                    .expect("benchmark parameter must be an integer"),
                Err(std::env::VarError::NotPresent) => default,
                Err(_) => panic!("benchmark parameter must be Unicode"),
            };
            assert!(value <= maximum, "benchmark parameter exceeds its bound");
            value
        }
        fn cpu_micros() -> Option<u128> {
            #[cfg(unix)]
            {
                let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
                if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
                    return None;
                }
                let usage = unsafe { usage.assume_init() };
                Some(
                    (usage.ru_utime.tv_sec as u128 + usage.ru_stime.tv_sec as u128) * 1_000_000
                        + usage.ru_utime.tv_usec as u128
                        + usage.ru_stime.tv_usec as u128,
                )
            }
            #[cfg(not(unix))]
            {
                None
            }
        }
        let tombstones = parameter(
            "COLLAB_VALIDATION_BENCH_TOMBSTONES",
            64,
            MAX_INCOMING_RECORDS_AND_TOMBSTONES,
        );
        let outgoing = parameter("COLLAB_VALIDATION_BENCH_OUTGOING", 8, MAX_OUTGOING_RECORDS);
        let payload_bytes = parameter(
            "COLLAB_VALIDATION_BENCH_PAYLOAD_BYTES",
            256,
            crate::room_service::MAX_OBJECT_BODY_LEN,
        );
        let iterations = parameter("COLLAB_VALIDATION_BENCH_ITERATIONS", 3, 100);
        assert!(iterations > 0 && tombstones + outgoing > 0);
        let fixture = Fixture::new();
        let core = fixture.core();
        let state = signed_validation_fixture(&fixture, &core, tombstones, outgoing, payload_bytes);
        let original = fs::read(core.state_path()).unwrap();
        quiet_worker_validation_reads(&core).unwrap(); // Untimed warm read.
        core.inject_write_fault(WriteFault::BeforeWrite);
        let cpu_start = cpu_micros();
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            quiet_worker_validation_reads(&core).unwrap();
        }
        let wall_us = start.elapsed().as_micros();
        let cpu_us = cpu_micros().zip(cpu_start).map(|(end, start)| end - start);
        let mut noop_ack_wall_us = None;
        let mut noop_ack_cpu_us = None;
        if let Some(entry) = state.outgoing.first() {
            let hash = collaboration_message_envelope_sha256(entry.envelope.as_bytes());
            let cpu_start = cpu_micros();
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                core.acknowledge_outgoing_product_projection(&hash).unwrap();
            }
            noop_ack_wall_us = Some(start.elapsed().as_micros());
            noop_ack_cpu_us = cpu_micros().zip(cpu_start).map(|(end, start)| end - start);
        }
        assert!(
            core.take_write_fault() == Some(WriteFault::BeforeWrite),
            "measured paths attempted a state write"
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), original);
        assert_validation_fixture_rejects_tampering(&fixture, &core, &state);
        println!(
            "{}",
            serde_json::json!({
                "schema":"elastos.collaboration.validation-benchmark/v1",
                "synthetic_signed_state":true,"debug_assertions":cfg!(debug_assertions),
                "state_bytes":original.len(),"outgoing":outgoing,"incoming":0,
                "tombstones":tombstones,"acceptance_receipts":outgoing+tombstones,
                "payload_bytes":payload_bytes,"iterations":iterations,"validation_reads_per_iteration":5,
                "wall_us":wall_us,"process_cpu_us":cpu_us,
                "noop_ack_iterations":if outgoing > 0 {iterations} else {0},
                "noop_ack_wall_us":noop_ack_wall_us,"noop_ack_process_cpu_us":noop_ack_cpu_us,
                "measured_state_writes":0,"tampered_signature_rejected":true
            })
        );
    }

    #[test]
    fn chat_payload_is_validated_before_any_durable_acceptance() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let key = generate_keypair().0;
        let remote = fixture.authority(key.clone());
        for product in [
            serde_json::json!({}),
            serde_json::json!({"body":" "}),
            serde_json::json!({"body":" leading"}),
            serde_json::json!({"body":"valid","extra":true}),
            serde_json::json!({"body":"x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN + 1)}),
        ] {
            let original = remote
                .prepare_outgoing(
                    SERVICE,
                    COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                    product,
                    NOW,
                    TTL,
                )
                .unwrap();
            let frame = remote
                .prepare_transport_frame(original.envelope_bytes())
                .unwrap();
            assert_eq!(
                transport_rejection(&core, &frame, NOW),
                CollaborationTransportRejection::InvalidMessage
            );
            assert!(core
                .accept_home_chat(&frame, &remote.local_device_did(), NOW)
                .is_err());
            assert!(core
                .accept_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                .is_err());
            assert!(!core.state_path().exists());
        }
        let (_, valid) = remote_transport_message(&fixture, key, NOW);
        core.accept_home_chat(&valid, &remote.local_device_did(), NOW)
            .unwrap();
        assert_eq!(core.pending_product_handoffs().unwrap().len(), 1);
    }

    #[test]
    fn restart_retires_only_verified_malformed_chat_handoffs_and_frees_slots() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let mut state = core.empty_state();
        let mut malformed = Vec::new();
        for _ in 0..4 {
            let remote = fixture.authority(generate_keypair().0);
            for _ in 0..MAX_PENDING_INCOMING_PER_SENDER {
                let original = remote
                    .prepare_outgoing(
                        SERVICE,
                        COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                        serde_json::json!({}),
                        NOW,
                        TTL,
                    )
                    .unwrap();
                let incoming = core
                    .authority
                    .authorize_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                    .unwrap();
                let receipt = core
                    .authority
                    .prepare_acceptance_receipt(&incoming, NOW)
                    .unwrap();
                malformed.push(
                    remote
                        .prepare_transport_frame(original.envelope_bytes())
                        .unwrap(),
                );
                state.incoming.push(IncomingRecord {
                    envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
                    acceptance_receipt: String::from_utf8(receipt).unwrap(),
                    product_handoff: ProductHandoffState::Pending,
                });
            }
        }
        assert_eq!(state.incoming.len(), MAX_PENDING_INCOMING);
        core.validate_state(&state).unwrap();
        core.ensure_state_directory().unwrap();
        core.write_state(&state).unwrap();
        let mut corrupted = state.clone();
        let mut signed: SignedCollaborationMessage =
            serde_json::from_str(&corrupted.incoming[0].envelope).unwrap();
        signed.signature = "00".repeat(64);
        corrupted.incoming[0].envelope =
            String::from_utf8(canonical_signed_collaboration_message_bytes(&signed).unwrap())
                .unwrap();
        let corrupted_bytes = canonical_state_bytes(&corrupted).unwrap();
        write_owner_only(&core.state_path(), &corrupted_bytes);
        assert!(fixture.core().pending_product_handoffs().is_err());
        assert_eq!(fs::read(core.state_path()).unwrap(), corrupted_bytes);
        write_owner_only(&core.state_path(), &canonical_state_bytes(&state).unwrap());
        let restarted = fixture.core();
        assert!(restarted.pending_product_handoffs().unwrap().is_empty());
        let repaired = restarted.load_state().unwrap().unwrap();
        assert_eq!(repaired.incoming_tombstones.len(), MAX_PENDING_INCOMING);
        for entry in &state.incoming {
            assert!(repaired
                .incoming_tombstones
                .iter()
                .any(|retired| retired.acceptance_receipt == entry.acceptance_receipt));
        }
        assert_eq!(
            transport_rejection(&restarted, &malformed[0], NOW),
            CollaborationTransportRejection::InvalidMessage
        );
        let after_expiry = NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1;
        let (_, valid) = remote_transport_message(&fixture, generate_keypair().0, after_expiry);
        transport_incoming(&restarted, &valid, after_expiry);
        let presence = fixture.authority(generate_keypair().0);
        let metadata = presence
            .prepare_outgoing(
                SERVICE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                after_expiry,
                45,
            )
            .unwrap();
        restarted
            .accept_incoming(
                metadata.envelope_bytes(),
                &presence.local_device_did(),
                after_expiry,
            )
            .unwrap();
        assert_eq!(fixture.core().pending_product_handoffs().unwrap().len(), 2);
    }

    #[test]
    fn malformed_presence_cannot_fill_handoffs_and_legacy_restart_keeps_replay_protection() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let mut legacy = core.empty_state();
        let mut malformed_frames = Vec::new();
        let mut first_author = None;
        for _ in 0..4 {
            let key = generate_keypair().0;
            let remote = fixture.authority(key.clone());
            for _ in 0..MAX_PENDING_INCOMING_PER_SENDER {
                let original = remote
                    .prepare_outgoing(
                        SERVICE,
                        crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                        serde_json::json!({"extra":true}),
                        NOW,
                        45,
                    )
                    .unwrap();
                let frame = remote
                    .prepare_transport_frame(original.envelope_bytes())
                    .unwrap();
                assert_eq!(
                    transport_rejection(&core, &frame, NOW),
                    CollaborationTransportRejection::InvalidMessage
                );
                assert!(core
                    .accept_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                    .is_err());
                assert!(!core.state_path().exists());
                // Seed only the old, fully signed and receipt-bound on-disk shape.
                let incoming = core
                    .authority
                    .authorize_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                    .unwrap();
                let receipt = core
                    .authority
                    .prepare_acceptance_receipt(&incoming, NOW)
                    .unwrap();
                legacy.incoming.push(IncomingRecord {
                    envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
                    acceptance_receipt: String::from_utf8(receipt).unwrap(),
                    product_handoff: ProductHandoffState::Pending,
                });
                if first_author.is_none() {
                    first_author = Some(key.clone());
                }
                malformed_frames.push(frame);
            }
        }
        assert_eq!(legacy.incoming.len(), MAX_PENDING_INCOMING);
        core.validate_state(&legacy).unwrap();
        core.ensure_state_directory().unwrap();
        core.write_state(&legacy).unwrap();
        let mut invalid_receipt = legacy.clone();
        invalid_receipt.incoming[0].acceptance_receipt =
            legacy.incoming[1].acceptance_receipt.clone();
        let invalid_bytes = canonical_state_bytes(&invalid_receipt).unwrap();
        write_owner_only(&core.state_path(), &invalid_bytes);
        assert!(fixture.core().pending_product_handoffs().is_err());
        assert_eq!(fs::read(core.state_path()).unwrap(), invalid_bytes);
        write_owner_only(&core.state_path(), &canonical_state_bytes(&legacy).unwrap());

        let after_expiry = NOW + 45 + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1;
        let restarted = fixture.core();
        assert!(restarted.pending_product_handoffs().unwrap().is_empty());
        let repaired = restarted.load_state().unwrap().unwrap();
        assert_eq!(repaired.incoming_tombstones.len(), MAX_PENDING_INCOMING);
        for entry in &legacy.incoming {
            assert!(repaired
                .incoming_tombstones
                .iter()
                .any(|retired| retired.acceptance_receipt == entry.acceptance_receipt));
        }
        for frame in &malformed_frames {
            assert_eq!(
                transport_rejection(&restarted, frame, after_expiry),
                CollaborationTransportRejection::InvalidMessage
            );
        }
        // Correcting the DTO cannot reuse the retired author's MessageID or nonce.
        let original: SignedCollaborationMessage =
            serde_json::from_str(&legacy.incoming[0].envelope).unwrap();
        let mut corrected = original.payload;
        corrected.payload["product"] = serde_json::json!({});
        corrected.created_at = after_expiry;
        corrected.expires_at = after_expiry + 45;
        let corrected = resign_message(&first_author.unwrap(), corrected);
        assert!(restarted
            .accept_incoming(
                &corrected,
                &message_source_endpoint_did(&corrected),
                after_expiry
            )
            .unwrap_err()
            .to_string()
            .contains("reused"));
        let presence = fixture.authority(generate_keypair().0);
        let valid_presence = presence
            .prepare_outgoing(
                SERVICE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                after_expiry,
                45,
            )
            .unwrap();
        let valid_frame = presence
            .prepare_transport_frame(valid_presence.envelope_bytes())
            .unwrap();
        transport_incoming(&restarted, &valid_frame, after_expiry);
        let (_, valid_chat) =
            remote_transport_message(&fixture, generate_keypair().0, after_expiry);
        transport_incoming(&restarted, &valid_chat, after_expiry);
        let pending = fixture.core().pending_product_handoffs().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(
            fixture
                .core()
                .load_state()
                .unwrap()
                .unwrap()
                .incoming_tombstones
                .len(),
            MAX_PENDING_INCOMING
        );
    }

    #[test]
    fn shared_consumer_matrix_refuses_unconsumable_payloads_before_storage_and_repairs_on_load() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let cases = [
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"extra":true}),
            ),
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":7}),
            ),
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":"ok","extra":true}),
            ),
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":"x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN + 1)}),
            ),
            (
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({"extra":true}),
            ),
            (
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!([]),
            ),
            ("elastos.chat.direct-message/v1", serde_json::json!({})),
            ("elastos.chat.unregistered/v1", serde_json::json!({})),
        ];
        let local_profile = shared_test_profile(&core.authority, &fixture.device_key);
        let mut legacy = core.empty_state();
        let mut first = None;
        for author_index in 0..4 {
            let key = generate_keypair().0;
            let remote = fixture.authority(key.clone());
            let profile = shared_test_profile(&remote, &key);
            for (case_index, (kind, product)) in cases.iter().enumerate() {
                let original = remote
                    .prepare_profile_outgoing(&profile, SERVICE, kind, product.clone(), NOW, TTL)
                    .unwrap();
                let frame = remote
                    .prepare_transport_frame(original.envelope_bytes())
                    .unwrap();
                assert_eq!(
                    transport_rejection(&core, &frame, NOW),
                    CollaborationTransportRejection::InvalidMessage
                );
                assert!(core
                    .accept_home_chat(&frame, &remote.local_device_did(), NOW)
                    .is_err());
                assert!(core
                    .accept_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                    .is_err());
                assert!(core
                    .authorize_history_message(original.envelope_bytes(), NOW)
                    .is_err());
                assert!(core
                    .retain_history_message(original.envelope_bytes(), NOW)
                    .is_err());
                assert!(core
                    .history_projection_candidates(&[original.envelope_bytes().to_vec()], NOW)
                    .is_err());
                let operation = profile_operation(
                    &format!("invalid:{author_index}:{case_index}"),
                    &local_profile,
                    kind,
                    product,
                );
                assert!(core
                    .prepare_profile_outgoing(
                        operation,
                        &local_profile,
                        kind,
                        product.clone(),
                        NOW,
                        TTL
                    )
                    .is_err());
                assert!(!core.state_path().exists());
                assert!(!core.history_path().exists());
                let authorized = core
                    .authority
                    .authorize_incoming(original.envelope_bytes(), &remote.local_device_did(), NOW)
                    .unwrap();
                let receipt = core
                    .authority
                    .prepare_acceptance_receipt(&authorized, NOW)
                    .unwrap();
                legacy.incoming.push(IncomingRecord {
                    envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
                    acceptance_receipt: String::from_utf8(receipt).unwrap(),
                    product_handoff: ProductHandoffState::Pending,
                });
                if first.is_none() {
                    first = Some((key.clone(), original.envelope_bytes().to_vec()));
                }
            }
            // An author-signed but overlength Profile is not an admissible author
            // for either registered consumer. Signature validity cannot repair it.
            for kind in [
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
            ] {
                let product = if kind == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE {
                    serde_json::json!({"body":"valid"})
                } else {
                    serde_json::json!({})
                };
                let original = remote
                    .prepare_profile_outgoing(&profile, SERVICE, kind, product, NOW, TTL)
                    .unwrap();
                let mut document = profile.document().clone();
                document.display_name = "x".repeat(65);
                let mut signed: SignedCollaborationMessage =
                    serde_json::from_slice(original.envelope_bytes()).unwrap();
                signed.payload.payload["signed_profile"] =
                    serde_json::to_value(resign_test_profile(&key, &document)).unwrap();
                let invalid = resign_message(&key, signed.payload);
                let frame = remote.prepare_transport_frame(&invalid).unwrap();
                assert_eq!(
                    transport_rejection(&core, &frame, NOW),
                    CollaborationTransportRejection::InvalidMessage
                );
                assert!(core
                    .accept_incoming(&invalid, &remote.local_device_did(), NOW)
                    .is_err());
                assert!(!core.state_path().exists());
            }
        }
        assert_eq!(legacy.incoming.len(), MAX_PENDING_INCOMING);
        core.validate_state(&legacy).unwrap();
        core.ensure_state_directory().unwrap();
        core.write_state(&legacy).unwrap();
        let before = fs::read(core.state_path()).unwrap();
        let failed_repair = fixture.core();
        failed_repair.inject_write_fault(WriteFault::BeforeWrite);
        assert!(failed_repair.summary().is_err());
        assert_eq!(fs::read(core.state_path()).unwrap(), before);
        let restarted = fixture.core();
        // A plain read, before any handoff or mutation, durably repairs legacy slots.
        assert_eq!(restarted.summary().unwrap().pending_product_handoffs, 0);
        let repaired = restarted.load_state().unwrap().unwrap();
        assert_eq!(repaired.incoming_tombstones.len(), MAX_PENDING_INCOMING);
        assert!(repaired.incoming.is_empty());
        for entry in &legacy.incoming {
            assert!(repaired
                .incoming_tombstones
                .iter()
                .any(|tombstone| tombstone.acceptance_receipt == entry.acceptance_receipt));
        }
        let (key, bytes) = first.unwrap();
        let mut signed: SignedCollaborationMessage = serde_json::from_slice(&bytes).unwrap();
        signed.payload.payload["product"] = serde_json::json!({"body":"corrected"});
        let corrected = resign_message(&key, signed.payload);
        assert!(restarted
            .accept_incoming(&corrected, &message_source_endpoint_did(&corrected), NOW)
            .unwrap_err()
            .to_string()
            .contains("reused"));
        // Chat's actual projection retains history at the wall clock; Presence
        // uses its explicit fixture clock below to prove its live snapshot.
        let chat_now = now_secs();
        let (_, chat) = remote_transport_message(&fixture, generate_keypair().0, chat_now);
        transport_incoming(&restarted, &chat, chat_now);
        let presence = fixture.authority(generate_keypair().0);
        let metadata = presence
            .prepare_outgoing(
                SERVICE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW,
                45,
            )
            .unwrap();
        restarted
            .accept_incoming(metadata.envelope_bytes(), &presence.local_device_did(), NOW)
            .unwrap();
        let projection_core = Arc::new(fixture.core());
        let chat_port = crate::collaboration_product::CollaborationChatProductPort::new(
            projection_core.clone(),
        )
        .unwrap();
        let presence_port = crate::collaboration_presence::CollaborationPresenceProductPort::new(
            projection_core.clone(),
        )
        .unwrap();
        assert_eq!(projection_core.pending_product_handoffs().unwrap().len(), 2);
        let chat_handoffs = chat_port.pending_messages().unwrap();
        assert_eq!(chat_handoffs.len(), 1);
        let object = chat_port
            .project_handoff(&fixture.data_root, &chat_handoffs[0])
            .unwrap();
        assert_eq!(object.body.as_deref(), Some("remote"));
        assert_eq!(object.sender_profile_verified, Some(true));
        let presence_handoffs = presence_port.pending_presences().unwrap();
        assert_eq!(presence_handoffs.len(), 1);
        assert_eq!(
            presence_port
                .project_handoff(&presence_handoffs[0], NOW)
                .unwrap(),
            crate::collaboration_presence::CollaborationPresenceProjectionOutcome::Applied
        );
        let snapshot = presence_port.snapshot(NOW).unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(snapshot.records()[0].last_seen_at(), NOW);
        assert_eq!(snapshot.records()[0].expires_at(), NOW + 45);
        assert_eq!(
            projection_core.summary().unwrap().pending_product_handoffs,
            0
        );
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &projection_core.local_device_did(),
            "Reader",
            "consumer repair test",
        )
        .unwrap();
        let restarted_core = Arc::new(fixture.core());
        let restarted_chat =
            crate::collaboration_product::CollaborationChatProductPort::new(restarted_core.clone())
                .unwrap();
        let restarted_presence =
            crate::collaboration_presence::CollaborationPresenceProductPort::new(
                restarted_core.clone(),
            )
            .unwrap();
        let poll = restarted_chat
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(poll.objects.len(), 1);
        assert_eq!(poll.objects[0].seq, object.seq);
        assert_eq!(poll.objects[0].body.as_deref(), Some("remote"));
        assert_eq!(poll.objects[0].sender_profile_verified, Some(true));
        assert_eq!(restarted_presence.snapshot(NOW).unwrap(), snapshot);
        assert!(restarted_chat.pending_messages().unwrap().is_empty());
        assert!(restarted_presence.pending_presences().unwrap().is_empty());
        assert_eq!(
            restarted_core.summary().unwrap().pending_product_handoffs,
            0
        );
    }

    #[test]
    fn generated_shared_products_cannot_fill_handoffs_and_consumers_work_after_restart() {
        fn next(seed: &mut u64) -> u64 {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed
        }

        const ROUNDS: usize = 512;
        const CONSUMERS: [&str; 2] = [
            COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
            crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
        ];
        const OTHER_DELEGATIONS: [&str; 6] = [
            "elastos.chat.direct-message/v1",
            "elastos.chat.unregistered/v1",
            "elastos.collaboration.acceptance-receipt/v1",
            "elastos.room.accept.v1",
            "elastos.room.invite.v1",
            "elastos.room.join-invite.v1",
        ];
        let fixture = Fixture::new();
        let mut core = Arc::new(fixture.core());
        let local_profile = shared_test_profile(&core.authority, &fixture.device_key);
        // Adding a delegated type requires an explicit consumer/generator decision.
        let delegated: Vec<_> = local_profile
            .document()
            .collaboration_signers
            .iter()
            .filter(|signer| signer.service == SERVICE)
            .flat_map(|signer| &signer.delegations)
            .flat_map(|delegation| delegation.object_types.iter().map(String::as_str))
            .collect();
        let mut expected = Vec::from(CONSUMERS);
        expected.extend(OTHER_DELEGATIONS);
        expected.sort_unstable();
        assert_eq!(delegated, expected);
        let mut seed = 0x8c7d_41ae_b395_206fu64;
        let authors: Vec<_> = (0..4)
            .map(|_| {
                let mut bytes = [0u8; 32];
                for chunk in bytes.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&next(&mut seed).to_le_bytes());
                }
                let key = SigningKey::from_bytes(&bytes);
                let authority = fixture.authority(key.clone());
                let profile = shared_test_profile(&authority, &key);
                (key, authority, profile)
            })
            .collect();
        let mut coverage = [[0usize; 8]; CONSUMERS.len()];
        let mut other_coverage = [0usize; OTHER_DELEGATIONS.len()];
        let mut routes = [0usize; 3];
        let mut attempts = 0;
        let mut valid_processed = 0;
        let mut identity_conflicts = 0;
        let mut objects = Vec::new();
        for round in 0..ROUNDS {
            for consumer in 0..=CONSUMERS.len() {
                let before_state = core
                    .state_path()
                    .exists()
                    .then(|| fs::read(core.state_path()).unwrap());
                let before_history = core
                    .history_path()
                    .exists()
                    .then(|| fs::read(core.history_path()).unwrap());
                let (key, remote, profile) = &authors[(next(&mut seed) % 4) as usize];
                let variant = (next(&mut seed) % 8) as usize;
                let length = (next(&mut seed) % 128) as usize;
                let field = format!("extra_{:x}", next(&mut seed));
                let text = format!("message {:x}", next(&mut seed));
                let wrong_type = match next(&mut seed) % 5 {
                    0 => serde_json::Value::Null,
                    1 => serde_json::json!(false),
                    2 => serde_json::json!(next(&mut seed)),
                    3 => serde_json::json!([text]),
                    _ => serde_json::json!({"nested":text}),
                };
                let (kind, product, bad_name) = if consumer < CONSUMERS.len() {
                    coverage[consumer][variant] += 1;
                    let product = if consumer == 0 {
                        match variant {
                            0 => serde_json::json!({"body":text, (field):wrong_type}),
                            1 => serde_json::json!({"body":wrong_type}),
                            2 => serde_json::json!({}),
                            3 => {
                                serde_json::json!({"body":(if length.is_multiple_of(2) { "🙂" } else { "x" }).repeat(
                                crate::room_service::MAX_OBJECT_BODY_LEN + 1 + length)})
                            }
                            4 => serde_json::json!({"body":" \n\t".repeat(1 + length)}),
                            5 => serde_json::json!([{"body":text}]),
                            6 => serde_json::json!({"body":text}),
                            _ => serde_json::json!({"body":format!(" {text} ")}),
                        }
                    } else {
                        match variant {
                            0 => serde_json::json!({(field):wrong_type}),
                            1 => wrong_type,
                            2 => serde_json::json!({"body":text}),
                            3 => serde_json::json!({"body":"x".repeat(
                                crate::room_service::MAX_OBJECT_BODY_LEN + 1 + length)}),
                            4 => serde_json::json!(" \n\t".repeat(1 + length)),
                            5 => serde_json::json!([{}]),
                            6 => serde_json::json!({}),
                            _ => serde_json::json!({"display_name":"n".repeat(65 + length)}),
                        }
                    };
                    (CONSUMERS[consumer], product, variant == 6)
                } else {
                    let other = (next(&mut seed) % OTHER_DELEGATIONS.len() as u64) as usize;
                    other_coverage[other] += 1;
                    (
                        OTHER_DELEGATIONS[other],
                        serde_json::json!({"body":text}),
                        false,
                    )
                };
                let prepared = remote
                    .prepare_profile_outgoing(profile, SERVICE, kind, product.clone(), NOW, TTL)
                    .unwrap();
                let mut signed: SignedCollaborationMessage =
                    serde_json::from_slice(prepared.envelope_bytes()).unwrap();
                // Keep signed identity and generated cases reproducible, including
                // the malformed Profile carried inside an otherwise valid envelope.
                signed.payload.message_id = format!("{:032x}", round * 3 + consumer + 1);
                signed.payload.nonce = format!("{:032x}", next(&mut seed));
                if bad_name {
                    let mut document = profile.document().clone();
                    document.display_name = "n".repeat(65 + length);
                    signed.payload.payload["signed_profile"] =
                        serde_json::to_value(resign_test_profile(key, &document)).unwrap();
                }
                let original = resign_message(key, signed.payload);
                verify_stored_collaboration_message(&original, &fixture.profile, SERVICE).unwrap();
                let frame = remote.prepare_transport_frame(&original).unwrap();
                // Rotate real signed ingress paths rather than repeating every
                // cryptographic path for every generated envelope.
                let route = (next(&mut seed) % routes.len() as u64) as usize;
                routes[route] += 1;
                match route {
                    0 => assert_eq!(
                        transport_rejection(&core, &frame, NOW),
                        CollaborationTransportRejection::InvalidMessage
                    ),
                    1 => assert!(core
                        .accept_home_chat(&frame, &remote.local_device_did(), NOW)
                        .is_err()),
                    _ => assert!(core
                        .accept_incoming(&original, &remote.local_device_did(), NOW)
                        .is_err()),
                }
                if !bad_name {
                    let operation = profile_operation(
                        &format!("generated:{round}:{consumer}"),
                        &local_profile,
                        kind,
                        &product,
                    );
                    assert!(core
                        .prepare_profile_outgoing(
                            operation,
                            &local_profile,
                            kind,
                            product,
                            NOW,
                            TTL
                        )
                        .is_err());
                }
                attempts += 1;
                assert!(core.pending_product_handoffs().unwrap().is_empty());
                assert_eq!(core.summary().unwrap().pending_product_handoffs, 0);
                assert_eq!(
                    core.state_path()
                        .exists()
                        .then(|| fs::read(core.state_path()).unwrap()),
                    before_state
                );
                assert_eq!(
                    core.history_path()
                        .exists()
                        .then(|| fs::read(core.history_path()).unwrap()),
                    before_history
                );
                if attempts % MAX_PENDING_INCOMING == 0 {
                    core = Arc::new(fixture.core());
                    assert!(core.pending_product_handoffs().unwrap().is_empty());
                }
            }
            if round.is_multiple_of(8) {
                let mut bytes = [0u8; 32];
                for chunk in bytes.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&next(&mut seed).to_le_bytes());
                }
                let key = SigningKey::from_bytes(&bytes);
                let remote = fixture.authority(key.clone());
                let mut document = shared_test_profile(&remote, &key).document().clone();
                document.display_name = match (round / 8) % 4 {
                    0 => "n".repeat(49),
                    1 => "n".repeat(64),
                    2 => "é".repeat(32),
                    _ => "🙂".repeat(16),
                };
                let profile =
                    crate::collaboration_profile_authority::verify_signed_profile_document(
                        &resign_test_profile(&key, &document),
                    )
                    .unwrap();
                let now = now_secs();
                let body = match (round / 8) % 4 {
                    0 => "🙂".repeat(crate::room_service::MAX_OBJECT_BODY_LEN),
                    1 => "x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN),
                    2 => "é".repeat(1 + (next(&mut seed) % 80) as usize),
                    _ => format!("combining e\u{301} {:x}", next(&mut seed)),
                };
                let chat = remote
                    .prepare_profile_outgoing(
                        &profile,
                        SERVICE,
                        COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                        serde_json::json!({"body":body}),
                        now,
                        TTL,
                    )
                    .unwrap();
                let frame = remote
                    .prepare_transport_frame(chat.envelope_bytes())
                    .unwrap();
                core.accept_home_chat(&frame, &remote.local_device_did(), now)
                    .unwrap();
                let port =
                    crate::collaboration_product::CollaborationChatProductPort::new(core.clone())
                        .unwrap();
                let handoffs = port.pending_messages().unwrap();
                assert_eq!(handoffs.len(), 1);
                let object = port
                    .project_handoff(&fixture.data_root, &handoffs[0])
                    .unwrap();
                assert_eq!(object.body.as_deref(), Some(body.as_str()));
                assert_eq!(object.sender, document.display_name);
                assert_eq!(object.sender_profile_verified, Some(true));
                objects.push(object);
                valid_processed += 1;

                let mut conflict: SignedCollaborationMessage =
                    serde_json::from_slice(chat.envelope_bytes()).unwrap();
                conflict.payload.payload["product"] =
                    serde_json::json!({"body":"different signed original"});
                if round.is_multiple_of(16) {
                    conflict.payload.nonce = format!("{:032x}", next(&mut seed));
                } else {
                    conflict.payload.message_id = format!("{:032x}", next(&mut seed));
                }
                let conflict = resign_message(&key, conflict.payload);
                core.authorize_history_message(&conflict, now).unwrap();
                let conflict_frame = remote.prepare_transport_frame(&conflict).unwrap();
                assert_eq!(
                    transport_rejection(&core, &conflict_frame, now),
                    CollaborationTransportRejection::MessageIdentityConflict
                );
                identity_conflicts += 1;

                let metadata = remote
                    .prepare_profile_outgoing(
                        &profile,
                        SERVICE,
                        crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                        serde_json::json!({}),
                        now,
                        45,
                    )
                    .unwrap();
                let frame = remote
                    .prepare_transport_frame(metadata.envelope_bytes())
                    .unwrap();
                transport_incoming(&core, &frame, now);
                let presence =
                    crate::collaboration_presence::CollaborationPresenceProductPort::new(
                        core.clone(),
                    )
                    .unwrap();
                let handoffs = presence.pending_presences().unwrap();
                assert_eq!(handoffs.len(), 1);
                assert_eq!(
                    presence.project_handoff(&handoffs[0], now).unwrap(),
                    crate::collaboration_presence::CollaborationPresenceProjectionOutcome::Applied
                );
                assert!(presence
                    .snapshot(now)
                    .unwrap()
                    .records()
                    .iter()
                    .any(
                        |record| record.sender_profile_did() == profile.document().profile_did
                            && record.display_name() == document.display_name
                    ));
                valid_processed += 1;
                assert!(fixture
                    .core()
                    .pending_product_handoffs()
                    .unwrap()
                    .is_empty());
            }
        }
        assert_eq!(attempts, ROUNDS * (CONSUMERS.len() + 1));
        assert!(attempts > MAX_PENDING_INCOMING * 32);
        assert!(coverage.iter().flatten().all(|count| *count > 0));
        assert!(other_coverage.iter().all(|count| *count > 0));
        assert!(routes.iter().all(|count| *count > 0));
        assert_eq!(valid_processed, (ROUNDS / 8) * CONSUMERS.len());
        assert_eq!(identity_conflicts, ROUNDS / 8);

        // Valid signed names above the old Room limit still reach both consumers,
        // including the Profile maximum. Use Chat's actual history wall clock.
        let chat_now = now_secs();
        let restarted = fixture.core();
        for name_length in [49, 56, 64] {
            let key = SigningKey::from_bytes(&[name_length as u8; 32]);
            let remote = fixture.authority(key.clone());
            let mut document = shared_test_profile(&remote, &key).document().clone();
            document.display_name = "n".repeat(name_length);
            let profile = crate::collaboration_profile_authority::verify_signed_profile_document(
                &resign_test_profile(&key, &document),
            )
            .unwrap();
            for kind in CONSUMERS {
                let product = if kind == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE {
                    serde_json::json!({"body":format!("healthy {name_length}")})
                } else {
                    serde_json::json!({})
                };
                let now = if kind == COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE {
                    chat_now
                } else {
                    NOW
                };
                let original = remote
                    .prepare_profile_outgoing(&profile, SERVICE, kind, product, now, 45)
                    .unwrap();
                restarted
                    .accept_incoming(original.envelope_bytes(), &remote.local_device_did(), now)
                    .unwrap();
            }
        }
        let projection_core = Arc::new(fixture.core());
        let chat_port = crate::collaboration_product::CollaborationChatProductPort::new(
            projection_core.clone(),
        )
        .unwrap();
        let presence_port = crate::collaboration_presence::CollaborationPresenceProductPort::new(
            projection_core.clone(),
        )
        .unwrap();
        assert_eq!(projection_core.pending_product_handoffs().unwrap().len(), 6);
        let handoffs = chat_port.pending_messages().unwrap();
        assert_eq!(handoffs.len(), 3);
        for handoff in &handoffs {
            let object = chat_port
                .project_handoff(&fixture.data_root, handoff)
                .unwrap();
            assert_eq!(object.sender_profile_verified, Some(true));
            objects.push(object);
        }
        for handoff in presence_port.pending_presences().unwrap() {
            assert_eq!(
                presence_port.project_handoff(&handoff, NOW).unwrap(),
                crate::collaboration_presence::CollaborationPresenceProjectionOutcome::Applied
            );
        }
        let snapshot = presence_port.snapshot(NOW).unwrap();
        assert_eq!(snapshot.records().len(), 3);
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &projection_core.local_device_did(),
            "Reader",
            "generated consumer test",
        )
        .unwrap();
        let final_core = Arc::new(fixture.core());
        let final_chat =
            crate::collaboration_product::CollaborationChatProductPort::new(final_core.clone())
                .unwrap();
        let final_presence = crate::collaboration_presence::CollaborationPresenceProductPort::new(
            final_core.clone(),
        )
        .unwrap();
        let poll = final_chat
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(poll.objects.len(), objects.len());
        for object in objects {
            let retained = poll
                .objects
                .iter()
                .find(|row| row.seq == object.seq)
                .unwrap();
            assert_eq!(retained.body, object.body);
            assert_eq!(retained.sender, object.sender);
            assert_eq!(retained.sender_profile_verified, Some(true));
        }
        assert_eq!(final_presence.snapshot(NOW).unwrap(), snapshot);
        assert!(final_core.pending_product_handoffs().unwrap().is_empty());
        assert!(final_chat.pending_messages().unwrap().is_empty());
        assert!(final_presence.pending_presences().unwrap().is_empty());
    }

    #[test]
    fn legacy_unconsumable_outgoing_is_retired_without_losing_request_replay_protection() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let profile = shared_test_profile(&core.authority, &fixture.device_key);
        let cases = [
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":false}),
            ),
            (
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({"extra":true}),
            ),
            ("elastos.chat.unregistered/v1", serde_json::json!({})),
        ];
        let mut legacy = core.empty_state();
        for index in 0..MAX_UNRESOLVED_OUTGOING {
            let (kind, product) = &cases[index % cases.len()];
            let original = core
                .authority
                .prepare_profile_outgoing(&profile, SERVICE, kind, product.clone(), NOW, TTL)
                .unwrap();
            legacy.outgoing.push(OutgoingRecord {
                operation: profile_operation(&format!("legacy:{index}"), &profile, kind, product),
                envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
                local_product_projection: if index % 2 == 0 {
                    OutgoingProductProjectionState::Pending
                } else {
                    OutgoingProductProjectionState::Complete
                },
                remote_acceptance_receipts: Vec::new(),
            });
        }
        core.validate_state(&legacy).unwrap();
        core.ensure_state_directory().unwrap();
        core.write_state(&legacy).unwrap();
        let restarted = fixture.core();
        let first = &legacy.outgoing[0];
        let (kind, product) = &cases[0];
        // A rejected operation must still commit the legacy repair it loaded.
        assert!(restarted
            .prepare_profile_outgoing(
                first.operation.clone(),
                &profile,
                kind,
                product.clone(),
                NOW,
                TTL
            )
            .unwrap_err()
            .to_string()
            .contains("retired"));
        let repaired = fixture.core().load_state().unwrap().unwrap();
        assert_eq!(repaired.outgoing.len(), MAX_UNRESOLVED_OUTGOING);
        assert!(
            repaired
                .outgoing
                .iter()
                .all(|entry| entry.local_product_projection
                    == OutgoingProductProjectionState::Retired)
        );
        for (old, retired) in legacy.outgoing.iter().zip(&repaired.outgoing) {
            assert_eq!(old.envelope, retired.envelope);
            assert_eq!(old.operation, retired.operation);
            assert!(restarted
                .acknowledge_outgoing_product_projection(&collaboration_message_envelope_sha256(
                    old.envelope.as_bytes()
                ))
                .is_err());
        }
        assert!(restarted.pending_outgoing(NOW).unwrap().is_empty());
        assert!(restarted
            .pending_outgoing_product_projections(NOW)
            .unwrap()
            .is_empty());
        assert!(restarted
            .outgoing_chat_delivery(&profile.document().profile_did)
            .unwrap()
            .is_empty());
        let first = &legacy.outgoing[0];
        let (kind, product) = &cases[0];
        assert!(restarted
            .prepare_profile_outgoing(
                first.operation.clone(),
                &profile,
                kind,
                product.clone(),
                NOW,
                TTL
            )
            .unwrap_err()
            .to_string()
            .contains("retired"));
        let different = serde_json::json!({"body":"valid"});
        let reused = profile_operation(
            &first.operation.request_id,
            &profile,
            COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
            &different,
        );
        assert!(restarted
            .prepare_profile_outgoing(
                reused,
                &profile,
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                different,
                NOW,
                TTL
            )
            .unwrap_err()
            .to_string()
            .contains("another binding"));
        let summary = restarted.summary().unwrap();
        assert_eq!(
            summary.live_unresolved_outgoing
                + summary.expired_unaccepted_outgoing
                + summary.remotely_accepted_outgoing,
            0
        );
        let valid = prepare(
            &restarted,
            "fresh-after-retirement",
            serde_json::json!({"body":"valid"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&restarted, &valid);
        assert_eq!(restarted.pending_outgoing(NOW).unwrap().len(), 1);
        let at_boundary = NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS;
        restarted
            .with_mutation(Some(at_boundary), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert_eq!(
            restarted.load_state().unwrap().unwrap().outgoing.len(),
            MAX_UNRESOLVED_OUTGOING + 1
        );
        restarted
            .with_mutation(Some(at_boundary + 1), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert!(fixture
            .core()
            .load_state()
            .unwrap()
            .unwrap()
            .outgoing
            .is_empty());

        let product = serde_json::json!({"body":"a valid intent cannot be marked retired"});
        let original = core
            .authority
            .prepare_profile_outgoing(
                &profile,
                SERVICE,
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                product.clone(),
                NOW,
                TTL,
            )
            .unwrap();
        let mut forged_retirement = core.empty_state();
        forged_retirement.outgoing.push(OutgoingRecord {
            operation: profile_operation(
                "false-retirement",
                &profile,
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                &product,
            ),
            envelope: String::from_utf8(original.envelope_bytes().to_vec()).unwrap(),
            local_product_projection: OutgoingProductProjectionState::Retired,
            remote_acceptance_receipts: Vec::new(),
        });
        core.write_state(&forged_retirement).unwrap();
        let bytes = fs::read(core.state_path()).unwrap();
        assert!(fixture.core().load_state().is_err());
        assert_eq!(fs::read(core.state_path()).unwrap(), bytes);
    }

    #[test]
    fn retained_history_retires_invalid_consumers_but_keeps_signed_replay_bindings() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let key = generate_keypair().0;
        let remote = fixture.authority(key.clone());
        let profile = shared_test_profile(&remote, &key);
        let mut history = core.empty_history();
        for (kind, product) in [
            (
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
            ),
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":false}),
            ),
            (
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({"extra":true}),
            ),
            (
                "elastos.chat.unregistered/v1",
                serde_json::json!({"body":"Chat-shaped but unregistered"}),
            ),
            (
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":"retained valid row"}),
            ),
        ] {
            let original = remote
                .prepare_profile_outgoing(&profile, SERVICE, kind, product, NOW, TTL)
                .unwrap();
            history
                .envelopes
                .push(String::from_utf8(original.envelope_bytes().to_vec()).unwrap());
        }
        core.ensure_state_directory().unwrap();
        let original = canonical_history_bytes(&history).unwrap();
        assert!(!String::from_utf8(original.clone())
            .unwrap()
            .contains("retired_envelope_hashes"));
        write_owner_only(&core.history_path(), &original);
        let mut corrupt = history.clone();
        let mut signed: SignedCollaborationMessage =
            serde_json::from_str(&corrupt.envelopes[0]).unwrap();
        signed.signature = "00".repeat(64);
        corrupt.envelopes[0] =
            String::from_utf8(canonical_signed_collaboration_message_bytes(&signed).unwrap())
                .unwrap();
        let corrupt_bytes = canonical_history_bytes(&corrupt).unwrap();
        write_owner_only(&core.history_path(), &corrupt_bytes);
        assert!(core.conversation_history(NOW).is_err());
        assert_eq!(fs::read(core.history_path()).unwrap(), corrupt_bytes);
        write_owner_only(&core.history_path(), &original);
        let reopened = fixture.core();
        assert_eq!(
            reopened.conversation_history(NOW).unwrap(),
            vec![history.envelopes[4].as_bytes().to_vec()]
        );
        let retired = reopened.load_history().unwrap().unwrap();
        let mut canonical_originals = history.envelopes.clone();
        // All five timestamps are NOW; the existing retention order breaks
        // ties with the original hash, without changing or dropping bytes.
        canonical_originals
            .sort_by_key(|envelope| collaboration_message_envelope_sha256(envelope.as_bytes()));
        assert_eq!(retired.envelopes, canonical_originals);
        assert_eq!(retired.retired_envelope_hashes.len(), 4);
        let saved = fs::read(core.history_path()).unwrap();
        assert_eq!(
            fixture.core().conversation_history(NOW).unwrap(),
            vec![history.envelopes[4].as_bytes().to_vec()]
        );
        let mut corrected: SignedCollaborationMessage =
            serde_json::from_str(&history.envelopes[0]).unwrap();
        corrected.payload.payload_type = COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE.to_string();
        corrected.payload.payload["product"] = serde_json::json!({"body":"corrected"});
        let corrected = resign_message(&key, corrected.payload);
        assert!(reopened
            .history_projection_candidates(std::slice::from_ref(&corrected), NOW)
            .unwrap()
            .is_empty());
        assert!(reopened.retain_history_message(&corrected, NOW).is_err());
        assert_eq!(fs::read(core.history_path()).unwrap(), saved);
        for mutation in 0..3 {
            let mut invalid = retired.clone();
            match mutation {
                0 => invalid
                    .retired_envelope_hashes
                    .push(collaboration_message_envelope_sha256(
                        history.envelopes[4].as_bytes(),
                    )),
                1 => invalid
                    .retired_envelope_hashes
                    .push(invalid.retired_envelope_hashes[0].clone()),
                _ => invalid.retired_envelope_hashes[0] = format!("sha256:{}", "00".repeat(32)),
            }
            let bytes = canonical_history_bytes(&invalid).unwrap();
            write_owner_only(&core.history_path(), &bytes);
            assert!(reopened.load_history().is_err());
            assert!(fixture.core().load_history().is_err());
            assert_eq!(fs::read(core.history_path()).unwrap(), bytes);
        }
        write_owner_only(&core.history_path(), &saved);
        let mut expired = fixture.core().load_history().unwrap().unwrap();
        prune_history(&mut expired, NOW + CONVERSATION_HISTORY_RETENTION_SECS).unwrap();
        assert!(expired.envelopes.is_empty());
        assert!(expired.retired_envelope_hashes.is_empty());
        core.validate_history(&expired).unwrap();
    }

    #[test]
    fn concurrent_shared_admission_keeps_five_cap_and_counts_only_new_commits() {
        use std::sync::{Arc, Barrier};
        let fixture = Fixture::new();
        let core = Arc::new(fixture.core());
        let key = generate_keypair().0;
        let remote = fixture.authority(key.clone());
        let source = remote.local_device_did();
        let (_, failed) = remote_transport_message(&fixture, key.clone(), NOW);
        core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(core.accept_home_chat(&failed, &source, NOW).is_err());
        let (_, duplicate) = remote_transport_message(&fixture, key.clone(), NOW);
        let start = Arc::new(Barrier::new(9));
        let workers = (0..8)
            .map(|_| {
                let core = core.clone();
                let start = start.clone();
                let frame = duplicate.clone();
                let source = source.clone();
                std::thread::spawn(move || {
                    start.wait();
                    core.accept_home_chat(&frame, &source, NOW).unwrap()
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let receipts = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(receipts.iter().all(|receipt| receipt == &receipts[0]));
        for _ in 0..COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW - 2 {
            let (_, frame) = remote_transport_message(&fixture, key.clone(), NOW);
            core.accept_home_chat(&frame, &source, NOW).unwrap();
        }
        let start = Arc::new(Barrier::new(3));
        let contenders = (0..2)
            .map(|_| {
                let core = core.clone();
                let start = start.clone();
                let source = source.clone();
                let (_, frame) = remote_transport_message(&fixture, key.clone(), NOW);
                std::thread::spawn(move || {
                    start.wait();
                    core.accept_home_chat(&frame, &source, NOW).is_ok()
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        assert_eq!(
            contenders
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|accepted| *accepted)
                .count(),
            1
        );
        assert_eq!(
            core.load_state().unwrap().unwrap().incoming.len(),
            COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW
        );
        core.accept_home_chat(&duplicate, &source, NOW).unwrap();
        let (_, other) = remote_transport_message(&fixture, generate_keypair().0, NOW);
        transport_incoming(&core, &other, NOW);
        assert_eq!(
            core.pending_product_handoffs().unwrap().len(),
            COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW + 1
        );
    }

    fn transport_rejection(
        core: &CollaborationCore,
        frame: &[u8],
        now: u64,
    ) -> CollaborationTransportRejection {
        match core.ingest_transport_frame(frame, now).unwrap() {
            CollaborationTransportIngestion::Rejected(reason) => reason,
            other => panic!("expected deterministic rejection, got {other:?}"),
        }
    }

    fn transport_incoming(
        core: &CollaborationCore,
        frame: &[u8],
        now: u64,
    ) -> CollaborationTransportIncomingAcceptance {
        match core.ingest_transport_frame(frame, now).unwrap() {
            CollaborationTransportIngestion::Incoming(accepted) => accepted,
            other => panic!("expected incoming acceptance, got {other:?}"),
        }
    }

    #[test]
    fn community_send_limit_refuses_a_burst_but_not_retries_or_a_later_message() {
        let fixture = Fixture::new();
        let core = fixture.core();
        for index in 0..COMMUNITY_SENDS_PER_WINDOW {
            prepare(
                &core,
                &format!("burst-{index}"),
                serde_json::json!({"body":format!("message {index}")}),
                NOW,
                TTL,
            )
            .unwrap();
        }
        let before = fs::read(core.state_path()).unwrap();
        let error = prepare(
            &core,
            "burst-over",
            serde_json::json!({"body":"over"}),
            NOW + 1,
            TTL,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<CommunitySendRateLimited>(),
            Some(&CommunitySendRateLimited {
                retry_after_secs: COMMUNITY_RATE_WINDOW_SECS - 1
            })
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), before);

        // A retry of a stored message is not a new send.
        prepare(
            &core,
            "burst-0",
            serde_json::json!({"body":"message 0"}),
            NOW + 1,
            TTL,
        )
        .unwrap();
        // The window slides.
        prepare(
            &core,
            "burst-over",
            serde_json::json!({"body":"over"}),
            NOW + COMMUNITY_RATE_WINDOW_SECS,
            TTL,
        )
        .unwrap();
    }

    #[test]
    fn community_send_limit_counts_exactly_the_saved_messages() {
        let fixture = Fixture::new();
        let core = fixture.core();
        // A send that fails before the write saves nothing and counts nothing.
        for _ in 0..=COMMUNITY_SENDS_PER_WINDOW {
            core.inject_write_fault(WriteFault::BeforeWrite);
            assert!(prepare(
                &core,
                "unsaved",
                serde_json::json!({"body":"unsaved"}),
                NOW,
                TTL
            )
            .unwrap_err()
            .downcast_ref::<CommunitySendRateLimited>()
            .is_none());
        }
        // A send that fails after the rename is saved, and counts.
        core.inject_write_fault(WriteFault::AfterRename);
        assert!(prepare(
            &core,
            "saved-late",
            serde_json::json!({"body":"late"}),
            NOW,
            TTL
        )
        .is_err());
        for index in 1..COMMUNITY_SENDS_PER_WINDOW {
            prepare(
                &core,
                &format!("saved-{index}"),
                serde_json::json!({"body":format!("message {index}")}),
                NOW,
                TTL,
            )
            .unwrap();
        }
        let error =
            prepare(&core, "over", serde_json::json!({"body":"over"}), NOW, TTL).unwrap_err();
        assert!(error.downcast_ref::<CommunitySendRateLimited>().is_some());
        // Its retry returns the stored message without counting again.
        prepare(
            &core,
            "saved-late",
            serde_json::json!({"body":"late"}),
            NOW,
            TTL,
        )
        .unwrap();
    }

    #[test]
    fn community_receive_limit_drops_a_flooding_sender_without_stalling_others() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let accept = |frame: &[u8], now: u64| {
            transport_incoming(&core, frame, now);
            for handoff in core.pending_product_handoffs().unwrap() {
                core.acknowledge_product_handoff(
                    handoff.authorized_message().message().envelope_sha256(),
                )
                .unwrap();
            }
        };
        let (flooder, _) = generate_keypair();
        for _ in 0..COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW {
            let (_, frame) = remote_transport_message(&fixture, flooder.clone(), NOW);
            accept(&frame, NOW);
        }
        let (_, over) = remote_transport_message(&fixture, flooder, NOW);
        let before = fs::read(core.state_path()).unwrap();
        assert_eq!(
            transport_rejection(&core, &over, NOW + 1),
            CollaborationTransportRejection::SenderRateLimited
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), before);

        // Another sender in the same window is unaffected.
        let (_, other) = remote_transport_message(&fixture, generate_keypair().0, NOW);
        accept(&other, NOW + 1);
        // The flooder's rebroadcast lands once its window slides.
        accept(&over, NOW + COMMUNITY_RATE_WINDOW_SECS);
    }

    fn remote_presence_frame(fixture: &Fixture, key: &SigningKey, now: u64, ttl: u64) -> Vec<u8> {
        let prepared = fixture
            .authority(key.clone())
            .prepare_outgoing(
                SERVICE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                now,
                ttl,
            )
            .unwrap();
        wrap_transport_frame(fixture, key, prepared.envelope_bytes())
    }

    fn acknowledge_all_handoffs(core: &CollaborationCore) {
        for handoff in core.pending_product_handoffs().unwrap() {
            core.acknowledge_product_handoff(
                handoff.authorized_message().message().envelope_sha256(),
            )
            .unwrap();
        }
    }

    #[test]
    fn steady_presence_from_twenty_peers_never_exhausts_intake() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let peers = (0..20).map(|_| generate_keypair().0).collect::<Vec<_>>();
        let presence_ttl = crate::collaboration_presence::PRESENCE_TTL_SECS;
        let mut now = NOW;
        let mut most_tombstones = 0;
        // Two simulated hours of presence every 15 s from each peer.
        while now < NOW + 2 * 60 * 60 {
            for peer in &peers {
                let frame = remote_presence_frame(&fixture, peer, now, presence_ttl);
                assert!(transport_incoming(&core, &frame, now).product_handoff_pending());
            }
            acknowledge_all_handoffs(&core);
            most_tombstones = most_tombstones.max(core.summary().unwrap().replay_tombstones);
            now += 15;
        }
        // Presence is admissible only until its own expiry, so it leaves no
        // persisted replay tombstone.
        assert_eq!(most_tombstones, 0);
        let (_, post) = remote_transport_message(&fixture, peers[0].clone(), now);
        assert!(transport_incoming(&core, &post, now).product_handoff_pending());
    }

    #[test]
    fn hostile_presence_flood_cannot_exhaust_intake_or_spend_chat_budget() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let hostile = generate_keypair().0;
        let presence_ttl = crate::collaboration_presence::PRESENCE_TTL_SECS;
        // Five simulated minutes at 50 presence frames per second.
        for second in 0..5 * 60 {
            let now = NOW + second;
            for _ in 0..50 {
                let frame = remote_presence_frame(&fixture, &hostile, now, presence_ttl);
                match core.ingest_transport_frame(&frame, now).unwrap() {
                    CollaborationTransportIngestion::Incoming(_) => {}
                    CollaborationTransportIngestion::Rejected(reason) => {
                        assert_eq!(reason, CollaborationTransportRejection::SenderRateLimited);
                    }
                    other => panic!("unexpected presence outcome {other:?}"),
                }
            }
            // A presence claiming a longer life than an honest one is refused.
            let long_lived = remote_presence_frame(&fixture, &hostile, now, TTL);
            assert_eq!(
                transport_rejection(&core, &long_lived, now),
                CollaborationTransportRejection::InvalidMessage
            );
            acknowledge_all_handoffs(&core);
        }
        let now = NOW + 5 * 60;
        assert_eq!(core.summary().unwrap().replay_tombstones, 0);
        assert!(core.held.take(now).is_empty());
        // Others' posts still arrive, and the flood spent none of the
        // hostile sender's chat budget.
        for _ in 0..3 {
            let (_, post) = remote_transport_message(&fixture, generate_keypair().0, now);
            assert!(transport_incoming(&core, &post, now).product_handoff_pending());
        }
        for _ in 0..COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW {
            let (_, post) = remote_transport_message(&fixture, hostile.clone(), now);
            assert!(transport_incoming(&core, &post, now).product_handoff_pending());
            acknowledge_all_handoffs(&core);
        }
    }

    #[test]
    fn failed_presence_acknowledgement_keeps_its_slot_until_a_retry_removes_it() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let presence_ttl = crate::collaboration_presence::PRESENCE_TTL_SECS;
        let frame = remote_presence_frame(&fixture, &generate_keypair().0, NOW, presence_ttl);
        transport_incoming(&core, &frame, NOW);
        let hash = core.pending_product_handoffs().unwrap()[0]
            .authorized_message()
            .message()
            .envelope_sha256()
            .to_string();
        core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(core.acknowledge_product_handoff(&hash).is_err());
        assert_eq!(core.pending_product_handoffs().unwrap().len(), 1);
        core.acknowledge_product_handoff(&hash).unwrap();
        assert!(core.pending_product_handoffs().unwrap().is_empty());
        assert_eq!(core.summary().unwrap().replay_tombstones, 0);
        core.acknowledge_product_handoff(&hash).unwrap();
    }

    #[test]
    fn chat_replays_stay_refused_and_presence_leaves_no_tombstone() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (_, post) = remote_transport_message(&fixture, generate_keypair().0, NOW);
        let receipt = transport_incoming(&core, &post, NOW)
            .acceptance_receipt_bytes()
            .to_vec();
        acknowledge_all_handoffs(&core);
        // A chat replay within its lifetime only gets its original receipt.
        let replay = transport_incoming(&core, &post, NOW + TTL - 1);
        assert!(!replay.product_handoff_pending());
        assert_eq!(replay.acceptance_receipt_bytes(), receipt);
        assert!(core.pending_product_handoffs().unwrap().is_empty());
        // A frame never seen before is refused once its expiry has passed.
        let (_, late) = remote_transport_message(&fixture, generate_keypair().0, NOW);
        assert_eq!(
            transport_rejection(&core, &late, NOW + TTL),
            CollaborationTransportRejection::InvalidMessage
        );

        // Presence is suppressed in memory until expiry and never persisted.
        let peer = generate_keypair().0;
        let presence_ttl = crate::collaboration_presence::PRESENCE_TTL_SECS;
        let presence = remote_presence_frame(&fixture, &peer, NOW, presence_ttl);
        let presence_receipt = transport_incoming(&core, &presence, NOW)
            .acceptance_receipt_bytes()
            .to_vec();
        acknowledge_all_handoffs(&core);
        let replay = transport_incoming(&core, &presence, NOW + presence_ttl - 1);
        assert!(!replay.product_handoff_pending());
        assert_eq!(replay.acceptance_receipt_bytes(), presence_receipt);
        let suppressed_until = NOW + presence_ttl + 2 * MAX_COLLABORATION_CLOCK_SKEW_SECS;
        assert!(!transport_incoming(&core, &presence, suppressed_until).product_handoff_pending());
        assert_eq!(
            transport_rejection(&core, &presence, suppressed_until + 1),
            CollaborationTransportRejection::InvalidMessage
        );

        // The state keeps the released format: only the chat tombstone, with
        // the exact 24-hour deadline the released validator requires.
        let written: CoreState =
            serde_json::from_slice(&fs::read(core.state_path()).unwrap()).unwrap();
        assert!(written.incoming.is_empty());
        assert_eq!(written.incoming_tombstones.len(), 1);
        assert_eq!(
            written.incoming_tombstones[0].retain_until,
            tombstone_retention_deadline(NOW)
        );
        fixture.core().validate_state(&written).unwrap();
    }

    #[test]
    fn transport_dispatch_and_pure_message_rejections_are_deterministic() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote_key, message) = remote_transport_message(&fixture, generate_keypair().0, NOW);

        assert_eq!(
            transport_rejection(&core, b"{", NOW),
            CollaborationTransportRejection::MalformedOrNoncanonicalFrame
        );
        let mut noncanonical = message.clone();
        noncanonical.push(b'\n');
        assert_eq!(
            transport_rejection(&core, &noncanonical, NOW),
            CollaborationTransportRejection::MalformedOrNoncanonicalFrame
        );

        let mut unknown_schema: serde_json::Value =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        unknown_schema["payload"]["schema"] = serde_json::json!("unknown/v1");
        let unknown_schema = wrap_transport_frame(
            &fixture,
            &remote_key,
            &serde_json::to_vec(&unknown_schema).unwrap(),
        );
        assert_eq!(
            transport_rejection(&core, &unknown_schema, NOW),
            CollaborationTransportRejection::UnsupportedSchema
        );

        let mut missing_schema: serde_json::Value =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        missing_schema["payload"]
            .as_object_mut()
            .unwrap()
            .remove("schema");
        let missing_schema = wrap_transport_frame(
            &fixture,
            &remote_key,
            &serde_json::to_vec(&missing_schema).unwrap(),
        );
        assert_eq!(
            transport_rejection(&core, &missing_schema, NOW),
            CollaborationTransportRejection::MalformedOrNoncanonicalFrame
        );

        let mut invalid_signature: SignedCollaborationMessage =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        invalid_signature.signature = "00".repeat(64);
        let invalid_signature = wrap_transport_frame(
            &fixture,
            &remote_key,
            &canonical_signed_collaboration_message_bytes(&invalid_signature).unwrap(),
        );
        assert_eq!(
            transport_rejection(&core, &invalid_signature, NOW),
            CollaborationTransportRejection::InvalidMessage
        );

        let mut wrong_network: SignedCollaborationMessage =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        wrong_network.payload.network_id = "another-network".to_string();
        let wrong_network = wrap_transport_frame(
            &fixture,
            &remote_key,
            &resign_message(&remote_key, wrong_network.payload),
        );
        assert_eq!(
            transport_rejection(&core, &wrong_network, NOW),
            CollaborationTransportRejection::InvalidMessage
        );

        let mut wrong_grant: SignedCollaborationMessage =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        wrong_grant.payload.conversation_id = "another-conversation".to_string();
        wrong_grant.payload.recipient.id = "another-conversation".to_string();
        let wrong_grant = wrap_transport_frame(
            &fixture,
            &remote_key,
            &resign_message(&remote_key, wrong_grant.payload),
        );
        assert_eq!(
            transport_rejection(&core, &wrong_grant, NOW),
            CollaborationTransportRejection::InvalidMessage
        );
        assert_eq!(
            transport_rejection(&core, &message, NOW + TTL),
            CollaborationTransportRejection::InvalidMessage
        );

        let self_message = fixture
            .authority(fixture.device_key.clone())
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"self"}),
                NOW,
                TTL,
            )
            .unwrap();
        assert_eq!(
            transport_rejection(
                &core,
                &fixture
                    .authority(fixture.device_key.clone())
                    .prepare_transport_frame(self_message.envelope_bytes())
                    .unwrap(),
                NOW,
            ),
            CollaborationTransportRejection::MessageSelfEcho
        );
        assert!(!core.state_path().exists());
    }

    #[test]
    fn transport_incoming_is_durable_replayable_and_retryable_on_capacity_or_write_failure() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote_key, message) = remote_transport_message(&fixture, generate_keypair().0, NOW);
        let accepted = transport_incoming(&core, &message, NOW);
        assert!(accepted.product_handoff_pending());
        let receipt = accepted.acceptance_receipt_bytes().to_vec();
        assert!(core.state_path().exists());

        let replay = transport_incoming(&fixture.core(), &message, NOW + 1);
        assert!(replay.product_handoff_pending());
        assert_eq!(replay.acceptance_receipt_bytes(), receipt);
        fixture
            .core()
            .acknowledge_product_handoff(&collaboration_message_envelope_sha256(
                &transport_frame_envelope_bytes(&message),
            ))
            .unwrap();
        let tombstone_replay = transport_incoming(&fixture.core(), &message, NOW + TTL);
        assert!(!tombstone_replay.product_handoff_pending());
        assert_eq!(tombstone_replay.acceptance_receipt_bytes(), receipt);

        let mut conflict: SignedCollaborationMessage =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message)).unwrap();
        conflict.payload.payload["product"] = serde_json::json!({"body":"conflict"});
        conflict.payload.nonce = "00112233445566778899aabbccddeeff".to_string();
        let conflict = wrap_transport_frame(
            &fixture,
            &remote_key,
            &resign_message(&remote_key, conflict.payload),
        );
        let before_conflict = fs::read(core.state_path()).unwrap();
        assert_eq!(
            transport_rejection(&fixture.core(), &conflict, NOW + 1),
            CollaborationTransportRejection::MessageIdentityConflict
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), before_conflict);

        let capacity_fixture = Fixture::new();
        let capacity_core = capacity_fixture.core();
        let (sender_key, _) = generate_keypair();
        // Spread over receive windows, so the backlog binds before the rate.
        let at = |index: usize| {
            NOW + (index / COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW) as u64
                * COMMUNITY_RATE_WINDOW_SECS
        };
        for index in 0..MAX_PENDING_INCOMING_PER_SENDER {
            let (_, frame) =
                remote_transport_message(&capacity_fixture, sender_key.clone(), at(index));
            transport_incoming(&capacity_core, &frame, at(index));
        }
        let last = at(MAX_PENDING_INCOMING_PER_SENDER - 1);
        let (_, over_capacity) = remote_transport_message(&capacity_fixture, sender_key, last);
        let before_capacity = fs::read(capacity_core.state_path()).unwrap();
        assert_eq!(
            transport_rejection(&capacity_core, &over_capacity, last),
            CollaborationTransportRejection::SenderBacklogFull
        );
        assert_eq!(
            fs::read(capacity_core.state_path()).unwrap(),
            before_capacity
        );

        let before_write_fixture = Fixture::new();
        let before_write_core = before_write_fixture.core();
        let (_, before_write_frame) =
            remote_transport_message(&before_write_fixture, generate_keypair().0, NOW);
        before_write_core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(before_write_core
            .ingest_transport_frame(&before_write_frame, NOW)
            .is_err());
        assert!(!before_write_core.state_path().exists());

        let indeterminate_fixture = Fixture::new();
        let indeterminate_core = indeterminate_fixture.core();
        let (_, indeterminate_frame) =
            remote_transport_message(&indeterminate_fixture, generate_keypair().0, NOW);
        indeterminate_core.inject_write_fault(WriteFault::AfterRename);
        assert!(indeterminate_core
            .ingest_transport_frame(&indeterminate_frame, NOW)
            .is_err());
        let recovered =
            transport_incoming(&indeterminate_fixture.core(), &indeterminate_frame, NOW);
        assert!(recovered.product_handoff_pending());
    }

    #[test]
    fn transport_acceptance_receipts_are_typed_durable_and_idempotent() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let outgoing = prepare(
            &core,
            "transport-receipt",
            serde_json::json!({"body":"outgoing"}),
            NOW,
            TTL,
        )
        .unwrap();
        let (recipient_key, _) = generate_keypair();
        let receipt = remote_transport_receipt(&fixture, &outgoing, recipient_key.clone(), NOW + 1);
        assert_eq!(
            core.ingest_transport_frame(&receipt, NOW + 1).unwrap(),
            CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::AcceptanceBeforeProductProjection
            )
        );
        assert!(core.load_state().unwrap().unwrap().outgoing[0]
            .remote_acceptance_receipts
            .is_empty());
        complete_projection(&core, &outgoing);
        assert_eq!(
            core.ingest_transport_frame(&receipt, NOW + 1).unwrap(),
            CollaborationTransportIngestion::RemoteAcceptance(
                CollaborationTransportRemoteAcceptance::Applied
            )
        );
        assert_eq!(
            fixture
                .core()
                .ingest_transport_frame(&receipt, NOW + 2)
                .unwrap(),
            CollaborationTransportIngestion::RemoteAcceptance(
                CollaborationTransportRemoteAcceptance::Replayed
            )
        );

        let conflicting =
            remote_transport_receipt(&fixture, &outgoing, recipient_key.clone(), NOW + 2);
        let before_conflict = fs::read(core.state_path()).unwrap();
        assert_eq!(
            transport_rejection(&core, &conflicting, NOW + 2),
            CollaborationTransportRejection::AcceptanceRecipientConflict
        );
        assert_eq!(fs::read(core.state_path()).unwrap(), before_conflict);

        let mut invalid: SignedCollaborationAcceptanceReceipt =
            serde_json::from_slice(&transport_frame_envelope_bytes(&receipt)).unwrap();
        invalid.signature = "00".repeat(64);
        let invalid = wrap_transport_frame(
            &fixture,
            &recipient_key,
            &canonical_signed_collaboration_acceptance_receipt_bytes(&invalid).unwrap(),
        );
        assert_eq!(
            transport_rejection(&core, &invalid, NOW + 2),
            CollaborationTransportRejection::InvalidAcceptanceReceipt
        );

        let local_receipt =
            remote_transport_receipt(&fixture, &outgoing, fixture.device_key.clone(), NOW + 2);
        assert_eq!(
            transport_rejection(&core, &local_receipt, NOW + 2),
            CollaborationTransportRejection::AcceptanceFromLocalDevice
        );

        let unmatched_fixture = Fixture::new();
        assert_eq!(
            transport_rejection(&unmatched_fixture.core(), &receipt, NOW + 2),
            CollaborationTransportRejection::AcceptanceWithoutOutgoingMessage
        );

        let schema_outgoing = prepare(
            &unmatched_fixture.core(),
            "schema-dispatch",
            serde_json::json!({"body":"schema dispatch"}),
            NOW,
            TTL,
        )
        .unwrap();
        let (schema_recipient, _) = generate_keypair();
        let schema_receipt = remote_transport_receipt(
            &unmatched_fixture,
            &schema_outgoing,
            schema_recipient.clone(),
            NOW + 1,
        );
        let mut receipt_as_message: SignedCollaborationAcceptanceReceipt =
            serde_json::from_slice(&transport_frame_envelope_bytes(&schema_receipt)).unwrap();
        receipt_as_message.payload.schema = COLLABORATION_MESSAGE_SCHEMA_V1.to_string();
        let receipt_as_message = wrap_transport_frame(
            &unmatched_fixture,
            &schema_recipient,
            &sign_acceptance_receipt(&schema_recipient, receipt_as_message.payload),
        );
        assert_eq!(
            transport_rejection(&unmatched_fixture.core(), &receipt_as_message, NOW + 1),
            CollaborationTransportRejection::InvalidMessage
        );

        let (message_key, message_as_receipt) =
            remote_transport_message(&unmatched_fixture, generate_keypair().0, NOW);
        let mut message_as_receipt: SignedCollaborationMessage =
            serde_json::from_slice(&transport_frame_envelope_bytes(&message_as_receipt)).unwrap();
        message_as_receipt.payload.schema = COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1.to_string();
        let message_as_receipt = wrap_transport_frame(
            &unmatched_fixture,
            &message_key,
            &resign_message(&message_key, message_as_receipt.payload),
        );
        assert_eq!(
            transport_rejection(&unmatched_fixture.core(), &message_as_receipt, NOW),
            CollaborationTransportRejection::InvalidAcceptanceReceipt
        );

        let capacity_fixture = Fixture::new();
        let capacity_core = capacity_fixture.core();
        let capacity_outgoing = prepare(
            &capacity_core,
            "receipt-capacity",
            serde_json::json!({"body":"capacity"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&capacity_core, &capacity_outgoing);
        for _ in 0..MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING {
            let receipt = remote_transport_receipt(
                &capacity_fixture,
                &capacity_outgoing,
                generate_keypair().0,
                NOW + 1,
            );
            assert!(matches!(
                capacity_core
                    .ingest_transport_frame(&receipt, NOW + 1)
                    .unwrap(),
                CollaborationTransportIngestion::RemoteAcceptance(
                    CollaborationTransportRemoteAcceptance::Applied
                )
            ));
        }
        let overflow = remote_transport_receipt(
            &capacity_fixture,
            &capacity_outgoing,
            generate_keypair().0,
            NOW + 1,
        );
        let before_overflow = fs::read(capacity_core.state_path()).unwrap();
        assert_eq!(
            transport_rejection(&capacity_core, &overflow, NOW + 1),
            CollaborationTransportRejection::AcceptanceNotStored
        );
        assert_eq!(
            fs::read(capacity_core.state_path()).unwrap(),
            before_overflow
        );

        let fault_fixture = Fixture::new();
        let fault_core = fault_fixture.core();
        let fault_outgoing = prepare(
            &fault_core,
            "receipt-write-fault",
            serde_json::json!({"body":"fault"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&fault_core, &fault_outgoing);
        let fault_receipt = remote_transport_receipt(
            &fault_fixture,
            &fault_outgoing,
            generate_keypair().0,
            NOW + 1,
        );
        let before_fault = fs::read(fault_core.state_path()).unwrap();
        fault_core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(fault_core
            .ingest_transport_frame(&fault_receipt, NOW + 1)
            .is_err());
        assert_eq!(fs::read(fault_core.state_path()).unwrap(), before_fault);

        fault_core.inject_write_fault(WriteFault::AfterRename);
        assert!(fault_core
            .ingest_transport_frame(&fault_receipt, NOW + 1)
            .is_err());
        assert_eq!(
            fault_fixture
                .core()
                .ingest_transport_frame(&fault_receipt, NOW + 1)
                .unwrap(),
            CollaborationTransportIngestion::RemoteAcceptance(
                CollaborationTransportRemoteAcceptance::Replayed
            )
        );
    }

    #[test]
    fn missing_projection_field_is_pending_and_cannot_hide_remote_acceptance() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let outgoing = prepare(
            &core,
            "legacy-pending-projection",
            serde_json::json!({"body":"pending"}),
            NOW,
            TTL,
        )
        .unwrap();
        let pending_bytes = fs::read(core.state_path()).unwrap();
        let pending_value: serde_json::Value = serde_json::from_slice(&pending_bytes).unwrap();
        assert!(pending_value["outgoing"][0]
            .get("local_product_projection")
            .is_none());
        let restarted = fixture.core();
        assert_eq!(
            restarted
                .pending_outgoing_product_projections(NOW)
                .unwrap()
                .len(),
            1
        );
        assert!(restarted.pending_outgoing(NOW).unwrap().is_empty());

        complete_projection(&restarted, &outgoing);
        let receipt = remote_receipt(&fixture, &outgoing, generate_keypair().0, NOW + 1);
        restarted
            .record_remote_acceptance(&receipt, NOW + 1)
            .unwrap();
        let mut inconsistent = restarted.load_state().unwrap().unwrap();
        inconsistent.outgoing[0].local_product_projection = OutgoingProductProjectionState::Pending;
        let inconsistent_bytes = canonical_state_bytes(&inconsistent).unwrap();
        let inconsistent_value: serde_json::Value =
            serde_json::from_slice(&inconsistent_bytes).unwrap();
        assert!(inconsistent_value["outgoing"][0]
            .get("local_product_projection")
            .is_none());
        write_owner_only(&restarted.state_path(), &inconsistent_bytes);
        assert!(fixture.core().load_state().is_err());
    }

    #[test]
    fn history_sidecar_keeps_originals_without_changing_live_state_format() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let outgoing = prepare(
            &core,
            "history-outgoing",
            serde_json::json!({"body":"local"}),
            NOW,
            TTL,
        )
        .unwrap();
        assert!(core.conversation_history(NOW).unwrap().is_empty());
        complete_projection(&core, &outgoing);
        assert!(!core.history_path().exists());
        core.retain_history_message(outgoing.envelope_bytes(), NOW)
            .unwrap();
        let live_before = fs::read(core.state_path()).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&live_before)
            .unwrap()
            .get("history")
            .is_none());
        let (remote, _) = generate_keypair();
        let (_, incoming) = remote_message(&fixture, remote, NOW + 1);
        let accepted = core
            .accept_incoming_from_signed_source_for_test(&incoming, NOW + 1)
            .unwrap();
        core.acknowledge_product_handoff(accepted.authorized_message().message().envelope_sha256())
            .unwrap();
        assert_eq!(core.conversation_history(NOW + 1).unwrap().len(), 1);
        let live_before = fs::read(core.state_path()).unwrap();
        core.retain_history_message(&incoming, NOW + 1).unwrap();
        assert_eq!(fs::read(core.state_path()).unwrap(), live_before);
        assert_eq!(
            fixture.core().conversation_history(NOW + TTL + 1).unwrap(),
            vec![outgoing.envelope_bytes().to_vec(), incoming.clone()]
        );
        core.retain_history_message(&incoming, NOW + TTL + 1)
            .unwrap();
        assert_eq!(core.conversation_history(NOW + TTL + 1).unwrap().len(), 2);
        assert!(core
            .conversation_history(NOW + CONVERSATION_HISTORY_RETENTION_SECS + 1)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn history_admission_preserves_live_expiry_and_refuses_foreign_or_altered_messages() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote, _) = generate_keypair();
        let (_, original) = remote_message(&fixture, remote, NOW);
        assert!(core
            .authorize_history_message(&original, NOW + TTL + 1)
            .is_ok());
        assert!(core
            .accept_incoming_from_signed_source_for_test(
                &original,
                NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1
            )
            .is_err());
        assert!(core
            .authorize_history_message(&original, NOW + CONVERSATION_HISTORY_RETENTION_SECS)
            .is_err());
        assert!(core
            .authorize_history_message(&original, NOW - MAX_COLLABORATION_CLOCK_SKEW_SECS - 1)
            .is_err());
        let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
        altered["payload"]["payload"]["product"]["body"] = serde_json::json!("altered");
        assert!(core
            .authorize_history_message(&serde_json::to_vec(&altered).unwrap(), NOW)
            .is_err());
        let foreign =
            alternate_grant_core(&fixture, fixture.device_key.clone(), "foreign-conversation");
        assert!(foreign.authorize_history_message(&original, NOW).is_err());
    }

    #[test]
    fn history_reservation_write_failures_precede_projection_and_retry_publication() {
        for fault in [
            WriteFault::BeforeWrite,
            WriteFault::AfterRename,
            WriteFault::BeforeAncestorSync,
        ] {
            let fixture = Fixture::new();
            let core = fixture.core();
            let now = now_secs();
            let (_, original) = remote_message(&fixture, generate_keypair().0, now);
            let originals = std::slice::from_ref(&original);
            core.inject_history_write_fault(fault);
            let mut called = false;
            assert!(core
                .with_history_projection(originals, now, |_| {
                    called = true;
                    Ok(())
                })
                .is_err());
            assert!(!called);
            assert!(core.conversation_history(now).unwrap().is_empty());

            let restarted = Arc::new(fixture.core());
            let pending = restarted.pending_history_projection(now).unwrap();
            if fault != WriteFault::BeforeWrite {
                assert_eq!(pending, originals);
                // An indeterminate reservation is fsynced again before Room.
                restarted.inject_history_write_fault(WriteFault::BeforeWrite);
                assert!(restarted
                    .with_history_projection(&pending, now, |_| {
                        called = true;
                        Ok(())
                    })
                    .is_err());
                assert!(!called);
            } else {
                assert!(pending.is_empty());
                assert!(!restarted.history_path().exists());
            }
            let product =
                crate::collaboration_product::CollaborationChatProductPort::new(restarted.clone())
                    .unwrap();
            product
                .project_history(&fixture.data_root, originals, now)
                .unwrap();
            assert!(restarted
                .pending_history_projection(now)
                .unwrap()
                .is_empty());
            assert_eq!(restarted.conversation_history(now).unwrap(), originals);
        }
    }

    #[test]
    fn history_publication_failure_preserves_binding_and_recovers_room_offline() {
        let fixture = Fixture::new();
        let core = Arc::new(fixture.core());
        let now = now_secs();
        let (remote_key, original) = remote_message(&fixture, generate_keypair().0, now);
        let authorized = core.authorize_history_message(&original, now).unwrap();
        let profile = authorized.sender_profile().document();
        let card = crate::room_service::RoomProfileCardView {
            schema: "elastos.profile-card/v1".to_string(),
            profile_id: profile.profile_did.clone(),
            display_name: profile.display_name.clone(),
            handle: profile.handle.clone(),
            updated_at: profile.updated_at,
        };
        let mut room_row = None;
        assert!(core
            .with_history_projection(std::slice::from_ref(&original), now, |candidates| {
                assert_eq!(candidates, std::slice::from_ref(&original));
                room_row = Some(crate::room_service::project_collaboration_text(
                    &fixture.data_root,
                    core.conversation_scope(),
                    authorized.message().envelope_sha256(),
                    &card,
                    "remote",
                    now,
                    crate::room_service::CollaborationTextOrigin::Incoming,
                )?);
                core.inject_history_write_fault(WriteFault::BeforeWrite);
                Ok(())
            })
            .is_err());
        let room_row = room_row.unwrap();
        assert!(core.conversation_history(now).unwrap().is_empty());
        assert_eq!(
            core.pending_history_projection(now).unwrap(),
            std::slice::from_ref(&original)
        );
        let mut alias: SignedCollaborationMessage = serde_json::from_slice(&original).unwrap();
        alias.payload.payload["product"]["body"] = serde_json::json!("signed alias");
        let conflict = resign_message(&remote_key, alias.payload);
        core.authorize_history_message(&conflict, now).unwrap();
        assert!(core
            .accept_incoming_from_signed_source_for_test(&conflict, now)
            .is_err());

        let restarted = Arc::new(fixture.core());
        let product =
            crate::collaboration_product::CollaborationChatProductPort::new(restarted.clone())
                .unwrap();
        product
            .retry_pending_history_projection(&fixture.data_root, now)
            .unwrap();
        assert!(restarted
            .pending_history_projection(now)
            .unwrap()
            .is_empty());
        assert_eq!(
            restarted.conversation_history(now).unwrap(),
            std::slice::from_ref(&original)
        );
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &core.local_device_did(),
            "Reader",
            "history publication retry test",
        )
        .unwrap();
        let completed = product
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(completed.objects.len(), 1);
        assert_eq!(completed.objects[0].seq, room_row.seq);
        assert_eq!(completed.objects[0].body.as_deref(), Some("remote"));
        let history: serde_json::Value =
            serde_json::from_slice(&fs::read(restarted.history_path()).unwrap()).unwrap();
        assert!(history.get("pending_projection_hashes").is_none());
    }

    #[test]
    fn cold_acceptance_sync_failure_returns_no_receipt_and_retries_saved_original() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (_, original) = remote_message(&fixture, generate_keypair().0, NOW);
        core.inject_write_fault(WriteFault::BeforeAncestorSync);
        assert!(core
            .accept_incoming_from_signed_source_for_test(&original, NOW)
            .is_err());
        assert!(core.verified_state.lock().unwrap().is_none());
        let saved = fs::read(core.state_path()).unwrap();
        let state: CoreState = serde_json::from_slice(&saved).unwrap();
        assert_eq!(state.incoming.len(), 1);
        assert_eq!(state.incoming[0].envelope.as_bytes(), original);
        let restarted = fixture.core();
        let retried = restarted
            .accept_incoming_from_signed_source_for_test(&original, NOW + 1)
            .unwrap();
        assert_eq!(
            retried.acceptance_receipt_bytes(),
            state.incoming[0].acceptance_receipt.as_bytes()
        );
        assert!(retried.product_handoff_pending());
        assert!(
            restarted
                .directory_syncs
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        );
        assert_eq!(fs::read(restarted.state_path()).unwrap(), saved);
        assert_eq!(restarted.pending_product_handoffs().unwrap().len(), 1);
    }

    #[test]
    fn history_retention_enforces_age_count_and_serialized_byte_bounds() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote, _) = generate_keypair();
        let authority = fixture.authority(remote);
        let mut state = core.empty_history();
        for offset in 0..205 {
            let outgoing = authority
                .prepare_outgoing(
                    SERVICE,
                    "elastos.chat.message/v1",
                    serde_json::json!({"body":format!("history {offset}")}),
                    NOW + offset,
                    TTL,
                )
                .unwrap();
            state
                .envelopes
                .push(String::from_utf8(outgoing.envelope_bytes().to_vec()).unwrap());
            prune_history(&mut state, NOW + 205).unwrap();
        }
        assert_eq!(state.envelopes.len(), MAX_CONVERSATION_HISTORY_MESSAGES);
        assert_eq!(
            serde_json::from_str::<SignedCollaborationMessage>(&state.envelopes[0])
                .unwrap()
                .payload
                .created_at,
            NOW + 5
        );
        let retained = state
            .envelopes
            .iter()
            .map(|envelope| envelope.as_bytes().to_vec())
            .collect::<Vec<_>>();
        // The product retention port owns directory creation and the write lock.
        // Seed this fresh root through that port, as durable projections do.
        core.retain_history_messages(&retained, NOW + 205).unwrap();
        assert_eq!(core.conversation_history(NOW + 205).unwrap(), retained);
        let before = fs::read(core.history_path()).unwrap();
        let older = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"older than the retained window"}),
                NOW,
                TTL,
            )
            .unwrap();
        assert!(core
            .history_projection_candidates(&[older.envelope_bytes().to_vec()], NOW + 205)
            .unwrap()
            .is_empty());
        let newest = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"new retained original"}),
                NOW + 205,
                TTL,
            )
            .unwrap();
        assert_eq!(
            core.history_projection_candidates(
                &[
                    state.envelopes[0].as_bytes().to_vec(),
                    newest.envelope_bytes().to_vec()
                ],
                NOW + 205
            )
            .unwrap(),
            vec![newest.envelope_bytes().to_vec()]
        );
        assert_eq!(fs::read(core.history_path()).unwrap(), before);
        for offset in 205..405 {
            let outgoing = authority
                .prepare_outgoing(
                    SERVICE,
                    "elastos.chat.message/v1",
                    serde_json::json!({"body":"🙂".repeat(crate::room_service::MAX_OBJECT_BODY_LEN)}),
                    NOW + offset,
                    TTL,
                )
                .unwrap();
            state
                .envelopes
                .push(String::from_utf8(outgoing.envelope_bytes().to_vec()).unwrap());
            prune_history(&mut state, NOW + 405).unwrap();
        }
        assert!(state.envelopes.len() < MAX_CONVERSATION_HISTORY_MESSAGES);
        assert!(canonical_history_bytes(&state).unwrap().len() <= MAX_CONVERSATION_HISTORY_BYTES);
        core.validate_history(&state).unwrap();
        prune_history(&mut state, NOW + 405 + CONVERSATION_HISTORY_RETENTION_SECS).unwrap();
        assert!(state.envelopes.is_empty());
    }

    #[test]
    fn history_refuses_live_identity_conflicts_after_original_cache_eviction_before_product_writes()
    {
        let fixture = Fixture::new();
        let core = std::sync::Arc::new(fixture.core());
        let product =
            crate::collaboration_product::CollaborationChatProductPort::new(core.clone()).unwrap();
        let now = now_secs();
        let (remote_key, _) = generate_keypair();
        let authority = fixture.authority(remote_key.clone());
        let original = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"original live message"}),
                now - 1_000,
                TTL,
            )
            .unwrap();
        core.accept_incoming_from_signed_source_for_test(original.envelope_bytes(), now - 999)
            .unwrap();

        // Both a reused ID and a reused nonce have valid current signatures,
        // but refer to different originals from the same Profile.
        let mut conflicts = Vec::new();
        for reuse_id in [true, false] {
            let mut altered: SignedCollaborationMessage =
                serde_json::from_slice(original.envelope_bytes()).unwrap();
            altered.payload.created_at = now;
            altered.payload.expires_at = now + TTL;
            altered.payload.payload["product"]["body"] = serde_json::json!("conflicting original");
            if reuse_id {
                altered.payload.nonce = random_hex_128().unwrap();
            } else {
                altered.payload.message_id = random_hex_128().unwrap();
            }
            let (signature, signer_did) = crate::crypto::domain_separated_sign(
                &remote_key,
                COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
                &canonical_collaboration_message_bytes(&altered.payload).unwrap(),
            );
            altered.signature = signature;
            altered.signer_did = signer_did;
            let bytes = canonical_signed_collaboration_message_bytes(&altered).unwrap();
            assert!(core.authorize_history_message(&bytes, now).is_ok());
            // A still-pending live envelope also owns its identity.
            assert!(core
                .history_projection_candidates(std::slice::from_ref(&bytes), now)
                .unwrap()
                .is_empty());
            conflicts.push(bytes);
        }
        assert_eq!(
            core.history_projection_candidates(&[original.envelope_bytes().to_vec()], now)
                .unwrap(),
            vec![original.envelope_bytes().to_vec()]
        );
        let handoff = product.pending_messages().unwrap().remove(0);
        product
            .project_handoff(&fixture.data_root, &handoff)
            .unwrap();
        assert_eq!(core.summary().unwrap().replay_tombstones, 1);

        // These signed originals model a participant cache, rather than new
        // local sends. Retention does not change the five-send live policy.
        let replacements = (0..MAX_CONVERSATION_HISTORY_MESSAGES)
            .map(|offset| {
                authority
                    .prepare_outgoing(
                        SERVICE,
                        "elastos.chat.message/v1",
                        serde_json::json!({"body":format!("cached history {offset}")}),
                        now - 999 + offset as u64,
                        TTL,
                    )
                    .unwrap()
                    .envelope_bytes()
                    .to_vec()
            })
            .collect::<Vec<_>>();
        core.retain_history_messages(&replacements, now).unwrap();
        let retained = core.conversation_history(now).unwrap();
        assert_eq!(retained.len(), MAX_CONVERSATION_HISTORY_MESSAGES);
        assert!(!retained.contains(&original.envelope_bytes().to_vec()));
        // Exact accepted originals remain valid replays even after eviction.
        assert!(core
            .history_projection_candidates(&[original.envelope_bytes().to_vec()], now)
            .unwrap()
            .is_empty());
        core.retain_history_message(original.envelope_bytes(), now)
            .unwrap();

        let new_original = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"otherwise valid catch-up"}),
                now,
                TTL,
            )
            .unwrap();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &core.local_device_did(),
            "Reader",
            "history identity test",
        )
        .unwrap();
        let before_poll = product
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(before_poll.objects.len(), 1);
        let room_path = elastos_common::localhost::rooted_localhost_fs_path(
            &fixture.data_root,
            crate::room_service::room_root_uri(),
        )
        .unwrap()
        .join("room/objects.json");
        let before_live = fs::read(core.state_path()).unwrap();
        // The session's "joined" system row also takes a Room seq outside this
        // collaboration namespace, so pin the catch-up row's own seq instead.
        let mut catch_up_seq = None;
        for conflict in conflicts {
            assert!(core
                .accept_incoming_from_signed_source_for_test(&conflict, now)
                .is_err());
            product
                .project_history(
                    &fixture.data_root,
                    &[new_original.envelope_bytes().to_vec(), conflict.clone()],
                    now,
                )
                .unwrap();
            assert!(core.retain_history_message(&conflict, now).is_err());
            assert_eq!(fs::read(core.state_path()).unwrap(), before_live);
            let after_poll = product
                .conversation_poll(&fixture.data_root, &session.token, 0)
                .unwrap();
            assert_eq!(after_poll.objects.len(), 2);
            assert_eq!(after_poll.objects[0].seq, before_poll.objects[0].seq);
            assert_eq!(after_poll.objects[0].body, before_poll.objects[0].body);
            assert_eq!(
                after_poll.objects[1].body.as_deref(),
                Some("otherwise valid catch-up")
            );
            assert!(after_poll.objects[1].seq > before_poll.latest_seq);
            assert_eq!(after_poll.latest_seq, after_poll.objects[1].seq);
            // The second conflict's batch must not re-project the catch-up row.
            assert_eq!(
                *catch_up_seq.get_or_insert(after_poll.objects[1].seq),
                after_poll.objects[1].seq
            );
            let saved = fs::read(&room_path).unwrap();
            product
                .project_history(
                    &fixture.data_root,
                    &[conflict, new_original.envelope_bytes().to_vec()],
                    now,
                )
                .unwrap();
            assert_eq!(fs::read(&room_path).unwrap(), saved);
        }
    }

    #[test]
    fn history_first_identity_conflicts_refuse_custody_and_retire_legacy_handoffs_on_hot_cache() {
        let fixture = Fixture::new();
        let core = Arc::new(fixture.core());
        let now = now_secs();
        let key = SigningKey::from_bytes(&[93u8; 32]);
        let author = fixture.authority(key.clone());
        let original = author
            .prepare_outgoing(
                SERVICE,
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":"history first"}),
                now,
                TTL,
            )
            .unwrap();
        core.retain_history_message(original.envelope_bytes(), now)
            .unwrap();
        let history = core.load_history().unwrap().unwrap();
        let before = fs::read(core.history_path()).unwrap();
        let mut conflicts = Vec::new();
        for reuse_id in [false, true] {
            let mut signed: SignedCollaborationMessage =
                serde_json::from_slice(original.envelope_bytes()).unwrap();
            signed.payload.payload["product"] = serde_json::json!({"body":"signed conflict"});
            if reuse_id {
                signed.payload.nonce = "01".repeat(16);
            } else {
                signed.payload.message_id = "02".repeat(16);
            }
            let bytes = resign_message(&key, signed.payload);
            let frame = author.prepare_transport_frame(&bytes).unwrap();
            core.authorize_history_message(&bytes, now).unwrap();
            for receiver in [core.clone(), Arc::new(fixture.core())] {
                assert_eq!(
                    transport_rejection(&receiver, &frame, now),
                    CollaborationTransportRejection::MessageIdentityConflict
                );
                assert!(receiver
                    .accept_incoming(&bytes, &author.local_device_did(), now)
                    .is_err());
                assert!(receiver
                    .accept_home_chat(&frame, &author.local_device_did(), now)
                    .is_err());
                assert!(receiver.pending_product_handoffs().unwrap().is_empty());
            }
            conflicts.push(bytes);
        }
        assert_eq!(fs::read(core.history_path()).unwrap(), before);
        assert!(!core.state_path().exists());

        // Model an older accepted slot, then a separate history publication.
        // The exact state cache still hits, so dispatch must recheck the sidecar.
        core.write_history(&core.empty_history()).unwrap();
        let incoming = core
            .authority
            .authorize_incoming(&conflicts[0], &author.local_device_did(), now)
            .unwrap();
        let receipt = core
            .authority
            .prepare_acceptance_receipt(&incoming, now)
            .unwrap();
        let mut legacy = core.empty_state();
        legacy.incoming.push(IncomingRecord {
            envelope: String::from_utf8(conflicts[0].clone()).unwrap(),
            acceptance_receipt: String::from_utf8(receipt.clone()).unwrap(),
            product_handoff: ProductHandoffState::Pending,
        });
        core.write_state(&legacy).unwrap();
        core.load_state().unwrap();
        assert!(core.verified_state.lock().unwrap().is_some());
        core.write_history(&history).unwrap();
        assert!(core.pending_product_handoffs().unwrap().is_empty());
        let repaired = fixture.core().load_state().unwrap().unwrap();
        assert!(repaired.incoming.is_empty());
        assert_eq!(repaired.incoming_tombstones.len(), 1);
        assert_eq!(
            repaired.incoming_tombstones[0]
                .acceptance_receipt
                .as_bytes(),
            receipt
        );
        let replay = fixture
            .core()
            .accept_incoming(&conflicts[0], &author.local_device_did(), now)
            .unwrap();
        assert!(!replay.product_handoff_pending());
        assert_eq!(replay.acceptance_receipt_bytes(), receipt);

        let healthy = author
            .prepare_outgoing(
                SERVICE,
                COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                serde_json::json!({"body":"healthy after conflict"}),
                now,
                TTL,
            )
            .unwrap();
        core.accept_incoming(healthy.envelope_bytes(), &author.local_device_did(), now)
            .unwrap();
        let port = crate::collaboration_product::CollaborationChatProductPort::new(Arc::new(
            fixture.core(),
        ))
        .unwrap();
        let pending = port.pending_messages().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            port.project_handoff(&fixture.data_root, &pending[0])
                .unwrap()
                .body
                .as_deref(),
            Some("healthy after conflict")
        );
        assert!(fixture
            .core()
            .pending_product_handoffs()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn history_batch_conflicts_keep_unrelated_originals_in_both_orders() {
        for reversed in [false, true] {
            let fixture = Fixture::new();
            let core = Arc::new(fixture.core());
            let now = now_secs();
            let key = SigningKey::from_bytes(&[94u8; 32]);
            let author = fixture.authority(key.clone());
            let first = author
                .prepare_outgoing(
                    SERVICE,
                    COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                    serde_json::json!({"body":"first candidate"}),
                    now,
                    TTL,
                )
                .unwrap();
            let mut changed: SignedCollaborationMessage =
                serde_json::from_slice(first.envelope_bytes()).unwrap();
            changed.payload.payload["product"] = serde_json::json!({"body":"second candidate"});
            let second = resign_message(&key, changed.payload);
            let unrelated = author
                .prepare_outgoing(
                    SERVICE,
                    COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE,
                    serde_json::json!({"body":"unrelated candidate"}),
                    now,
                    TTL,
                )
                .unwrap();
            let mut batch = vec![
                first.envelope_bytes().to_vec(),
                second,
                unrelated.envelope_bytes().to_vec(),
            ];
            if reversed {
                batch.swap(0, 1);
            }
            let selected = core.history_projection_candidates(&batch, now).unwrap();
            assert_eq!(selected.len(), 2);
            assert!(selected.contains(&batch[0]));
            assert!(selected.contains(&batch[2]));
            assert!(!selected.contains(&batch[1]));
            let product =
                crate::collaboration_product::CollaborationChatProductPort::new(core.clone())
                    .unwrap();
            product
                .project_history(&fixture.data_root, &batch, now)
                .unwrap();
            let saved = core.conversation_history(now).unwrap();
            assert_eq!(saved, selected);
            let session = crate::room_service::start_local_runtime_session(
                &fixture.data_root,
                &core.local_device_did(),
                "Reader",
                "batch conflict test",
            )
            .unwrap();
            let before = product
                .conversation_poll(&fixture.data_root, &session.token, 0)
                .unwrap();
            assert_eq!(before.objects.len(), 2);
            assert!(before
                .objects
                .iter()
                .any(|row| row.body.as_deref() == Some("unrelated candidate")));
            batch.reverse();
            product
                .project_history(&fixture.data_root, &batch, now)
                .unwrap();
            let after = product
                .conversation_poll(&fixture.data_root, &session.token, 0)
                .unwrap();
            assert_eq!(
                serde_json::to_value(after.objects).unwrap(),
                serde_json::to_value(before.objects).unwrap()
            );
            assert_eq!(after.latest_seq, before.latest_seq);
            assert_eq!(core.conversation_history(now).unwrap(), saved);
        }
    }

    #[test]
    fn absent_summary_is_pure_and_outgoing_idempotency_uses_the_exact_runtime_binding() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let managed_root = fixture.data_root.join("collaboration");
        assert_eq!(
            core.summary().unwrap(),
            CollaborationCoreSummary {
                live_unresolved_outgoing: 0,
                expired_unaccepted_outgoing: 0,
                remotely_accepted_outgoing: 0,
                pending_product_handoffs: 0,
                replay_tombstones: 0,
            }
        );
        assert!(!managed_root.exists());

        let payload = serde_json::json!({"body":"same text"});
        let first = prepare(&core, "send-1", payload.clone(), NOW, TTL).unwrap();
        let retry = prepare(&core, "send-1", payload.clone(), NOW + 20, TTL).unwrap();
        assert_eq!(first.envelope_bytes(), retry.envelope_bytes());
        assert_eq!(first.envelope_sha256(), retry.envelope_sha256());
        assert_eq!(first.envelope_sha256(), retry.envelope_sha256());

        let restarted = fixture.core();
        let restart_retry = prepare(&restarted, "send-1", payload.clone(), NOW + 40, TTL).unwrap();
        assert_eq!(first.envelope_bytes(), restart_retry.envelope_bytes());
        let duplicate_content =
            prepare(&restarted, "send-2", payload.clone(), NOW + 40, TTL).unwrap();
        assert_ne!(first.envelope_bytes(), duplicate_content.envelope_bytes());

        let changed = serde_json::json!({"body":"changed"});
        assert!(prepare(&restarted, "send-1", changed, NOW + 40, TTL)
            .unwrap_err()
            .to_string()
            .contains("another binding"));

        let mut malformed = operation(
            &restarted,
            "send-3",
            "elastos.chat.message/v1",
            &payload,
            TTL,
        );
        malformed.preview = serde_json::json!({"different":true});
        assert!(restarted
            .prepare_outgoing(
                malformed,
                SERVICE,
                "elastos.chat.message/v1",
                payload.clone(),
                NOW,
                TTL,
            )
            .unwrap_err()
            .to_string()
            .contains("canonical intent"));
        let mut wrong_method = operation(
            &restarted,
            "send-3",
            "elastos.chat.message/v1",
            &payload,
            TTL,
        );
        wrong_method.method = "room.view".to_string();
        assert!(restarted
            .prepare_outgoing(
                wrong_method,
                SERVICE,
                "elastos.chat.message/v1",
                payload,
                NOW,
                TTL,
            )
            .unwrap_err()
            .to_string()
            .contains("context"));
    }

    #[test]
    fn incoming_commit_replay_acknowledgement_and_restart_are_exact() {
        let expired_fixture = Fixture::new();
        let expired_core = expired_fixture.core();
        let (expired_sender, _) = generate_keypair();
        let (_, expired_message) = remote_message(&expired_fixture, expired_sender, NOW);
        assert!(expired_core
            .accept_incoming(
                &expired_message,
                &message_source_endpoint_did(&expired_message),
                NOW + TTL,
            )
            .unwrap_err()
            .to_string()
            .contains("expired"));
        assert!(!expired_core.state_path().exists());

        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote_key, _) = generate_keypair();
        let (remote_key, message_bytes) = remote_message(&fixture, remote_key, NOW);
        let accepted = core
            .accept_incoming(
                &message_bytes,
                &message_source_endpoint_did(&message_bytes),
                NOW,
            )
            .unwrap();
        assert!(accepted.product_handoff_pending());
        assert_eq!(
            accepted.authorized_message().message().envelope_sha256(),
            collaboration_message_envelope_sha256(&message_bytes)
        );
        let receipt = verify_stored_collaboration_acceptance_receipt(
            accepted.acceptance_receipt_bytes(),
            accepted.authorized_message().message(),
        )
        .unwrap();
        assert_eq!(
            receipt.envelope().payload.sender_profile_did,
            accepted
                .authorized_message()
                .message()
                .envelope()
                .payload
                .sender_profile_did
        );
        assert_eq!(
            receipt.envelope().payload.message_nonce,
            accepted
                .authorized_message()
                .message()
                .envelope()
                .payload
                .nonce
        );

        let restarted = fixture.core();
        let pending = restarted.pending_product_handoffs().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].authorized_message().message().envelope_sha256(),
            accepted.authorized_message().message().envelope_sha256()
        );
        let replay = restarted
            .accept_incoming(
                &message_bytes,
                &message_source_endpoint_did(&message_bytes),
                NOW + 1,
            )
            .unwrap();
        assert_eq!(
            replay.acceptance_receipt_bytes(),
            accepted.acceptance_receipt_bytes()
        );

        restarted
            .acknowledge_product_handoff(accepted.authorized_message().message().envelope_sha256())
            .unwrap();
        assert!(restarted.pending_product_handoffs().unwrap().is_empty());
        let after_expiry = fixture
            .core()
            .accept_incoming(
                &message_bytes,
                &message_source_endpoint_did(&message_bytes),
                NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS,
            )
            .unwrap();
        assert!(!after_expiry.product_handoff_pending());
        assert_eq!(
            after_expiry.acceptance_receipt_bytes(),
            accepted.acceptance_receipt_bytes()
        );

        let mut conflicting: SignedCollaborationMessage =
            serde_json::from_slice(&message_bytes).unwrap();
        conflicting.payload.payload["product"] = serde_json::json!({"body":"conflict"});
        conflicting.payload.nonce = "00112233445566778899aabbccddeeff".to_string();
        let conflicting = resign_message(&remote_key, conflicting.payload);
        assert!(fixture
            .core()
            .accept_incoming(
                &conflicting,
                &message_source_endpoint_did(&conflicting),
                NOW + 1
            )
            .unwrap_err()
            .to_string()
            .contains("reused"));

        let mut nonce_conflict: SignedCollaborationMessage =
            serde_json::from_slice(&message_bytes).unwrap();
        nonce_conflict.payload.message_id = "ffeeddccbbaa99887766554433221100".to_string();
        nonce_conflict.payload.payload["product"] = serde_json::json!({"body":"nonce conflict"});
        let nonce_conflict = resign_message(&remote_key, nonce_conflict.payload);
        assert!(fixture
            .core()
            .accept_incoming(
                &nonce_conflict,
                &message_source_endpoint_did(&nonce_conflict),
                NOW + 1,
            )
            .unwrap_err()
            .to_string()
            .contains("reused"));

        let retention_deadline = tombstone_retention_deadline(NOW);
        restarted
            .with_mutation(Some(retention_deadline), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert_eq!(
            restarted
                .load_state()
                .unwrap()
                .unwrap()
                .incoming_tombstones
                .len(),
            1
        );
        restarted
            .with_mutation(Some(retention_deadline + 1), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert!(restarted
            .load_state()
            .unwrap()
            .unwrap()
            .incoming_tombstones
            .is_empty());
    }

    #[test]
    fn custody_settles_presence_while_shared_chat_retries_its_original_lifetime() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let chat = prepare(
            &core,
            "shared-custody",
            serde_json::json!({"body":"retained original"}),
            NOW,
            TTL,
        )
        .unwrap();
        let presence_type = "elastos.chat.presence/v1";
        let presence_payload = serde_json::json!({});
        let presence = core
            .prepare_outgoing(
                operation(
                    &core,
                    "presence-custody",
                    presence_type,
                    &presence_payload,
                    45,
                ),
                SERVICE,
                presence_type,
                presence_payload,
                NOW,
                45,
            )
            .unwrap();
        let recipient = generate_keypair().0;
        for outgoing in [&chat, &presence] {
            complete_projection(&core, outgoing);
            core.record_remote_acceptance(
                &remote_receipt(&fixture, outgoing, recipient.clone(), NOW + 1),
                NOW + 1,
            )
            .unwrap();
        }
        let before = fs::read(core.state_path()).unwrap();
        let retry = fixture.core().pending_outgoing(NOW + 1).unwrap();
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].envelope_bytes(), chat.envelope_bytes());
        assert!(retry[0].remote_acceptance_recorded());
        assert_eq!(retry[0].created_at(), NOW);
        assert!(core.pending_outgoing(NOW + TTL).unwrap().is_empty());
        assert_eq!(fs::read(core.state_path()).unwrap(), before);
    }

    #[test]
    fn self_echo_remote_acceptance_and_recipient_replay_are_fail_closed() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let local_authority = fixture.authority(fixture.device_key.clone());
        let self_message = local_authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"self"}),
                NOW,
                TTL,
            )
            .unwrap();
        assert!(core
            .accept_incoming(
                self_message.envelope_bytes(),
                &message_source_endpoint_did(self_message.envelope_bytes()),
                NOW,
            )
            .unwrap_err()
            .to_string()
            .contains("echo"));

        let outgoing = prepare(
            &core,
            "remote-acceptance",
            serde_json::json!({"body":"hello"}),
            NOW,
            TTL,
        )
        .unwrap();
        complete_projection(&core, &outgoing);
        assert_eq!(core.pending_outgoing(NOW).unwrap().len(), 1);
        let (recipient_key, _) = generate_keypair();
        let receipt = remote_receipt(&fixture, &outgoing, recipient_key.clone(), NOW + 1);
        core.record_remote_acceptance(&receipt, NOW + 1).unwrap();
        core.record_remote_acceptance(&receipt, NOW + 1).unwrap();
        let retry = core.pending_outgoing(NOW + 1).unwrap();
        assert_eq!(retry.len(), 1);
        assert!(retry[0].remote_acceptance_recorded());
        assert_eq!(retry[0].envelope_bytes(), outgoing.envelope_bytes());
        assert!(core.pending_outgoing(NOW + TTL).unwrap().is_empty());

        let conflicting = remote_receipt(&fixture, &outgoing, recipient_key, NOW + 2);
        assert!(core
            .record_remote_acceptance(&conflicting, NOW + 2)
            .unwrap_err()
            .to_string()
            .contains("inconsistently"));

        let (second_recipient, _) = generate_keypair();
        let second = remote_receipt(&fixture, &outgoing, second_recipient, NOW + 2);
        core.record_remote_acceptance(&second, NOW + 2).unwrap();

        for _ in 2..MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING {
            let (recipient, _) = generate_keypair();
            let receipt = remote_receipt(&fixture, &outgoing, recipient, NOW + 2);
            core.record_remote_acceptance(&receipt, NOW + 2).unwrap();
        }
        let restarted = fixture.core();
        let state = restarted.load_state().unwrap().unwrap();
        assert_eq!(
            state.outgoing[0].remote_acceptance_receipts.len(),
            MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING
        );
        let (overflow_recipient, _) = generate_keypair();
        let overflow = remote_receipt(&fixture, &outgoing, overflow_recipient, NOW + 2);
        let before_overflow = fs::read(restarted.state_path()).unwrap();
        assert!(restarted
            .record_remote_acceptance(&overflow, NOW + 2)
            .unwrap_err()
            .to_string()
            .contains("capacity"));
        assert_eq!(fs::read(restarted.state_path()).unwrap(), before_overflow);

        let local_receipt =
            remote_receipt(&fixture, &outgoing, fixture.device_key.clone(), NOW + 2);
        assert!(restarted
            .record_remote_acceptance(&local_receipt, NOW + 2)
            .unwrap_err()
            .to_string()
            .contains("own outgoing"));

        let prune_now = NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1;
        prepare(
            &restarted,
            "accepted-prune-anchor",
            serde_json::json!({"body":"after accepted expiry"}),
            prune_now,
            TTL,
        )
        .unwrap();
        let state = fixture.core().load_state().unwrap().unwrap();
        assert_eq!(state.outgoing.len(), 1);
        assert_eq!(
            state.outgoing[0].operation.request_id,
            "accepted-prune-anchor"
        );
    }

    #[test]
    fn bounded_offline_outgoing_prunes_only_after_terminal_expiry() {
        let clock_fixture = Fixture::new();
        let clock_core = clock_fixture.core();
        prepare(
            &clock_core,
            "clock-rollback",
            serde_json::json!({"body":"clock rollback"}),
            NOW,
            TTL,
        )
        .unwrap();
        assert!(clock_core
            .pending_outgoing_product_projections(NOW - MAX_COLLABORATION_CLOCK_SKEW_SECS - 1)
            .unwrap()
            .is_empty());
        assert_eq!(
            clock_core
                .pending_outgoing_product_projections(NOW - MAX_COLLABORATION_CLOCK_SKEW_SECS)
                .unwrap()
                .len(),
            1
        );

        let fixture = Fixture::new();
        let mut core = fixture.core();
        // This test fills the outbox in one second; pacing has its own test.
        core.sends_per_window = usize::MAX;
        let mut originals = Vec::new();
        for index in 0..MAX_UNRESOLVED_OUTGOING {
            let original = prepare(
                &core,
                &format!("offline-{index}"),
                serde_json::json!({"body":format!("message {index}")}),
                NOW,
                1,
            )
            .unwrap();
            originals.push(original);
        }
        assert_eq!(
            core.pending_outgoing_product_projections(NOW)
                .unwrap()
                .len(),
            MAX_UNRESOLVED_OUTGOING
        );
        assert!(prepare(
            &core,
            "offline-blocked",
            serde_json::json!({"body":"blocked"}),
            NOW,
            1,
        )
        .unwrap_err()
        .to_string()
        .contains("capacity"));

        let prune_now = NOW + 1 + MAX_COLLABORATION_CLOCK_SKEW_SECS;
        assert!(prepare(
            &core,
            "offline-too-early",
            serde_json::json!({"body":"too_early"}),
            prune_now,
            1,
        )
        .unwrap_err()
        .to_string()
        .contains("capacity"));
        let restarted = fixture.core();
        let recovery = restarted
            .pending_outgoing_product_projections(prune_now + 1)
            .unwrap();
        assert_eq!(recovery.len(), originals.len());
        for (pending, original) in recovery.iter().zip(&originals) {
            assert_eq!(
                pending.outgoing().envelope_bytes(),
                original.envelope_bytes()
            );
        }
        assert!(restarted
            .pending_outgoing(prune_now + 1)
            .unwrap()
            .is_empty());
        assert!(prepare(
            &restarted,
            "offline-still-needs-projection",
            serde_json::json!({"body":"cannot discard pending intents"}),
            prune_now + 1,
            TTL,
        )
        .unwrap_err()
        .to_string()
        .contains("capacity"));
        for original in &originals {
            complete_projection(&restarted, original);
        }
        let admitted = prepare(
            &restarted,
            "offline-after-retention",
            serde_json::json!({"body":"admitted"}),
            prune_now + 1,
            TTL,
        )
        .unwrap();
        assert_eq!(
            restarted
                .pending_outgoing_product_projections(prune_now + 1)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fixture
                .core()
                .pending_outgoing_product_projections(prune_now + 1)
                .unwrap()[0]
                .outgoing()
                .envelope_bytes(),
            admitted.envelope_bytes()
        );
    }

    #[test]
    fn expired_pending_presence_is_acknowledged_before_terminal_pruning() {
        let fixture = Fixture::new();
        let core = Arc::new(fixture.core());
        let profile = core.authority.sender_profile_for_test().unwrap();
        let port =
            crate::collaboration_presence::CollaborationPresenceProductPort::new(core).unwrap();
        let original = port
            .prepare_presence(
                crate::collaboration_presence::presence_request_binding(
                    "expired-presence-recovery",
                    "runtime-principal",
                    &profile,
                )
                .unwrap(),
                &profile,
                NOW,
            )
            .unwrap();
        let after_expiry = original.expires_at() + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1;
        let restarted = Arc::new(fixture.core());
        restarted
            .with_mutation(Some(after_expiry), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert!(restarted.pending_outgoing(after_expiry).unwrap().is_empty());
        let restarted_port =
            crate::collaboration_presence::CollaborationPresenceProductPort::new(restarted.clone())
                .unwrap();
        let pending = restarted_port
            .pending_outgoing_presences(after_expiry)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].envelope_sha256(), original.envelope_sha256());
        assert_eq!(
            restarted_port
                .project_prepared_presence(&pending[0], after_expiry)
                .unwrap(),
            crate::collaboration_presence::CollaborationPresenceProjectionOutcome::Expired,
        );
        assert!(restarted_port
            .snapshot(after_expiry)
            .unwrap()
            .records()
            .is_empty());
        assert!(restarted
            .pending_outgoing_product_projections(after_expiry)
            .unwrap()
            .is_empty());
        restarted
            .with_mutation(Some(after_expiry), |_| {
                Ok(Mutation {
                    value: (),
                    changed: false,
                })
            })
            .unwrap();
        assert!(fixture
            .core()
            .load_state()
            .unwrap()
            .unwrap()
            .outgoing
            .is_empty());
    }

    #[test]
    fn incoming_capacity_is_reserved_before_receipt_and_never_evicts_pending_work() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender_key, _) = generate_keypair();
        for _ in 0..MAX_PENDING_INCOMING_PER_SENDER {
            let (_, bytes) = remote_message(&fixture, sender_key.clone(), NOW);
            core.accept_incoming(&bytes, &message_source_endpoint_did(&bytes), NOW)
                .unwrap();
        }
        let (_, over_sender) = remote_message(&fixture, sender_key, NOW);
        let before = fs::read(core.state_path()).unwrap();
        assert!(core
            .accept_incoming(
                &over_sender,
                &message_source_endpoint_did(&over_sender),
                NOW
            )
            .unwrap_err()
            .to_string()
            .contains("sender capacity"));
        assert_eq!(fs::read(core.state_path()).unwrap(), before);

        while core.pending_product_handoffs().unwrap().len() < MAX_PENDING_INCOMING {
            let (key, _) = generate_keypair();
            let (_, bytes) = remote_message(&fixture, key, NOW);
            core.accept_incoming(&bytes, &message_source_endpoint_did(&bytes), NOW)
                .unwrap();
        }
        let (another, _) = generate_keypair();
        let (_, over_total) = remote_message(&fixture, another, NOW);
        let before = fs::read(core.state_path()).unwrap();
        assert!(core
            .accept_incoming(&over_total, &message_source_endpoint_did(&over_total), NOW)
            .unwrap_err()
            .to_string()
            .contains("capacity"));
        assert_eq!(fs::read(core.state_path()).unwrap(), before);

        let first = core.pending_product_handoffs().unwrap()[0]
            .authorized_message()
            .message()
            .clone();
        let mut synthetic = core.load_state().unwrap().unwrap();
        synthetic.incoming_tombstones = (0..MAX_INCOMING_RECORDS_AND_TOMBSTONES)
            .map(|_| IncomingTombstone {
                acceptance_receipt: String::new(),
                retain_until: NOW + TTL,
            })
            .collect();
        assert!(core
            .ensure_incoming_capacity(&synthetic, &first, b"{}")
            .unwrap_err()
            .to_string()
            .contains("capacity"));

        let mut over_tombstone_bound = core.load_state().unwrap().unwrap();
        let receipt = over_tombstone_bound.incoming[0].acceptance_receipt.clone();
        over_tombstone_bound.incoming.clear();
        over_tombstone_bound.incoming_tombstones = (0..=MAX_INCOMING_RECORDS_AND_TOMBSTONES)
            .map(|_| IncomingTombstone {
                acceptance_receipt: receipt.clone(),
                retain_until: NOW + TTL + MAX_COLLABORATION_CLOCK_SKEW_SECS,
            })
            .collect();
        assert!(core
            .validate_state(&over_tombstone_bound)
            .unwrap_err()
            .to_string()
            .contains("entry limits"));
    }

    #[test]
    fn candidate_state_is_fully_validated_before_replacement() {
        let fixture = Fixture::new();
        let core = fixture.core();
        prepare(
            &core,
            "candidate-validation",
            serde_json::json!({"body":"candidate"}),
            NOW,
            TTL,
        )
        .unwrap();
        let original = fs::read(core.state_path()).unwrap();
        assert!(core
            .with_mutation(None, |state| {
                state.binding.grant_cid = "invalid".to_string();
                Ok(Mutation {
                    value: (),
                    changed: true,
                })
            })
            .unwrap_err()
            .to_string()
            .contains("binding"));
        assert_eq!(fs::read(core.state_path()).unwrap(), original);
    }

    #[test]
    fn stored_state_is_structurally_verified_after_expiry_and_rejects_tampering() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote, _) = generate_keypair();
        let (_, bytes) = remote_message(&fixture, remote, NOW);
        let accepted = core
            .accept_incoming(&bytes, &message_source_endpoint_did(&bytes), NOW)
            .unwrap();
        core.acknowledge_product_handoff(accepted.authorized_message().message().envelope_sha256())
            .unwrap();
        prepare(
            &core,
            "stored-signature-source",
            serde_json::json!({"body":"stored"}),
            NOW,
            TTL,
        )
        .unwrap();
        assert!(fixture.core().load_state().unwrap().is_some());

        let original = fs::read(core.state_path()).unwrap();

        let (other_device_key, _) = generate_keypair();
        let other_device_core = CollaborationCore::new(
            &fixture.data_root,
            other_device_key,
            fixture.profile.clone(),
            fixture.grant.clone(),
            OPERATION_CAPSULE,
        )
        .unwrap();
        other_device_core.ensure_state_directory().unwrap();
        write_owner_only(&other_device_core.state_path(), &original);
        assert!(other_device_core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("binding"));

        let other_grant_core =
            alternate_grant_core(&fixture, fixture.device_key.clone(), "another-conversation");
        other_grant_core.ensure_state_directory().unwrap();
        write_owner_only(&other_grant_core.state_path(), &original);
        assert!(other_grant_core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("binding"));

        let mut state: CoreState = serde_json::from_slice(&original).unwrap();
        state.binding.network_id = "another-network".to_string();
        fs::write(core.state_path(), canonical_state_bytes(&state).unwrap()).unwrap();
        assert!(core.summary().unwrap_err().to_string().contains("binding"));
        fs::write(core.state_path(), &original).unwrap();

        let mut noncanonical = original.clone();
        noncanonical.push(b'\n');
        fs::write(core.state_path(), noncanonical).unwrap();
        assert!(core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("not canonical"));
        fs::write(core.state_path(), &original).unwrap();

        let mut tampered_message: CoreState = serde_json::from_slice(&original).unwrap();
        let mut message_value: serde_json::Value =
            serde_json::from_str(&tampered_message.outgoing[0].envelope).unwrap();
        message_value["signature"] = serde_json::Value::String("invalid".to_string());
        tampered_message.outgoing[0].envelope = serde_json::to_string(&message_value).unwrap();
        fs::write(
            core.state_path(),
            canonical_state_bytes(&tampered_message).unwrap(),
        )
        .unwrap();
        assert!(core.summary().is_err());
        fs::write(core.state_path(), &original).unwrap();

        let mut tampered_receipt: CoreState = serde_json::from_slice(&original).unwrap();
        let mut receipt_value: serde_json::Value =
            serde_json::from_str(&tampered_receipt.incoming_tombstones[0].acceptance_receipt)
                .unwrap();
        receipt_value["signature"] = serde_json::Value::String("invalid".to_string());
        tampered_receipt.incoming_tombstones[0].acceptance_receipt =
            serde_json::to_string(&receipt_value).unwrap();
        fs::write(
            core.state_path(),
            canonical_state_bytes(&tampered_receipt).unwrap(),
        )
        .unwrap();
        assert!(core.summary().is_err());
        fs::write(core.state_path(), &original).unwrap();

        let mut wrong_conversation: CoreState = serde_json::from_slice(&original).unwrap();
        let signed: SignedCollaborationAcceptanceReceipt =
            serde_json::from_str(&wrong_conversation.incoming_tombstones[0].acceptance_receipt)
                .unwrap();
        let mut payload = signed.payload;
        payload.conversation_id = "another-conversation".to_string();
        wrong_conversation.incoming_tombstones[0].acceptance_receipt =
            String::from_utf8(sign_acceptance_receipt(&fixture.device_key, payload)).unwrap();
        fs::write(
            core.state_path(),
            canonical_state_bytes(&wrong_conversation).unwrap(),
        )
        .unwrap();
        assert!(core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("tombstone binding"));
        fs::write(core.state_path(), &original).unwrap();

        for delta in [-1_i64, 1_i64] {
            let mut altered_retention: CoreState = serde_json::from_slice(&original).unwrap();
            let retain_until = altered_retention.incoming_tombstones[0].retain_until;
            altered_retention.incoming_tombstones[0].retain_until = if delta < 0 {
                retain_until - 1
            } else {
                retain_until + 1
            };
            fs::write(
                core.state_path(),
                canonical_state_bytes(&altered_retention).unwrap(),
            )
            .unwrap();
            assert!(core
                .summary()
                .unwrap_err()
                .to_string()
                .contains("tombstone binding"));
        }
        fs::write(core.state_path(), &original).unwrap();

        let mut uppercase: CoreState = serde_json::from_slice(&original).unwrap();
        let receipt = &mut uppercase.incoming_tombstones[0].acceptance_receipt;
        let mut receipt_value: serde_json::Value = serde_json::from_str(receipt).unwrap();
        let hash = receipt_value["payload"]["message_envelope_sha256"]
            .as_str()
            .unwrap()
            .to_ascii_uppercase();
        receipt_value["payload"]["message_envelope_sha256"] = serde_json::Value::String(hash);
        *receipt = serde_json::to_string(&receipt_value).unwrap();
        fs::write(
            core.state_path(),
            canonical_state_bytes(&uppercase).unwrap(),
        )
        .unwrap();
        assert!(core.summary().is_err());
    }

    #[test]
    fn owner_only_no_follow_paths_and_each_write_failure_boundary_are_enforced() {
        for fault in [
            WriteFault::BeforeWrite,
            WriteFault::AfterFileSync,
            WriteFault::AfterRename,
        ] {
            let fixture = Fixture::new();
            let core = fixture.core();
            let payload = serde_json::json!({"body":format!("fault {}", fault as u8)});
            core.inject_write_fault(fault);
            assert!(prepare(&core, "faulted-send", payload.clone(), NOW, TTL).is_err());
            if fault == WriteFault::AfterRename {
                let persisted = fs::read(core.state_path()).unwrap();
                let state: CoreState = serde_json::from_slice(&persisted).unwrap();
                let exact = state.outgoing[0].envelope.as_bytes().to_vec();
                let retry =
                    prepare(&fixture.core(), "faulted-send", payload, NOW + 1, TTL).unwrap();
                assert_eq!(retry.envelope_bytes(), exact);
            } else {
                assert!(!core.state_path().exists());
            }
        }

        let fixture = Fixture::new();
        let core = fixture.core();
        prepare(
            &core,
            "secure-state",
            serde_json::json!({"body":"secure"}),
            NOW,
            TTL,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let data_root = core.product_data_root();
            for directory in [
                data_root.join("collaboration"),
                data_root.join("collaboration/default-conversation"),
                core.state_dir.clone(),
            ] {
                assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
            }
            assert_eq!(
                fs::metadata(core.lock_path()).unwrap().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(core.state_path()).unwrap().mode() & 0o777,
                0o600
            );

            fs::set_permissions(core.state_path(), fs::Permissions::from_mode(0o644)).unwrap();
            assert!(core
                .summary()
                .unwrap_err()
                .to_string()
                .contains("owner-only"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn lock_and_state_symlinks_fail_closed_and_file_lock_serializes_core_instances() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let lock_fixture = Fixture::new();
        let lock_core = lock_fixture.core();
        lock_core.ensure_state_directory().unwrap();
        let external_lock = lock_fixture._temp.path().join("external-lock");
        write_owner_only(&external_lock, b"");
        symlink(&external_lock, lock_core.lock_path()).unwrap();
        assert!(prepare(
            &lock_core,
            "symlink-lock",
            serde_json::json!({"body":"lock"}),
            NOW,
            TTL,
        )
        .is_err());

        let state_fixture = Fixture::new();
        let state_core = state_fixture.core();
        state_core.ensure_state_directory().unwrap();
        let external_state = state_fixture._temp.path().join("external-state");
        write_owner_only(&external_state, b"{}");
        symlink(&external_state, state_core.state_path()).unwrap();
        assert!(state_core.summary().is_err());

        let mode_fixture = Fixture::new();
        let mode_core = mode_fixture.core();
        mode_core.ensure_state_directory().unwrap();
        write_owner_only(&mode_core.lock_path(), b"");
        fs::set_permissions(mode_core.lock_path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(prepare(
            &mode_core,
            "insecure-lock",
            serde_json::json!({"body":"insecure lock"}),
            NOW,
            TTL,
        )
        .unwrap_err()
        .to_string()
        .contains("owner-only"));

        let serialized_fixture = Fixture::new();
        let first = Arc::new(serialized_fixture.core());
        first.ensure_state_directory().unwrap();
        let file_guard = lock_owner_only_file(&first.lock_path()).unwrap();
        let second = Arc::new(serialized_fixture.core());
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = prepare(
                &second,
                "serialized-send",
                serde_json::json!({"body":"serialized"}),
                NOW,
                TTL,
            );
            done_tx.send(result.map(|_| ())).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(file_guard);
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(first.load_state().unwrap().unwrap().outgoing.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn read_side_rejects_symlinked_empty_and_populated_managed_ancestors() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let core = fixture.core();
        let external_empty = fixture._temp.path().join("external-empty");
        fs::create_dir(&external_empty).unwrap();
        symlink(&external_empty, fixture.data_root.join("collaboration")).unwrap();
        assert!(core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("real directory"));

        fs::remove_file(fixture.data_root.join("collaboration")).unwrap();
        let external_root = fixture._temp.path().join("external-root");
        fs::create_dir(&external_root).unwrap();
        let external_core = CollaborationCore::new(
            &external_root,
            fixture.device_key.clone(),
            fixture.profile.clone(),
            fixture.grant.clone(),
            OPERATION_CAPSULE,
        )
        .unwrap();
        prepare(
            &external_core,
            "external-state",
            serde_json::json!({"body":"external"}),
            NOW,
            TTL,
        )
        .unwrap();
        symlink(
            external_root.join("collaboration"),
            fixture.data_root.join("collaboration"),
        )
        .unwrap();
        assert!(core
            .summary()
            .unwrap_err()
            .to_string()
            .contains("real directory"));
    }
}
