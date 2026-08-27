//! Durable provider-neutral state for one verified open default conversation.

use std::collections::{HashMap, HashSet};
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
    verify_stored_collaboration_message, VerifiedCollaborationMessage,
};
use crate::esp_binding::{esp_request_binding, EspRequestBinding, ESP_REQUEST_BINDING_SCHEMA};

const CORE_STATE_SCHEMA: &str = "elastos.collaboration.default-conversation-state/v1";
const CORE_STATE_DIR: &str = "collaboration/default-conversation";
const CORE_STATE_FILE: &str = "state-v1.json";
const CORE_LOCK_FILE: &str = "state-v1.lock";
const CORE_NAMESPACE_DOMAIN: &[u8] = b"elastos.collaboration.default-conversation-state.v1";
pub(crate) const DEFAULT_CONVERSATION_SEND_METHOD: &str = "message.send";
const MAX_CORE_STATE_BYTES: usize = 24 * 1024 * 1024;
const MAX_UNRESOLVED_OUTGOING: usize = 64;
const MAX_OUTGOING_RECORDS: usize = 4_096;
const MAX_PENDING_INCOMING: usize = 32;
const MAX_PENDING_INCOMING_PER_SENDER: usize = 8;
const MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING: usize = 32;
const MAX_INCOMING_RECORDS_AND_TOMBSTONES: usize = 4_096;

pub(crate) struct CollaborationCore {
    authority: DefaultConversationDeviceAuthority,
    operation_capsule: String,
    data_root: PathBuf,
    state_dir: PathBuf,
    mutation_mutex: Mutex<()>,
    #[cfg(test)]
    write_fault: std::sync::atomic::AtomicU8,
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
    pub(crate) fn envelope_bytes(&self) -> &[u8] {
        &self.envelope_bytes
    }

    pub(crate) fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }
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

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteFault {
    BeforeWrite = 1,
    AfterFileSync = 2,
    AfterRename = 3,
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
        Ok(Self {
            authority,
            operation_capsule: operation_capsule.to_string(),
            data_root: data_root.to_path_buf(),
            state_dir,
            mutation_mutex: Mutex::new(()),
            #[cfg(test)]
            write_fault: std::sync::atomic::AtomicU8::new(0),
        })
    }

    pub(crate) fn sender_service(&self) -> &str {
        self.authority.sender_service()
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
        let verified = verify_stored_collaboration_message(
            envelope_bytes,
            self.authority.profile(),
            self.authority.sender_service(),
        )?;
        authorize_default_conversation_message(self.authority.grant(), &verified)
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
                    entry.remote_acceptance_receipts.is_empty()
                        && stored_message_expiry(&entry.envelope)
                            .is_ok_and(|expires_at| expires_at > now)
                })
                .count(),
            expired_unaccepted_outgoing: state
                .outgoing
                .iter()
                .filter(|entry| {
                    entry.remote_acceptance_receipts.is_empty()
                        && stored_message_expiry(&entry.envelope)
                            .is_ok_and(|expires_at| expires_at <= now)
                })
                .count(),
            remotely_accepted_outgoing: state
                .outgoing
                .iter()
                .filter(|entry| !entry.remote_acceptance_receipts.is_empty())
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
        frame: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> Result<CollaborationTransportIngestion, CollaborationTransportRetryableError> {
        let verified = match verify_stored_collaboration_message(
            frame,
            self.authority.profile(),
            self.authority.sender_service(),
        ) {
            Ok(verified) => verified,
            Err(_) => {
                return Ok(CollaborationTransportIngestion::Rejected(
                    CollaborationTransportRejection::InvalidMessage,
                ))
            }
        };
        if authorize_default_conversation_transport_message(
            self.authority.grant(),
            &verified,
            source_endpoint_did,
        )
        .is_err()
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::InvalidMessage,
            ));
        }
        if verified.envelope().signer_did == self.authority.local_device_did() {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::MessageSelfEcho,
            ));
        }

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
        prune_terminal_state(&mut state, now).map_err(|_| CollaborationTransportRetryableError)?;
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
        if incoming_identity_conflicts(&state, incoming.message())
            .map_err(|_| CollaborationTransportRetryableError)?
        {
            return Ok(CollaborationTransportIngestion::Rejected(
                CollaborationTransportRejection::MessageIdentityConflict,
            ));
        }
        self.ensure_incoming_capacity(&state, incoming.message(), frame)
            .map_err(|_| CollaborationTransportRetryableError)?;

        let accepted = self
            .accept_incoming(frame, source_endpoint_did, now)
            .map_err(|_| CollaborationTransportRetryableError)?;
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
        let receipt = match verify_stored_acceptance_receipt_envelope(frame) {
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
        prune_terminal_state(&mut state, now).map_err(|_| CollaborationTransportRetryableError)?;
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
        for existing_bytes in &entry.remote_acceptance_receipts {
            let existing = verify_stored_acceptance_receipt_envelope(existing_bytes.as_bytes())
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
        if entry.remote_acceptance_receipts.len() >= MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING {
            return Err(CollaborationTransportRetryableError);
        }

        self.record_remote_acceptance(frame, now)
            .map_err(|_| CollaborationTransportRetryableError)?;
        Ok(CollaborationTransportIngestion::RemoteAcceptance(
            CollaborationTransportRemoteAcceptance::Applied,
        ))
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

        self.with_mutation(Some(now), |state| {
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
                validate_operation_for_intent(
                    &operation,
                    &self.operation_capsule,
                    payload_type,
                    &authenticated_payload,
                    ttl_secs,
                )?;
                return Ok(Mutation {
                    value: outgoing_handle(existing, verified),
                    changed: false,
                });
            }

            let unresolved = state
                .outgoing
                .iter()
                .filter(|entry| entry.remote_acceptance_receipts.is_empty())
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
            let value = outgoing_handle(&record, prepared.verified_message().clone());
            state.outgoing.push(record);
            ensure_state_fits(state)?;
            Ok(Mutation {
                value,
                changed: true,
            })
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

    pub(crate) fn pending_outgoing(&self, now: u64) -> anyhow::Result<Vec<DurableOutgoingMessage>> {
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        state
            .outgoing
            .iter()
            .filter_map(|entry| {
                if entry.local_product_projection == OutgoingProductProjectionState::Complete
                    && entry.remote_acceptance_receipts.is_empty()
                {
                    Some((entry, self.verify_outgoing_record(entry)))
                } else {
                    None
                }
            })
            .filter_map(|(entry, verified)| match verified {
                Ok(verified) if verified.envelope().payload.expires_at > now => {
                    if verified.envelope().payload.created_at
                        <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
                    {
                        Some(Ok(outgoing_handle(entry, verified)))
                    } else {
                        None
                    }
                }
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
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
                    if verified.envelope().payload.expires_at > now
                        && verified.envelope().payload.created_at
                            <= now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) =>
                {
                    Some(Ok(PendingOutgoingProductProjection {
                        outgoing: outgoing_handle(entry, verified),
                    }))
                }
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
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
        let receipt = verify_stored_acceptance_receipt_envelope(receipt_bytes)?;
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
            verify_collaboration_acceptance_receipt(receipt_bytes, &verified_message, now)?;

            for existing in &entry.remote_acceptance_receipts {
                let existing = verify_stored_acceptance_receipt_envelope(existing.as_bytes())?;
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
            if let Some(existing) =
                self.exact_incoming_replay(state, envelope_bytes, &envelope_hash, now)?
            {
                return Ok(Mutation {
                    value: existing,
                    changed: false,
                });
            }
            reject_incoming_conflicts(state, incoming.message())?;
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
        let state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        state
            .incoming
            .iter()
            .map(|entry| {
                let authorized = self.verify_incoming_record(entry)?;
                Ok(PendingProductHandoff { authorized })
            })
            .collect()
    }

    pub(crate) fn acknowledge_product_handoff(&self, envelope_hash: &str) -> anyhow::Result<()> {
        validate_sha256_label(envelope_hash)?;
        self.with_mutation(None, |state| {
            if state.incoming_tombstones.iter().any(|entry| {
                verify_stored_acceptance_receipt_envelope(entry.acceptance_receipt.as_bytes())
                    .map(|receipt| {
                        receipt.envelope().payload.message_envelope_sha256 == envelope_hash
                    })
                    .unwrap_or(false)
            }) {
                return Ok(Mutation {
                    value: (),
                    changed: false,
                });
            }
            let index = state
                .incoming
                .iter()
                .position(|entry| {
                    collaboration_message_envelope_sha256(entry.envelope.as_bytes())
                        == envelope_hash
                })
                .context("pending collaboration product handoff was not found")?;
            if state.incoming.len() + state.incoming_tombstones.len()
                > MAX_INCOMING_RECORDS_AND_TOMBSTONES
            {
                anyhow::bail!("collaboration replay tombstone capacity is exhausted");
            }
            let record = state.incoming.remove(index);
            let verified = self.verify_incoming_record(&record)?;
            let receipt = verify_stored_collaboration_acceptance_receipt(
                record.acceptance_receipt.as_bytes(),
                verified.message(),
            )?;
            state.incoming_tombstones.push(IncomingTombstone {
                acceptance_receipt: record.acceptance_receipt,
                retain_until: tombstone_retention_deadline(receipt.envelope().payload.accepted_at),
            });
            ensure_state_fits(state)?;
            Ok(Mutation {
                value: (),
                changed: true,
            })
        })
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
        if let Some(entry) = state.incoming_tombstones.iter().find(|entry| {
            entry.retain_until >= now
                && verify_stored_acceptance_receipt_envelope(entry.acceptance_receipt.as_bytes())
                    .map(|receipt| {
                        receipt.envelope().payload.message_envelope_sha256 == envelope_hash
                    })
                    .unwrap_or(false)
        }) {
            let verified = verify_stored_collaboration_message(
                envelope_bytes,
                self.authority.profile(),
                self.authority.sender_service(),
            )?;
            let authorized =
                authorize_default_conversation_message(self.authority.grant(), &verified)?;
            if verified.envelope().signer_did == self.authority.local_device_did() {
                anyhow::bail!("persisted incoming replay is a local self-echo");
            }
            verify_stored_collaboration_acceptance_receipt(
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
        let sender = &message.envelope().payload.sender_profile_did;
        let per_sender = state
            .incoming
            .iter()
            .filter_map(|entry| self.verify_incoming_record(entry).ok())
            .filter(|entry| entry.message().envelope().payload.sender_profile_did == *sender)
            .count();
        if per_sender >= MAX_PENDING_INCOMING_PER_SENDER {
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
        let verified = verify_stored_collaboration_message(
            entry.envelope.as_bytes(),
            self.authority.profile(),
            self.authority.sender_service(),
        )?;
        authorize_default_conversation_message(self.authority.grant(), &verified)?;
        if verified.envelope().signer_did != self.authority.local_device_did() {
            anyhow::bail!("persisted outgoing message is not signed by the local device");
        }
        validate_operation_for_message(&entry.operation, &self.operation_capsule, &verified)?;
        let mut recipients = HashSet::new();
        for receipt_bytes in &entry.remote_acceptance_receipts {
            let receipt = verify_stored_collaboration_acceptance_receipt(
                receipt_bytes.as_bytes(),
                &verified,
            )?;
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
        let receipt =
            verify_stored_acceptance_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
        let accepted_at = receipt.envelope().payload.accepted_at;
        let verified = verify_stored_collaboration_message(
            entry.envelope.as_bytes(),
            self.authority.profile(),
            self.authority.sender_service(),
        )?;
        let authorized = authorize_default_conversation_message(self.authority.grant(), &verified)?;
        if verified.envelope().signer_did == self.authority.local_device_did() {
            anyhow::bail!("persisted incoming message is a local self-echo");
        }
        verify_stored_collaboration_acceptance_receipt(
            entry.acceptance_receipt.as_bytes(),
            authorized.message(),
        )?;
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
        let _file_guard = ExclusiveFileLock::acquire(&self.lock_path())?;
        let mut state = self.load_state()?.unwrap_or_else(|| self.empty_state());
        let pruned = prune_now
            .map(|now| prune_terminal_state(&mut state, now))
            .transpose()?
            .unwrap_or(false);
        let mutation = f(&mut state)?;
        if mutation.changed || pruned {
            self.validate_state(&state)?;
            self.write_state(&state)?;
        }
        Ok(mutation.value)
    }

    fn load_state(&self) -> anyhow::Result<Option<CoreState>> {
        if !self.validate_existing_state_ancestors()? {
            return Ok(None);
        }
        let path = self.state_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        validate_owner_only_regular_file(&path, &metadata)?;
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
        if metadata.len() as usize > MAX_CORE_STATE_BYTES {
            anyhow::bail!("collaboration durable state exceeds its byte limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)?;
        let state: CoreState =
            serde_json::from_slice(&bytes).context("invalid collaboration durable state")?;
        if canonical_state_bytes(&state)? != bytes {
            anyhow::bail!("collaboration durable state is not canonical JSON");
        }
        self.validate_state(&state)?;
        Ok(Some(state))
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
                .filter(|entry| entry.remote_acceptance_receipts.is_empty())
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
            let receipt =
                verify_stored_acceptance_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
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
            File::open(&self.state_dir)?.sync_all()?;
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

fn transport_incoming_acceptance(
    accepted: DurableIncomingAcceptance,
) -> CollaborationTransportIncomingAcceptance {
    CollaborationTransportIncomingAcceptance {
        acceptance_receipt_bytes: accepted.acceptance_receipt_bytes,
        product_handoff_pending: accepted.product_handoff_pending,
    }
}

fn reject_incoming_conflicts(
    state: &CoreState,
    message: &VerifiedCollaborationMessage,
) -> anyhow::Result<()> {
    if incoming_identity_conflicts(state, message)? {
        anyhow::bail!("collaboration sender reused a message ID or nonce");
    }
    Ok(())
}

fn incoming_identity_conflicts(
    state: &CoreState,
    message: &VerifiedCollaborationMessage,
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
        let receipt =
            verify_stored_acceptance_receipt_envelope(entry.acceptance_receipt.as_bytes())?;
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
) -> DurableOutgoingMessage {
    DurableOutgoingMessage {
        envelope_bytes: entry.envelope.as_bytes().to_vec(),
        envelope_sha256: verified.envelope_sha256().to_string(),
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
        if expires_at.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) >= now {
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

pub(crate) struct ExclusiveFileLock {
    file: File,
}

impl ExclusiveFileLock {
    pub(crate) fn acquire(path: &Path) -> anyhow::Result<Self> {
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
                serde_json::json!({"content":"remote"}),
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
                serde_json::json!({"content":"self"}),
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
        conflict.payload.payload["product"] = serde_json::json!({"content":"conflict"});
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
        for _ in 0..MAX_PENDING_INCOMING_PER_SENDER {
            let (_, frame) = remote_transport_message(&capacity_fixture, sender_key.clone(), NOW);
            transport_incoming(&capacity_core, &frame, NOW);
        }
        let (_, over_capacity) = remote_transport_message(&capacity_fixture, sender_key, NOW);
        let before_capacity = fs::read(capacity_core.state_path()).unwrap();
        assert!(capacity_core
            .ingest_transport_frame(&over_capacity, NOW)
            .is_err());
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
            serde_json::json!({"content":"outgoing"}),
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
            serde_json::json!({"schema":"dispatch"}),
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
            serde_json::json!({"capacity":true}),
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
        assert!(capacity_core
            .ingest_transport_frame(&overflow, NOW + 1)
            .is_err());
        assert_eq!(
            fs::read(capacity_core.state_path()).unwrap(),
            before_overflow
        );

        let fault_fixture = Fixture::new();
        let fault_core = fault_fixture.core();
        let fault_outgoing = prepare(
            &fault_core,
            "receipt-write-fault",
            serde_json::json!({"fault":true}),
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
            serde_json::json!({"content":"pending"}),
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

        let payload = serde_json::json!({"content":"same text"});
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

        let changed = serde_json::json!({"content":"changed"});
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
        conflicting.payload.payload["product"] = serde_json::json!({"content":"conflict"});
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
        nonce_conflict.payload.payload["product"] = serde_json::json!({"content":"nonce conflict"});
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
    fn self_echo_remote_acceptance_and_recipient_replay_are_fail_closed() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let local_authority = fixture.authority(fixture.device_key.clone());
        let self_message = local_authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"content":"self"}),
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
            serde_json::json!({"content":"hello"}),
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
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());

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
            serde_json::json!({"after":"accepted expiry"}),
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
            serde_json::json!({"clock":"rollback"}),
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
        let core = fixture.core();
        for index in 0..MAX_UNRESOLVED_OUTGOING {
            prepare(
                &core,
                &format!("offline-{index}"),
                serde_json::json!({"index":index}),
                NOW,
                1,
            )
            .unwrap();
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
            serde_json::json!({"blocked":true}),
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
            serde_json::json!({"too_early":true}),
            prune_now,
            1,
        )
        .unwrap_err()
        .to_string()
        .contains("capacity"));
        let admitted = prepare(
            &core,
            "offline-after-retention",
            serde_json::json!({"admitted":true}),
            prune_now + 1,
            TTL,
        )
        .unwrap();
        assert_eq!(
            core.pending_outgoing_product_projections(prune_now + 1)
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
            serde_json::json!({"candidate":true}),
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
            serde_json::json!({"stored":true}),
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
            let payload = serde_json::json!({"fault":fault as u8});
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
            serde_json::json!({"secure":true}),
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
            serde_json::json!({"lock":true}),
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
            serde_json::json!({"lock":"insecure"}),
            NOW,
            TTL,
        )
        .unwrap_err()
        .to_string()
        .contains("owner-only"));

        let serialized_fixture = Fixture::new();
        let first = Arc::new(serialized_fixture.core());
        first.ensure_state_directory().unwrap();
        let file_guard = ExclusiveFileLock::acquire(&first.lock_path()).unwrap();
        let second = Arc::new(serialized_fixture.core());
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = prepare(
                &second,
                "serialized-send",
                serde_json::json!({"serialized":true}),
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
            serde_json::json!({"external":true}),
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
