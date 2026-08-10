//! Profile-contact-gated direct message envelopes and protected local state.
//!
//! This module deliberately does not share the default conversation's gossip
//! storage or transport. A direct envelope names Profile identities; Carrier
//! routes the exact envelope to the endpoint selected from the accepted signed
//! Profile. The scoped envelope signer and the route endpoint are verified
//! independently.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::Engine as _;
use elastos_common::collaboration_protocol::{
    canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
    collaboration_message_envelope_sha256, CollaborationMessage, CollaborationRecipient,
    CollaborationRecipientKind, SignedCollaborationMessage, COLLABORATION_MESSAGE_SCHEMA_V1,
    COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1, MAX_COLLABORATION_CLOCK_SKEW_SECS,
    MAX_COLLABORATION_PAYLOAD_BYTES,
};
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer,
};
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};

use crate::auth::{read_principal_root_object, write_protected_principal_root_object};
use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument;
use crate::collaboration_protocol::{
    validate_id, validate_payload_type, verify_collaboration_acceptance_receipt,
    verify_collaboration_message, VerifiedCollaborationMessage,
};
use crate::crypto::{domain_separated_sign, encode_did_key};

pub(crate) const DIRECT_MESSAGE_PROVIDER_SCHEME: &str = "collaboration-direct";
pub(crate) const DIRECT_MESSAGE_PROVIDER_OP: &str = "deliver";
/// Contact revocations ride the same pair channel with their own op, because
/// their receive rule differs from a message's: an already-removed pair must
/// still settle the sender's retry instead of refusing an unknown sender.
pub(crate) const DIRECT_REVOCATION_PROVIDER_OP: &str = "revoke";

/// The explicit declared read policy for a conversation whose relationship
/// ended. Refusing a read is a capability, not a side effect: history the
/// product ingested stays readable after removal, while sending and receiving
/// stop. A future app-declared retention policy replaces this constant with a
/// per-app declaration; until then this is the one place the choice lives, and
/// Runtime enforces it rather than letting an emergent refusal decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectHistoryPolicy {
    ReadableAfterRemoval,
}

pub(crate) const DECLARED_DIRECT_HISTORY_POLICY: DirectHistoryPolicy =
    DirectHistoryPolicy::ReadableAfterRemoval;
pub(crate) const DIRECT_MESSAGE_PAYLOAD_TYPE: &str = "elastos.chat.direct-message/v1";
const DIRECT_MESSAGE_STATE_SCHEMA: &str = "elastos.chat.direct-message-store/v1";
const DIRECT_MESSAGE_STATE_OBJECT: &str = ".AppData/ElastOS/Chat/direct-messages.json";
const MAX_DIRECT_MESSAGE_STATE_BYTES: usize = 24 * 1024 * 1024;
const MAX_DIRECT_MESSAGES: usize = 4_096;
const MAX_DIRECT_MESSAGE_READ_RECORDS: usize = 200;
const MAX_DIRECT_MESSAGE_TEXT_BYTES: usize = 8 * 1024;
const DIRECT_MESSAGE_TTL_SECS: u64 = 24 * 60 * 60;
/// Declared end-of-life for direct chat messages: an envelope the peer never
/// acknowledged within its lifetime becomes terminal and visibly `expired`.
const DECLARED_DIRECT_MESSAGE_END_OF_LIFE: crate::collaboration_delivery::DeliveryEndOfLife =
    crate::collaboration_delivery::DeliveryEndOfLife::TerminalExpired;
const DIRECT_PROVIDER_TIMEOUT_MS: u64 = 5_000;
const MAX_DIRECT_CONTEXTS: usize = 32;
const MAX_DIRECT_WIRE_BASE64_BYTES: usize = 128 * 1024;

static DIRECT_MESSAGE_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct DirectMessageStore {
    data_root: PathBuf,
    principal_id: String,
    localhost_root: String,
    network: VerifiedCollaborationNetworkProfile,
    local_profile: VerifiedCollaborationProfileDocument,
}

#[derive(Clone)]
pub(crate) struct CollaborationDirectMessageService {
    inner: Arc<DirectServiceInner>,
}

struct DirectServiceInner {
    signing_key: SigningKey,
    network: VerifiedCollaborationNetworkProfile,
    registry: Arc<ProviderRegistry>,
    contexts: Mutex<BTreeMap<String, DirectContext>>,
}

#[derive(Clone)]
struct DirectContext {
    contact_store: Arc<crate::collaboration_contact_store::CollaborationContactStore>,
    message_store: Arc<DirectMessageStore>,
    profile: VerifiedCollaborationProfileDocument,
    authority: DirectContextAuthority,
}

#[derive(Clone)]
enum DirectContextAuthority {
    Session {
        session_id: String,
        proof_binding_id: Option<String>,
        grant_id: String,
    },
    /// Registered by the Runtime at startup for the person who owns this
    /// Home, so a contact can deliver to them whenever their Home is
    /// running. Receiving is gated by the durable relationship — a signed
    /// envelope from an accepted contact — not by whether its owner
    /// happens to have a browser open. Sending still requires a session,
    /// because sending acts on the person's behalf.
    ///
    /// Carries the proof binding it was registered for so the durable
    /// authority stays revocable: a session-owned context dies with its
    /// session, and this one must die when the passkey behind it is
    /// revoked, or revoking a stolen laptop's passkey would stop the tabs
    /// and leave the Runtime receiving.
    RuntimeOwned { proof_binding_id: String },
    #[cfg(test)]
    VerifiedForTest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectDeliveryRequest {
    op: String,
    message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectDeliveryResponse {
    status: String,
    receipt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectMessageRecord {
    pub(crate) envelope_bytes: Vec<u8>,
    pub(crate) incoming: bool,
    pub(crate) recorded_at: u64,
    pub(crate) receipt_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectDeliveryStatus {
    ReceiptSettled,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectApiError {
    InvalidRequest,
    ForbiddenConversation,
    IntentConflict,
    Authority,
    Internal,
}

impl std::fmt::Display for DirectApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "invalid direct message request",
            Self::ForbiddenConversation => "direct conversation is unavailable",
            Self::IntentConflict => "direct message request_id conflicts with durable intent",
            Self::Authority => "direct message authority is unavailable",
            Self::Internal => "direct message operation failed",
        })
    }
}

impl std::error::Error for DirectApiError {}

fn validate_direct_send_request(
    request_id: &str,
    conversation_id: &str,
    text: &str,
) -> Result<(), DirectApiError> {
    if validate_id(request_id, "direct message request_id").is_err()
        || validate_id(conversation_id, "direct conversation_id").is_err()
        || text.is_empty()
        || text.len() > MAX_DIRECT_MESSAGE_TEXT_BYTES
    {
        return Err(DirectApiError::InvalidRequest);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DirectConversationSummary {
    pub(crate) conversation_id: String,
    pub(crate) display_name: String,
    /// True once the relationship ended. The conversation stays listed —
    /// history is readable per the declared policy — but composing stops.
    pub(crate) removed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DirectMessageSummary {
    pub(crate) message_id: String,
    pub(crate) direction: &'static str,
    pub(crate) text: String,
    pub(crate) created_at: u64,
    pub(crate) delivery_state: &'static str,
}

pub(crate) struct DirectSendAuthority<'a> {
    pub(crate) contact_store: Arc<crate::collaboration_contact_store::CollaborationContactStore>,
    pub(crate) profile: VerifiedCollaborationProfileDocument,
    pub(crate) session_id: &'a str,
    pub(crate) proof_binding_id: Option<&'a str>,
    pub(crate) grant_id: &'a str,
    /// The authority actor the caller's validated launch token proved against
    /// the session grant. Browser windows act under Home authority, so their
    /// grants enumerate "home", not the executable capsule.
    pub(crate) authority_app: &'a str,
}

pub(crate) struct DirectSendIntent<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) conversation_id: &'a str,
    pub(crate) text: &'a str,
    pub(crate) now: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectMessageState {
    schema: String,
    network_id: String,
    local_profile_did: String,
    messages: Vec<StoredMessage>,
    receipts: Vec<StoredReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessage {
    envelope: String,
    incoming: bool,
    recorded_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredReceipt {
    message_envelope_sha256: String,
    envelope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectMessagePayload {
    pub(crate) request_id: String,
    pub(crate) text: String,
}

impl DirectMessageStore {
    pub(crate) fn new(
        data_root: &Path,
        principal_id: &str,
        localhost_root: &str,
        network: VerifiedCollaborationNetworkProfile,
        local_profile: VerifiedCollaborationProfileDocument,
        local_device_did: &str,
    ) -> anyhow::Result<Self> {
        if principal_id.trim().is_empty() || localhost_root.trim().is_empty() {
            anyhow::bail!("direct message store requires a principal-scoped root");
        }
        if !local_profile.authorizes_endpoint(local_device_did) {
            anyhow::bail!("direct message Profile does not authorize the current device DID");
        }
        Ok(Self {
            data_root: data_root.to_path_buf(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            network,
            local_profile,
        })
    }

    pub(crate) fn local_profile_did(&self) -> &str {
        &self.local_profile.document().profile_did
    }

    pub(crate) fn records(&self) -> anyhow::Result<Vec<DirectMessageRecord>> {
        let Some(state) = self.load()? else {
            return Ok(Vec::new());
        };
        state
            .messages
            .into_iter()
            .map(|entry| {
                let envelope_bytes = decode(&entry.envelope, "direct message")?;
                let message_hash = collaboration_message_envelope_sha256(&envelope_bytes);
                Ok(DirectMessageRecord {
                    envelope_bytes,
                    incoming: entry.incoming,
                    recorded_at: entry.recorded_at,
                    receipt_settled: state
                        .receipts
                        .iter()
                        .any(|receipt| receipt.message_envelope_sha256 == message_hash),
                })
            })
            .collect()
    }

    pub(crate) fn has_receipt(&self, message_hash: &str) -> anyhow::Result<bool> {
        Ok(self.load()?.is_some_and(|state| {
            state
                .receipts
                .iter()
                .any(|receipt| receipt.message_envelope_sha256 == message_hash)
        }))
    }

    fn receipt(&self, message_hash: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(state) = self.load()? else {
            return Ok(None);
        };
        state
            .receipts
            .iter()
            .find(|entry| entry.message_envelope_sha256 == message_hash)
            .map(|entry| decode(&entry.envelope, "direct receipt"))
            .transpose()
    }

    pub(crate) fn persist_message(
        &self,
        envelope_bytes: &[u8],
        incoming: bool,
        now: u64,
    ) -> anyhow::Result<bool> {
        self.persist_message_with_limits(
            envelope_bytes,
            incoming,
            now,
            MAX_DIRECT_MESSAGES,
            MAX_DIRECT_MESSAGE_STATE_BYTES,
        )
    }

    fn persist_message_with_limits(
        &self,
        envelope_bytes: &[u8],
        incoming: bool,
        now: u64,
        max_messages: usize,
        max_bytes: usize,
    ) -> anyhow::Result<bool> {
        self.validate_persisted_message(envelope_bytes, incoming)?;
        self.mutate(|state| {
            let hash = collaboration_message_envelope_sha256(envelope_bytes);
            if state
                .messages
                .iter()
                .map(|entry| decode(&entry.envelope, "direct message"))
                .collect::<anyhow::Result<Vec<_>>>()?
                .iter()
                .any(|bytes| collaboration_message_envelope_sha256(bytes) == hash)
            {
                return Ok(false);
            }
            state.messages.push(StoredMessage {
                envelope: encode(envelope_bytes),
                incoming,
                recorded_at: now,
            });
            self.prune_for_capacity(state, now, max_messages, max_bytes)?;
            Ok(true)
        })
    }

    fn prune_for_capacity(
        &self,
        state: &mut DirectMessageState,
        now: u64,
        max_messages: usize,
        max_bytes: usize,
    ) -> anyhow::Result<()> {
        self.validate_state_integrity(state)?;
        while state.messages.len() > max_messages
            || state.receipts.len() > max_messages
            || serde_json::to_vec(state)?.len() > max_bytes
        {
            // Terminal means the Runtime will never act on the record again:
            // the envelope's TTL (plus skew) has passed, so retry_pending has
            // stopped re-delivering and the read model reports it expired.
            // Settled pairs go first — message plus receipt, fully closed.
            // Abandoned records — expired with no receipt, the shape a peer
            // that never acknowledges produces — are terminal too; before they
            // were prunable, one dead contact could fill the store until every
            // write failed, wedging healthy conversations with it.
            let mut settled = Vec::new();
            let mut abandoned = Vec::new();
            for (index, entry) in state.messages.iter().enumerate() {
                let bytes = decode(&entry.envelope, "direct message")?;
                let message = crate::collaboration_protocol::verify_stored_collaboration_message(
                    &bytes,
                    &self.network,
                    "chat",
                )?;
                let hash = message.envelope_sha256().to_string();
                if message
                    .envelope()
                    .payload
                    .expires_at
                    .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
                    > now
                {
                    continue;
                }
                let receipts = state
                    .receipts
                    .iter()
                    .filter(|receipt| receipt.message_envelope_sha256 == hash)
                    .count();
                if receipts == 1 {
                    settled.push((entry.recorded_at, hash, index));
                } else if receipts == 0 {
                    abandoned.push((entry.recorded_at, hash, index));
                }
            }
            let candidate = settled
                .into_iter()
                .min()
                .or_else(|| abandoned.into_iter().min());
            let Some((_, hash, index)) = candidate else {
                // Every record is still live: unexpired envelopes the Runtime
                // is actively retrying or may still receive. Refusing the
                // write is honest backpressure, not a wedge.
                anyhow::bail!("direct message store has no safely prunable terminal record");
            };
            state.messages.remove(index);
            state
                .receipts
                .retain(|receipt| receipt.message_envelope_sha256 != hash);
        }
        Ok(())
    }

    pub(crate) fn persist_receipt(
        &self,
        message_hash: &str,
        receipt_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<bool> {
        self.persist_receipt_with_limits(
            message_hash,
            receipt_bytes,
            now,
            MAX_DIRECT_MESSAGES,
            MAX_DIRECT_MESSAGE_STATE_BYTES,
        )
    }

    fn persist_receipt_with_limits(
        &self,
        message_hash: &str,
        receipt_bytes: &[u8],
        now: u64,
        max_messages: usize,
        max_bytes: usize,
    ) -> anyhow::Result<bool> {
        self.mutate(|state| {
            let message = state
                .messages
                .iter()
                .find(|entry| {
                    decode(&entry.envelope, "direct message")
                        .ok()
                        .is_some_and(|bytes| {
                            collaboration_message_envelope_sha256(&bytes) == message_hash
                        })
                })
                .ok_or_else(|| anyhow::anyhow!("direct message receipt has no durable message"))?;
            let message = crate::collaboration_protocol::verify_stored_collaboration_message(
                &decode(&message.envelope, "direct message")?,
                &self.network,
                "chat",
            )?;
            crate::collaboration_protocol::verify_stored_collaboration_acceptance_receipt(
                receipt_bytes,
                &message,
            )?;
            if let Some(existing) = state
                .receipts
                .iter()
                .find(|entry| entry.message_envelope_sha256 == message_hash)
            {
                if decode(&existing.envelope, "direct receipt")? != receipt_bytes {
                    anyhow::bail!("direct message receipt conflicts with durable receipt");
                }
                return Ok(false);
            }
            state.receipts.push(StoredReceipt {
                message_envelope_sha256: message_hash.to_string(),
                envelope: encode(receipt_bytes),
            });
            self.prune_for_capacity(state, now, max_messages, max_bytes)?;
            Ok(true)
        })
    }

    fn validate_persisted_message(&self, bytes: &[u8], incoming: bool) -> anyhow::Result<()> {
        let message = crate::collaboration_protocol::verify_stored_collaboration_message(
            bytes,
            &self.network,
            "chat",
        )?;
        let payload: DirectMessagePayload =
            serde_json::from_value(message.envelope().payload.payload.clone())
                .context("invalid direct message payload")?;
        validate_id(&payload.request_id, "direct message request_id")?;
        if message.envelope().payload.payload_type != DIRECT_MESSAGE_PAYLOAD_TYPE
            || payload.text.is_empty()
            || payload.text.len() > MAX_DIRECT_MESSAGE_TEXT_BYTES
            || message.envelope().payload.recipient.kind != CollaborationRecipientKind::Profile
        {
            anyhow::bail!("direct message payload is invalid");
        }
        if (incoming
            && message.envelope().payload.recipient.id != self.local_profile.document().profile_did)
            || (!incoming
                && message.envelope().payload.sender_profile_did
                    != self.local_profile.document().profile_did)
        {
            anyhow::bail!("direct message direction does not match the local Profile");
        }
        Ok(())
    }

    fn object_uri(&self) -> String {
        format!("{}/{}", self.localhost_root, DIRECT_MESSAGE_STATE_OBJECT)
    }

    fn load(&self) -> anyhow::Result<Option<DirectMessageState>> {
        let uri = self.object_uri();
        let path = self.object_path()?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = read_principal_root_object(
            &self.data_root,
            &self.principal_id,
            &self.localhost_root,
            &uri,
            &path,
        )?;
        if bytes.len() > MAX_DIRECT_MESSAGE_STATE_BYTES {
            anyhow::bail!("direct message store exceeds its byte limit");
        }
        let state: DirectMessageState =
            serde_json::from_slice(&bytes).context("invalid direct message store")?;
        if serde_json::to_vec(&state)? != bytes {
            anyhow::bail!("direct message store is not canonical JSON");
        }
        self.validate_state(&state)?;
        Ok(Some(state))
    }

    fn validate_state(&self, state: &DirectMessageState) -> anyhow::Result<()> {
        self.validate_state_integrity(state)?;
        if state.messages.len() > MAX_DIRECT_MESSAGES || state.receipts.len() > MAX_DIRECT_MESSAGES
        {
            anyhow::bail!("direct message store exceeds its entry limit");
        }
        Ok(())
    }

    fn validate_state_integrity(&self, state: &DirectMessageState) -> anyhow::Result<()> {
        if state.schema != DIRECT_MESSAGE_STATE_SCHEMA
            || state.network_id != self.network.profile().network_id
            || state.local_profile_did != self.local_profile.document().profile_did
        {
            anyhow::bail!("direct message store binding or schema mismatch");
        }
        let mut message_ids = std::collections::HashSet::new();
        let mut nonces = std::collections::HashSet::new();
        let mut messages = std::collections::HashMap::new();
        let mut receipt_messages = std::collections::HashSet::new();
        for entry in &state.messages {
            let bytes = decode(&entry.envelope, "direct message")?;
            let message = crate::collaboration_protocol::verify_stored_collaboration_message(
                &bytes,
                &self.network,
                "chat",
            )?;
            if message.envelope().payload.payload_type != DIRECT_MESSAGE_PAYLOAD_TYPE {
                anyhow::bail!("stored direct message has an invalid payload type");
            }
            let payload: DirectMessagePayload =
                serde_json::from_value(message.envelope().payload.payload.clone())
                    .context("invalid stored direct message payload")?;
            if payload.text.is_empty() || payload.text.len() > MAX_DIRECT_MESSAGE_TEXT_BYTES {
                anyhow::bail!("stored direct message text has an invalid byte length");
            }
            if !message_ids.insert(message.envelope().payload.message_id.clone())
                || !nonces.insert(message.envelope().payload.nonce.clone())
            {
                anyhow::bail!("stored direct message conflicts with an existing message identity");
            }
            messages.insert(message.envelope_sha256().to_string(), message);
        }
        for receipt in &state.receipts {
            if !receipt_messages.insert(receipt.message_envelope_sha256.as_str()) {
                anyhow::bail!("stored direct receipt conflicts with an existing receipt");
            }
            let message = messages
                .get(&receipt.message_envelope_sha256)
                .ok_or_else(|| anyhow::anyhow!("stored direct receipt has no message"))?;
            crate::collaboration_protocol::verify_stored_collaboration_acceptance_receipt(
                &decode(&receipt.envelope, "direct receipt")?,
                message,
            )?;
        }
        Ok(())
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut DirectMessageState) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _guard = DIRECT_MESSAGE_MUTATION_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message mutation lock is poisoned"))?;
        let mut state = self.load()?.unwrap_or(DirectMessageState {
            schema: DIRECT_MESSAGE_STATE_SCHEMA.to_string(),
            network_id: self.network.profile().network_id.clone(),
            local_profile_did: self.local_profile.document().profile_did.clone(),
            messages: Vec::new(),
            receipts: Vec::new(),
        });
        let result = mutation(&mut state)?;
        self.validate_state(&state)?;
        let bytes = serde_json::to_vec(&state)?;
        if bytes.len() > MAX_DIRECT_MESSAGE_STATE_BYTES {
            anyhow::bail!("direct message store exceeds its byte limit");
        }
        let path = self.object_path()?;
        write_protected_principal_root_object(
            &self.data_root,
            &self.principal_id,
            &self.localhost_root,
            &self.object_uri(),
            &path,
            &bytes,
        )?;
        Ok(result)
    }

    fn object_path(&self) -> anyhow::Result<PathBuf> {
        elastos_common::localhost::rooted_localhost_fs_path(&self.data_root, &self.object_uri())
            .ok_or_else(|| anyhow::anyhow!("invalid direct message object path"))
    }
}

impl CollaborationDirectMessageService {
    fn compare_registered_profile(
        existing: &VerifiedCollaborationProfileDocument,
        incoming: &VerifiedCollaborationProfileDocument,
    ) -> anyhow::Result<std::cmp::Ordering> {
        let ordering = incoming
            .document()
            .revision
            .cmp(&existing.document().revision);
        if ordering == std::cmp::Ordering::Less {
            anyhow::bail!("direct message context Profile revision is stale");
        }
        if ordering == std::cmp::Ordering::Equal
            && crate::collaboration_discovery_runtime::signed_profile_bytes(incoming)?
                != crate::collaboration_discovery_runtime::signed_profile_bytes(existing)?
        {
            anyhow::bail!("direct message context Profile revision conflicts");
        }
        Ok(ordering)
    }

    #[cfg(test)]
    pub(crate) fn context_snapshot_for_test(&self) -> serde_json::Value {
        let Ok(contexts) = self.inner.contexts.lock() else {
            return serde_json::Value::String("poisoned".to_string());
        };
        serde_json::Value::Array(
            contexts
                .iter()
                .map(|(key, context)| {
                    let authority = match &context.authority {
                        DirectContextAuthority::Session {
                            session_id,
                            proof_binding_id,
                            grant_id,
                        } => serde_json::json!({
                            "kind": "session",
                            "session_id": session_id,
                            "proof_binding_id": proof_binding_id,
                            "grant_id": grant_id,
                        }),
                        DirectContextAuthority::RuntimeOwned { proof_binding_id } => {
                            serde_json::json!({
                                "kind": "runtime_owned",
                                "proof_binding_id": proof_binding_id,
                            })
                        }
                        DirectContextAuthority::VerifiedForTest => {
                            serde_json::json!({ "kind": "verified_for_test" })
                        }
                    };
                    let profile_bytes = serde_json::to_vec(context.profile.signed_envelope())
                        .expect("verified direct-message Profile must serialize");
                    serde_json::json!({
                        "key": key,
                        "profile_did": context.profile.document().profile_did,
                        "profile_revision": context.profile.document().revision,
                        "profile_hash": format!(
                            "sha256:{}",
                            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(profile_bytes))
                        ),
                        "authority": authority,
                    })
                })
                .collect(),
        )
    }

    pub(crate) async fn new(
        signing_key: SigningKey,
        network: VerifiedCollaborationNetworkProfile,
        registry: Arc<ProviderRegistry>,
    ) -> anyhow::Result<Self> {
        let inner = Arc::new(DirectServiceInner {
            signing_key,
            network,
            registry: registry.clone(),
            contexts: Mutex::new(BTreeMap::new()),
        });
        let provider: Arc<dyn Provider> = Arc::new(CollaborationDirectMessageProvider {
            inner: inner.clone(),
        });
        registry.register(provider.clone()).await;
        registry
            .register_sub_provider(DIRECT_MESSAGE_PROVIDER_SCHEME, provider)
            .await?;
        Ok(Self { inner })
    }

    pub(crate) fn register_context(
        &self,
        contact_store: Arc<crate::collaboration_contact_store::CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
        session_id: &str,
        proof_binding_id: Option<&str>,
        grant_id: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("direct message Profile does not match the contact store");
        }
        crate::collaboration_discovery_runtime::ensure_sync_context_authorized(
            contact_store.as_ref(),
            &profile,
            contact_store.principal_id(),
            session_id,
            proof_binding_id,
            grant_id,
            now,
        )?;
        let message_store = Arc::new(DirectMessageStore::new(
            contact_store.data_root(),
            contact_store.principal_id(),
            contact_store.localhost_root(),
            self.inner.network.clone(),
            profile.clone(),
            contact_store.local_device_did(),
        )?);
        let key = profile.document().profile_did.clone();
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?;
        if !contexts.contains_key(&key) && contexts.len() >= MAX_DIRECT_CONTEXTS {
            anyhow::bail!("direct message context limit reached");
        }
        if let Some(existing) = contexts.get(&key) {
            Self::compare_registered_profile(&existing.profile, &profile)?;
        }
        contexts.insert(
            key,
            DirectContext {
                contact_store,
                message_store,
                profile,
                authority: DirectContextAuthority::Session {
                    session_id: session_id.to_string(),
                    proof_binding_id: proof_binding_id.map(ToOwned::to_owned),
                    grant_id: grant_id.to_string(),
                },
            },
        );
        Ok(())
    }

    /// Register the receiving side for a Home's owner without a session.
    ///
    /// A Home that is running is reachable. Delivery used to require the
    /// recipient to have registered a context from a signed-in browser, so a
    /// running Home with no tab open refused messages from contacts it had
    /// already accepted, and the sender's queue only drained once the
    /// recipient opened Chat. The envelope is still verified and still has
    /// to come from an accepted contact; only the browser session is gone.
    pub(crate) fn register_runtime_owned_context(
        &self,
        contact_store: Arc<crate::collaboration_contact_store::CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
        proof_binding_id: &str,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("direct message Profile does not match the contact store");
        }
        let message_store = Arc::new(DirectMessageStore::new(
            contact_store.data_root(),
            contact_store.principal_id(),
            contact_store.localhost_root(),
            self.inner.network.clone(),
            profile.clone(),
            contact_store.local_device_did(),
        )?);
        let key = profile.document().profile_did.clone();
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?;
        if !contexts.contains_key(&key) && contexts.len() >= MAX_DIRECT_CONTEXTS {
            anyhow::bail!("direct message context limit reached");
        }
        // A context carries the Profile it was registered with, and the
        // owner editing their own Profile makes that copy stale — which
        // then refuses incoming mail from contacts who did nothing wrong.
        // Refresh the stored Profile when it advances, but never let a
        // Runtime-owned refresh weaken a live session authority.
        if let Some(existing) = contexts.get_mut(&key) {
            let ordering = Self::compare_registered_profile(&existing.profile, &profile)?;
            match &existing.authority {
                DirectContextAuthority::Session { .. } => {
                    if ordering == std::cmp::Ordering::Greater {
                        existing.contact_store = contact_store;
                        existing.message_store = message_store;
                        existing.profile = profile;
                    }
                    return Ok(());
                }
                DirectContextAuthority::RuntimeOwned { .. } => {
                    if ordering != std::cmp::Ordering::Greater {
                        return Ok(());
                    }
                }
                #[cfg(test)]
                DirectContextAuthority::VerifiedForTest => {
                    if ordering == std::cmp::Ordering::Greater {
                        existing.contact_store = contact_store;
                        existing.message_store = message_store;
                        existing.profile = profile;
                    }
                    return Ok(());
                }
            }
        }
        contexts.insert(
            key,
            DirectContext {
                contact_store,
                message_store,
                profile,
                authority: DirectContextAuthority::RuntimeOwned {
                    proof_binding_id: proof_binding_id.to_string(),
                },
            },
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn register_verified_context_for_test(
        &self,
        contact_store: Arc<crate::collaboration_contact_store::CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("direct message Profile does not match the contact store");
        }
        let message_store = Arc::new(DirectMessageStore::new(
            contact_store.data_root(),
            contact_store.principal_id(),
            contact_store.localhost_root(),
            self.inner.network.clone(),
            profile.clone(),
            contact_store.local_device_did(),
        )?);
        let key = profile.document().profile_did.clone();
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?;
        if !contexts.contains_key(&key) && contexts.len() >= MAX_DIRECT_CONTEXTS {
            anyhow::bail!("direct message context limit reached");
        }
        contexts.insert(
            key,
            DirectContext {
                contact_store,
                message_store,
                profile,
                authority: DirectContextAuthority::VerifiedForTest,
            },
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn persist_outgoing_for_test(
        &self,
        local_profile_did: &str,
        request_id: &str,
        conversation_id: &str,
        recipient_profile_did: &str,
        text: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let context = self.context(local_profile_did, now)?;
        let envelope = prepare_direct_message(
            &self.inner.signing_key,
            &self.inner.network,
            &context.profile,
            DirectMessageIntent {
                request_id,
                conversation_id,
                recipient_profile_did,
                text,
            },
            now,
        )?;
        context
            .message_store
            .persist_message(&envelope, false, now)?;
        Ok(envelope)
    }

    #[cfg(test)]
    pub(crate) fn records_for_test(
        &self,
        local_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<Vec<DirectMessageRecord>> {
        self.context(local_profile_did, now)?
            .message_store
            .records()
    }

    #[cfg(test)]
    pub(crate) async fn send_text(
        &self,
        local_profile_did: &str,
        request_id: &str,
        conversation_id: &str,
        text: &str,
        now: u64,
    ) -> Result<DirectDeliveryStatus, DirectApiError> {
        validate_direct_send_request(request_id, conversation_id, text)?;
        let context = self
            .context(local_profile_did, now)
            .map_err(|_| DirectApiError::Authority)?;
        self.send_text_with_context(&context, request_id, conversation_id, text, now)
            .await
    }

    pub(crate) async fn send_text_authorized(
        &self,
        authority: DirectSendAuthority<'_>,
        intent: DirectSendIntent<'_>,
    ) -> Result<DirectDeliveryStatus, DirectApiError> {
        validate_direct_send_request(intent.request_id, intent.conversation_id, intent.text)?;
        crate::collaboration_discovery_runtime::ensure_direct_context_authorized(
            authority.contact_store.as_ref(),
            &authority.profile,
            authority.contact_store.principal_id(),
            authority.session_id,
            authority.proof_binding_id,
            authority.grant_id,
            authority.authority_app,
            intent.now,
        )
        .map_err(|_| DirectApiError::Authority)?;
        let message_store = Arc::new(
            DirectMessageStore::new(
                authority.contact_store.data_root(),
                authority.contact_store.principal_id(),
                authority.contact_store.localhost_root(),
                self.inner.network.clone(),
                authority.profile.clone(),
                authority.contact_store.local_device_did(),
            )
            .map_err(|_| DirectApiError::Internal)?,
        );
        let context = DirectContext {
            contact_store: authority.contact_store,
            message_store,
            profile: authority.profile,
            authority: DirectContextAuthority::Session {
                session_id: authority.session_id.to_string(),
                proof_binding_id: authority.proof_binding_id.map(ToOwned::to_owned),
                grant_id: authority.grant_id.to_string(),
            },
        };
        self.send_text_with_context(
            &context,
            intent.request_id,
            intent.conversation_id,
            intent.text,
            intent.now,
        )
        .await
    }

    async fn send_text_with_context(
        &self,
        context: &DirectContext,
        request_id: &str,
        conversation_id: &str,
        text: &str,
        now: u64,
    ) -> Result<DirectDeliveryStatus, DirectApiError> {
        let contact = context
            .contact_store
            .snapshot()
            .map_err(|_| DirectApiError::Internal)?
            .contacts()
            .iter()
            .find(|contact| contact.conversation_id() == conversation_id)
            .cloned()
            .ok_or(DirectApiError::ForbiddenConversation)?;
        let mut existing_intent = None;
        for record in context
            .message_store
            .records()
            .map_err(|_| DirectApiError::Internal)?
            .into_iter()
            .filter(|record| !record.incoming)
        {
            let envelope: SignedCollaborationMessage =
                serde_json::from_slice(&record.envelope_bytes)
                    .map_err(|_| DirectApiError::Internal)?;
            let payload: DirectMessagePayload =
                serde_json::from_value(envelope.payload.payload.clone())
                    .map_err(|_| DirectApiError::Internal)?;
            if payload.request_id == request_id {
                existing_intent = Some((record, envelope, payload));
                break;
            }
        }
        if let Some((existing, envelope, payload)) = existing_intent {
            if envelope.payload.conversation_id != conversation_id
                || payload.text != text
                || envelope.payload.recipient.id != contact.remote_profile_did()
            {
                return Err(DirectApiError::IntentConflict);
            }
            return self
                .deliver(
                    context,
                    &existing.envelope_bytes,
                    contact.remote_presence_device_did(),
                    now,
                )
                .await
                .map_err(|_| DirectApiError::Internal);
        }
        let bytes = prepare_direct_message(
            &self.inner.signing_key,
            &self.inner.network,
            &context.profile,
            DirectMessageIntent {
                request_id,
                conversation_id,
                recipient_profile_did: contact.remote_profile_did(),
                text,
            },
            now,
        )
        .map_err(|_| DirectApiError::Internal)?;
        context
            .message_store
            .persist_message(&bytes, false, now)
            .map_err(|_| DirectApiError::Internal)?;
        self.deliver(context, &bytes, contact.remote_presence_device_did(), now)
            .await
            .map_err(|_| DirectApiError::Internal)
    }

    pub(crate) fn conversation_summaries(
        &self,
        contact_store: &crate::collaboration_contact_store::CollaborationContactStore,
    ) -> Result<Vec<DirectConversationSummary>, DirectApiError> {
        let snapshot = contact_store
            .snapshot()
            .map_err(|_| DirectApiError::Internal)?;
        let mut conversations = snapshot
            .contacts()
            .iter()
            .map(|contact| DirectConversationSummary {
                conversation_id: contact.conversation_id().to_string(),
                display_name: contact.remote_display_name().to_string(),
                removed: false,
            })
            .collect::<Vec<_>>();
        match DECLARED_DIRECT_HISTORY_POLICY {
            DirectHistoryPolicy::ReadableAfterRemoval => {
                conversations.extend(snapshot.removed().iter().map(|removed| {
                    DirectConversationSummary {
                        conversation_id: removed.conversation_id().to_string(),
                        display_name: removed.display_name().to_string(),
                        removed: true,
                    }
                }));
            }
        }
        conversations.sort_by(|left, right| left.conversation_id.cmp(&right.conversation_id));
        Ok(conversations)
    }

    pub(crate) fn message_summaries(
        &self,
        contact_store: &crate::collaboration_contact_store::CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        conversation_id: &str,
        now: u64,
    ) -> Result<Vec<DirectMessageSummary>, DirectApiError> {
        let contacts = contact_store
            .snapshot()
            .map_err(|_| DirectApiError::Internal)?;
        let accepted = contacts
            .contacts()
            .iter()
            .any(|contact| contact.conversation_id() == conversation_id);
        let removed = contacts
            .removed()
            .iter()
            .any(|removed| removed.conversation_id() == conversation_id);
        let readable = accepted
            || match DECLARED_DIRECT_HISTORY_POLICY {
                DirectHistoryPolicy::ReadableAfterRemoval => removed,
            };
        if !readable {
            return Err(DirectApiError::ForbiddenConversation);
        }
        let store = DirectMessageStore::new(
            contact_store.data_root(),
            contact_store.principal_id(),
            contact_store.localhost_root(),
            self.inner.network.clone(),
            profile.clone(),
            contact_store.local_device_did(),
        )
        .map_err(|_| DirectApiError::Internal)?;
        let mut messages = Vec::new();
        for record in store.records().map_err(|_| DirectApiError::Internal)? {
            let envelope: SignedCollaborationMessage =
                serde_json::from_slice(&record.envelope_bytes)
                    .map_err(|_| DirectApiError::Internal)?;
            let payload: DirectMessagePayload =
                serde_json::from_value(envelope.payload.payload.clone())
                    .map_err(|_| DirectApiError::Internal)?;
            if envelope.payload.conversation_id != conversation_id {
                continue;
            }
            messages.push(DirectMessageSummary {
                message_id: envelope.payload.message_id,
                direction: if record.incoming {
                    "incoming"
                } else {
                    "outgoing"
                },
                text: payload.text,
                created_at: envelope.payload.created_at,
                // `expired` is terminal: retry_pending stops re-delivering the
                // moment the envelope's TTL passes, so the product must stop
                // saying "Sending". A settled receipt still wins — expiry only
                // reclassifies messages the Runtime gave up on.
                delivery_state: if record.incoming {
                    "received"
                } else if record.receipt_settled {
                    "receipt_settled"
                } else if envelope.payload.expires_at <= now {
                    "expired"
                } else {
                    "pending"
                },
            });
        }
        messages.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.message_id.cmp(&right.message_id))
        });
        if messages.len() > MAX_DIRECT_MESSAGE_READ_RECORDS {
            messages.drain(..messages.len() - MAX_DIRECT_MESSAGE_READ_RECORDS);
        }
        Ok(messages)
    }

    /// Retries only the exact durable envelopes which have not yet received a
    /// matching receipt.  It never creates replacement message identities.
    pub(crate) async fn retry_pending(
        &self,
        local_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let context = self.context(local_profile_did, now)?;
        let contacts = context.contact_store.snapshot()?;
        let mut pending = Vec::new();
        for record in context.message_store.records()? {
            if record.incoming || record.receipt_settled {
                continue;
            }
            let raw: SignedCollaborationMessage = serde_json::from_slice(&record.envelope_bytes)
                .context("invalid durable direct message")?;
            if raw.payload.created_at > now.saturating_add(30)
                || raw.payload.expires_at <= now
                || raw.payload.recipient.kind != CollaborationRecipientKind::Profile
                || !contacts.contacts().iter().any(|contact| {
                    contact.conversation_id() == raw.payload.conversation_id
                        && contact.remote_profile_did() == raw.payload.recipient.id
                })
            {
                continue;
            }
            let Some(recipient) = contacts
                .contacts()
                .iter()
                .find(|contact| contact.remote_profile_did() == raw.payload.recipient.id)
                .map(|contact| contact.remote_presence_device_did().to_string())
            else {
                continue;
            };
            pending.push(crate::collaboration_delivery::DeliveryPlanItem {
                key: raw.payload.conversation_id,
                envelope: record.envelope_bytes,
                recipient_endpoint_did: recipient,
            });
            if pending.len() == 4 {
                break;
            }
        }
        // An unacknowledged message stops at its envelope lifetime and reads
        // `expired` — end-of-life is terminal and visible, never silent
        // re-delivery — so the selection above skips expired envelopes and
        // the pass below never re-mints.
        debug_assert!(matches!(
            DECLARED_DIRECT_MESSAGE_END_OF_LIFE,
            crate::collaboration_delivery::DeliveryEndOfLife::TerminalExpired
        ));
        let service = &self;
        let context = &context;
        crate::collaboration_delivery::run_bounded_delivery_pass(
            pending,
            |item| async move {
                let outcome = service
                    .deliver(context, &item.envelope, &item.recipient_endpoint_did, now)
                    .await
                    .map(|status| match status {
                        DirectDeliveryStatus::ReceiptSettled => {
                            crate::collaboration_delivery::DeliveryAttempt::Settled
                        }
                        DirectDeliveryStatus::Pending => {
                            crate::collaboration_delivery::DeliveryAttempt::Unreachable
                        }
                    });
                (item, outcome)
            },
            // Settlement is already durable before the pass sees it: deliver
            // persists the verified receipt into the message store itself.
            |_| Ok(()),
        )
        .await
    }

    fn context(&self, local_profile_did: &str, now: u64) -> anyhow::Result<DirectContext> {
        let context = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?
            .get(local_profile_did)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("direct message context is not registered"))?;
        self.revalidate_context(&context, now)?;
        Ok(context)
    }

    fn revalidate_context(&self, context: &DirectContext, now: u64) -> anyhow::Result<()> {
        match &context.authority {
            DirectContextAuthority::Session {
                session_id,
                proof_binding_id,
                grant_id,
            } => crate::collaboration_discovery_runtime::ensure_sync_context_authorized(
                context.contact_store.as_ref(),
                &context.profile,
                context.contact_store.principal_id(),
                session_id,
                proof_binding_id.as_deref(),
                grant_id,
                now,
            ),
            DirectContextAuthority::RuntimeOwned { proof_binding_id } => {
                // No session to re-check, so re-check what durably grants
                // this authority instead — and read it from disk, because
                // comparing two fields this context was built from proves
                // only that we built it, and would still say yes long
                // after the person revoked the passkey behind it.
                let principal = crate::auth::load_principal_for_proof_binding(
                    context.contact_store.data_root(),
                    proof_binding_id,
                )?;
                crate::auth::ensure_proof_binding_not_revoked(&principal)?;
                if principal.principal_id != context.contact_store.principal_id() {
                    anyhow::bail!("direct message principal no longer owns the contact store");
                }
                Ok(())
            }
            #[cfg(test)]
            DirectContextAuthority::VerifiedForTest => Ok(()),
        }
    }

    async fn deliver(
        &self,
        context: &DirectContext,
        message: &[u8],
        recipient_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<DirectDeliveryStatus> {
        let raw: SignedCollaborationMessage =
            serde_json::from_slice(message).context("invalid direct message envelope")?;
        let recipient_profile = context
            .contact_store
            .accepted_profile(&raw.payload.recipient.id)?
            .ok_or_else(|| anyhow::anyhow!("direct message recipient Profile is not accepted"))?;
        if recipient_profile.sole_endpoint_did()? != recipient_endpoint_did {
            anyhow::bail!("direct message route does not match the recipient Profile");
        }
        let verified = verify_direct_message(
            message,
            &self.inner.network,
            &raw.payload.conversation_id,
            &context.profile,
            &recipient_profile,
            now,
        )?;
        let message_hash = verified.envelope_sha256().to_string();
        if context.message_store.has_receipt(&message_hash)? {
            return Ok(DirectDeliveryStatus::ReceiptSettled);
        }
        let response = match self
            .inner
            .registry
            .invoke_provider(ProviderInvocation {
                source: "collaboration-direct".to_string(),
                target: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
                op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
                request: serde_json::to_value(DirectDeliveryRequest {
                    op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
                    message: encode(message),
                })?,
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                    peer_did: recipient_endpoint_did.to_string(),
                    timeout_ms: Some(DIRECT_PROVIDER_TIMEOUT_MS),
                }),
            })
            .await
        {
            Ok(response) => response,
            Err(err) => {
                // Pending is correct — the bounded retry pass owns durability —
                // but a swallowed error made the cold-path stall undiagnosable:
                // a measured idle pair stayed in "Sending" past six minutes on
                // the 5s dial budget and past two minutes on a 30s one, so the
                // dial is failing outright, not running out of time. Say why.
                tracing::debug!(
                    peer_did = %recipient_endpoint_did,
                    error = %err,
                    "direct message delivery attempt failed; envelope stays pending"
                );
                return Ok(DirectDeliveryStatus::Pending);
            }
        };
        let mut response = response;
        if let Some(object) = response.as_object_mut() {
            object.remove("_runtime_transfer");
        }
        let response: DirectDeliveryResponse =
            serde_json::from_value(response).context("invalid direct message provider response")?;
        if response.status != "ok" {
            anyhow::bail!("direct message provider rejected delivery");
        }
        if response.receipt.is_empty() || response.receipt.len() > MAX_DIRECT_WIRE_BASE64_BYTES {
            anyhow::bail!("direct message provider receipt has an invalid byte length");
        }
        let receipt_bytes = decode(&response.receipt, "direct message receipt")?;
        let receipt = verify_collaboration_acceptance_receipt(&receipt_bytes, &verified, now)?;
        if receipt.accepting_endpoint_did() != recipient_endpoint_did {
            anyhow::bail!("direct message receipt came from another endpoint");
        }
        context
            .message_store
            .persist_receipt(&message_hash, &receipt_bytes, now)?;
        Ok(DirectDeliveryStatus::ReceiptSettled)
    }

    /// Delivers one signed revocation to the pair's selected endpoint and verifies the
    /// acknowledgement. The caller owns durability; this is one attempt.
    pub(crate) async fn deliver_contact_revocation(
        &self,
        envelope_bytes: &[u8],
        recipient_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let verified = crate::collaboration_discovery::verify_collaboration_contact_revocation(
            envelope_bytes,
            &self.inner.network,
            now,
        )?;
        let response = self
            .inner
            .registry
            .invoke_provider(ProviderInvocation {
                source: "collaboration-direct".to_string(),
                target: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
                op: DIRECT_REVOCATION_PROVIDER_OP.to_string(),
                request: serde_json::to_value(DirectDeliveryRequest {
                    op: DIRECT_REVOCATION_PROVIDER_OP.to_string(),
                    message: encode(envelope_bytes),
                })?,
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                    peer_did: recipient_endpoint_did.to_string(),
                    timeout_ms: Some(DIRECT_PROVIDER_TIMEOUT_MS),
                }),
            })
            .await
            .map_err(|err| anyhow::anyhow!("contact revocation delivery failed: {err}"))?;
        let mut response = response;
        if let Some(object) = response.as_object_mut() {
            object.remove("_runtime_transfer");
        }
        let response: DirectDeliveryResponse = serde_json::from_value(response)
            .context("invalid contact revocation delivery response")?;
        if response.status != "ok" {
            anyhow::bail!("contact revocation delivery returned an error");
        }
        if response.receipt.is_empty() || response.receipt.len() > MAX_DIRECT_WIRE_BASE64_BYTES {
            anyhow::bail!("contact revocation receipt has an invalid byte length");
        }
        let receipt = decode(&response.receipt, "contact revocation receipt")?;
        let receipt = verify_collaboration_acceptance_receipt(&receipt, verified.message(), now)?;
        if receipt.accepting_endpoint_did() != recipient_endpoint_did {
            anyhow::bail!("contact revocation receipt came from another endpoint");
        }
        Ok(())
    }

    /// The recipient side of a revocation: applied by the pair's contact
    /// store, answered with the standard acceptance receipt. Idempotent — an
    /// already-removed pair acknowledges again rather than refusing, so the
    /// sender's durable retry settles.
    fn receive_contact_revocation(
        &self,
        message: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let raw: SignedCollaborationMessage =
            serde_json::from_slice(message).context("invalid contact revocation envelope")?;
        let recipient = &raw.payload.recipient;
        if recipient.kind != CollaborationRecipientKind::Profile {
            anyhow::bail!("contact revocation recipient must be a Profile");
        }
        let contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?;
        let context = contexts
            .values()
            .find(|context| context.message_store.local_profile_did() == recipient.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("contact revocation recipient is not registered"))?;
        drop(contexts);
        self.revalidate_context(&context, now)?;
        let verified = crate::collaboration_discovery::verify_collaboration_contact_revocation(
            message,
            &self.inner.network,
            now,
        )?;
        context
            .contact_store
            .apply_remote_contact_revocation(message, source_endpoint_did, now)?;
        let receipt = receipt_for(&self.inner.signing_key, verified.message(), now)?;
        Ok(receipt)
    }

    fn receive(
        &self,
        message: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let raw: SignedCollaborationMessage =
            serde_json::from_slice(message).context("invalid direct message envelope")?;
        let recipient = &raw.payload.recipient;
        if recipient.kind != CollaborationRecipientKind::Profile {
            anyhow::bail!("direct message recipient must be a Profile");
        }
        let contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("direct message context lock is poisoned"))?;
        let context = contexts
            .values()
            .find(|context| context.message_store.local_profile_did() == recipient.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("direct message recipient is not registered"))?;
        drop(contexts);
        self.revalidate_context(&context, now)?;
        let contact = context
            .contact_store
            .snapshot()?
            .contacts()
            .iter()
            .find(|contact| {
                contact.conversation_id() == raw.payload.conversation_id
                    && contact.remote_profile_did() == raw.payload.sender_profile_did
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("direct message sender is not an accepted current contact endpoint")
            })?;
        let sender_profile = context
            .contact_store
            .accepted_profile(contact.remote_profile_did())?
            .ok_or_else(|| anyhow::anyhow!("direct message sender Profile is unavailable"))?;
        if sender_profile.sole_endpoint_did()? != source_endpoint_did {
            anyhow::bail!("direct message did not arrive from the accepted Profile endpoint");
        }
        let verified = verify_direct_message(
            message,
            &self.inner.network,
            contact.conversation_id(),
            &sender_profile,
            &context.profile,
            now,
        )?;
        if let Some(receipt) = context.message_store.receipt(verified.envelope_sha256())? {
            return Ok(receipt);
        }
        context.message_store.persist_message(message, true, now)?;
        let receipt = receipt_for(&self.inner.signing_key, &verified, now)?;
        context
            .message_store
            .persist_receipt(verified.envelope_sha256(), &receipt, now)?;
        // The person is told a verified message arrived. Best effort by
        // design: the message is durably persisted above and a replay returns
        // the receipt before this point, so a failed notification write never
        // rejects the sender or re-notifies. The name is the signed contact
        // presentation this receive just verified against.
        let _ = crate::notifications::upsert_direct_message_notification(
            context.contact_store.data_root(),
            contact.conversation_id(),
            contact.remote_display_name(),
            now,
        );
        Ok(receipt)
    }
}

struct CollaborationDirectMessageProvider {
    inner: Arc<DirectServiceInner>,
}

#[async_trait::async_trait]
impl Provider for CollaborationDirectMessageProvider {
    async fn handle(
        &self,
        _request: elastos_runtime::provider::ResourceRequest,
    ) -> Result<elastos_runtime::provider::ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "direct messages do not expose resource routes".to_string(),
        ))
    }
    fn schemes(&self) -> Vec<&'static str> {
        vec![DIRECT_MESSAGE_PROVIDER_SCHEME]
    }
    fn name(&self) -> &'static str {
        "collaboration-direct-message"
    }
    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut request = request.clone();
        let object = request.as_object_mut().ok_or_else(|| {
            ProviderError::Provider("invalid direct message provider request".to_string())
        })?;
        let source_endpoint_did = validate_direct_runtime_invocation(
            object.get("_runtime_invocation"),
        )
        .map_err(|_| {
            ProviderError::Provider("invalid direct message provider invocation".to_string())
        })?;
        object.remove("_runtime_invocation");
        let request: DirectDeliveryRequest = serde_json::from_value(request).map_err(|_| {
            ProviderError::Provider("invalid direct message provider request".to_string())
        })?;
        if request.op != DIRECT_MESSAGE_PROVIDER_OP && request.op != DIRECT_REVOCATION_PROVIDER_OP {
            return Err(ProviderError::Provider(
                "invalid direct message provider request".to_string(),
            ));
        }
        if request.message.is_empty() || request.message.len() > MAX_DIRECT_WIRE_BASE64_BYTES {
            return Err(ProviderError::Provider(
                "invalid direct message provider request".to_string(),
            ));
        }
        let service = CollaborationDirectMessageService {
            inner: self.inner.clone(),
        };
        let bytes = decode(&request.message, "direct message").map_err(|_| {
            ProviderError::Provider("invalid direct message provider request".to_string())
        })?;
        let receipt = if request.op == DIRECT_REVOCATION_PROVIDER_OP {
            service
                .receive_contact_revocation(&bytes, &source_endpoint_did, now_ts())
                .map_err(|_| {
                    ProviderError::Provider("contact revocation delivery rejected".to_string())
                })?
        } else {
            service
                .receive(&bytes, &source_endpoint_did, now_ts())
                .map_err(|err| {
                    // The receiving side is where a refusal is explainable; say
                    // why here or the sender only ever learns "rejected".
                    tracing::debug!(error = %err, "direct message delivery rejected");
                    ProviderError::Provider("direct message delivery rejected".to_string())
                })?
        };
        let receipt = encode(&receipt);
        if receipt.len() > MAX_DIRECT_WIRE_BASE64_BYTES {
            return Err(ProviderError::Provider(
                "direct message receipt is too large".to_string(),
            ));
        }
        Ok(serde_json::to_value(DirectDeliveryResponse {
            status: "ok".to_string(),
            receipt,
        })
        .expect("direct response serializes"))
    }
}

fn validate_direct_runtime_invocation(value: Option<&serde_json::Value>) -> anyhow::Result<String> {
    let runtime = value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("direct message provider invocation is missing"))?;
    // Two ops share this plane; the capability must match the invoked op
    // exactly — a deliver grant is not a revoke grant.
    let op = match runtime.get("op").and_then(serde_json::Value::as_str) {
        Some(op @ (DIRECT_MESSAGE_PROVIDER_OP | DIRECT_REVOCATION_PROVIDER_OP)) => op,
        _ => anyhow::bail!("direct message provider invocation is invalid"),
    };
    let capability = format!("provider:{0}->{0}:{1}", DIRECT_MESSAGE_PROVIDER_SCHEME, op);
    for (field, expected) in [
        ("schema", "elastos.provider.invocation/v1"),
        ("source", DIRECT_MESSAGE_PROVIDER_SCHEME),
        ("target", DIRECT_MESSAGE_PROVIDER_SCHEME),
        ("op", op),
        ("capability", capability.as_str()),
        ("transport", "carrier-provider-plane"),
        ("transfer", "json"),
    ] {
        if runtime.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
            anyhow::bail!("direct message provider invocation is invalid");
        }
    }
    crate::collaboration_protocol::authenticated_carrier_source_endpoint(runtime.get("carrier"))
}

/// Signs the acceptance receipt for a verified collaboration message. Shared so
/// the profile update path settles deliveries with the same receipt contract.
pub(crate) fn acceptance_receipt_for(
    signing_key: &SigningKey,
    message: &VerifiedCollaborationMessage,
    now: u64,
) -> anyhow::Result<Vec<u8>> {
    receipt_for(signing_key, message, now)
}

fn receipt_for(
    signing_key: &SigningKey,
    message: &VerifiedCollaborationMessage,
    now: u64,
) -> anyhow::Result<Vec<u8>> {
    use elastos_common::collaboration_protocol::{
        canonical_collaboration_acceptance_receipt_bytes,
        canonical_signed_collaboration_acceptance_receipt_bytes, CollaborationAcceptanceReceipt,
        SignedCollaborationAcceptanceReceipt, COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1,
        COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
    };
    let payload = CollaborationAcceptanceReceipt {
        schema: COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1.to_string(),
        network_id: message.envelope().payload.network_id.clone(),
        message_envelope_sha256: message.envelope_sha256().to_string(),
        conversation_id: message.envelope().payload.conversation_id.clone(),
        sender_profile_did: message.envelope().payload.sender_profile_did.clone(),
        message_id: message.envelope().payload.message_id.clone(),
        message_nonce: message.envelope().payload.nonce.clone(),
        recipient_endpoint_did: encode_did_key(&signing_key.verifying_key()),
        accepted_at: now,
    };
    let (signature, signer_did) = domain_separated_sign(
        signing_key,
        COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
        &canonical_collaboration_acceptance_receipt_bytes(&payload)?,
    );
    canonical_signed_collaboration_acceptance_receipt_bytes(&SignedCollaborationAcceptanceReceipt {
        payload,
        signature,
        signer_did,
    })
    .map_err(Into::into)
}

pub(crate) struct DirectMessageIntent<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) conversation_id: &'a str,
    pub(crate) recipient_profile_did: &'a str,
    pub(crate) text: &'a str,
}

pub(crate) fn prepare_direct_message(
    signing_key: &SigningKey,
    network: &VerifiedCollaborationNetworkProfile,
    sender_profile: &VerifiedCollaborationProfileDocument,
    intent: DirectMessageIntent<'_>,
    now: u64,
) -> anyhow::Result<Vec<u8>> {
    validate_id(intent.conversation_id, "direct conversation_id")?;
    validate_id(intent.request_id, "direct message request_id")?;
    crate::crypto::decode_did_key(intent.recipient_profile_did)
        .context("invalid direct message recipient Profile DID")?;
    let signer_did = encode_did_key(&signing_key.verifying_key());
    if !sender_profile.authorizes_signer(&signer_did, "chat", DIRECT_MESSAGE_PAYLOAD_TYPE) {
        anyhow::bail!("direct message signer is not authorized by the sender Profile");
    }
    if intent.text.is_empty() || intent.text.len() > MAX_DIRECT_MESSAGE_TEXT_BYTES {
        anyhow::bail!("direct message text has an invalid byte length");
    }
    let payload = serde_json::to_value(DirectMessagePayload {
        request_id: intent.request_id.to_string(),
        text: intent.text.to_string(),
    })?;
    validate_payload_type(DIRECT_MESSAGE_PAYLOAD_TYPE)?;
    if serde_json::to_vec(&payload)?.len() > MAX_COLLABORATION_PAYLOAD_BYTES {
        anyhow::bail!("direct message payload is too large");
    }
    let expires_at = now
        .checked_add(DIRECT_MESSAGE_TTL_SECS)
        .context("direct message expiry overflows")?;
    let message = CollaborationMessage {
        schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
        network_id: network.profile().network_id.clone(),
        conversation_id: intent.conversation_id.to_string(),
        message_id: random_hex()?,
        nonce: random_hex()?,
        created_at: now,
        expires_at,
        sender_profile_did: sender_profile.document().profile_did.clone(),
        sender_service: "chat".to_string(),
        recipient: CollaborationRecipient {
            kind: CollaborationRecipientKind::Profile,
            id: intent.recipient_profile_did.to_string(),
        },
        payload_type: DIRECT_MESSAGE_PAYLOAD_TYPE.to_string(),
        payload,
    };
    let payload_bytes = canonical_collaboration_message_bytes(&message)?;
    let (signature, signer_did) = domain_separated_sign(
        signing_key,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
    );
    let envelope = canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
        payload: message,
        signature,
        signer_did,
    })?;
    verify_collaboration_message(&envelope, network, "chat", now)?;
    Ok(envelope)
}

pub(crate) fn verify_direct_message(
    bytes: &[u8],
    network: &VerifiedCollaborationNetworkProfile,
    conversation_id: &str,
    sender_profile: &VerifiedCollaborationProfileDocument,
    recipient_profile: &VerifiedCollaborationProfileDocument,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationMessage> {
    let message = verify_collaboration_message(bytes, network, "chat", now)?;
    let payload = &message.envelope().payload;
    if payload.conversation_id != conversation_id
        || payload.sender_profile_did != sender_profile.document().profile_did
        || payload.recipient.kind != CollaborationRecipientKind::Profile
        || payload.recipient.id != recipient_profile.document().profile_did
        || payload.payload_type != DIRECT_MESSAGE_PAYLOAD_TYPE
    {
        anyhow::bail!("direct message authority does not match the accepted contact");
    }
    if !sender_profile.authorizes_signer(
        &message.envelope().signer_did,
        "chat",
        DIRECT_MESSAGE_PAYLOAD_TYPE,
    ) {
        anyhow::bail!("direct message signer is not authorized by the sender Profile");
    }
    let payload: DirectMessagePayload = serde_json::from_value(payload.payload.clone())
        .context("invalid direct message payload")?;
    validate_id(&payload.request_id, "direct message request_id")?;
    if payload.text.is_empty() || payload.text.len() > MAX_DIRECT_MESSAGE_TEXT_BYTES {
        anyhow::bail!("direct message text has an invalid byte length");
    }
    Ok(message)
}

fn random_hex() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).context("OS randomness unavailable for direct message ID")?;
    Ok(hex::encode(bytes))
}
fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
fn decode(value: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .with_context(|| format!("invalid {label} encoding"))
}

#[cfg(test)]
mod tests;
