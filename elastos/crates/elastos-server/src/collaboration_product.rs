//! Typed product boundary for the verified default collaboration conversation.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::collaboration_core::{
    CollaborationCore, DurableOutgoingMessage, PendingProductHandoff,
    DEFAULT_CONVERSATION_SEND_METHOD,
};
use crate::esp_binding::{esp_request_binding, EspRequestBinding};
use crate::room_service::CollaborationTextOrigin;

pub(crate) const CHAT_SERVICE: &str = "chat";
pub(crate) const CHAT_ROOM_CAPSULE: &str = "chat-room";
pub(crate) const CHAT_INTERFACE: &str = "elastos.chat.room";
const CHAT_RESOURCE: &str = "elastos://chat/message";
const CHAT_PAYLOAD_TYPE: &str = "elastos.chat.message/v1";
const CHAT_MESSAGE_TTL_SECS: u64 = 300;

/// Cloneable opaque port for the one configured Chat collaboration product.
#[derive(Clone)]
pub struct CollaborationChatProductPort {
    core: Arc<CollaborationCore>,
    history_status: Arc<Mutex<crate::room_service::RoomHistoryView>>,
    community_connected: Arc<AtomicBool>,
}

/// Read-only result of durably preparing one outgoing Chat message.
#[derive(Clone)]
pub struct PreparedCollaborationChatMessage {
    core: Arc<CollaborationCore>,
    network_id: String,
    conversation_id: String,
    envelope_sha256: String,
    envelope_bytes: Vec<u8>,
    sender_profile_did: String,
    sender_profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    body: String,
    issued_at: u64,
    expires_at: u64,
}

/// Opaque durable Chat projection. Only the originating port can acknowledge it.
pub struct CollaborationChatHandoff {
    core: Arc<CollaborationCore>,
    network_id: String,
    conversation_id: String,
    envelope_sha256: String,
    envelope_bytes: Vec<u8>,
    sender_profile_did: String,
    sender_profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    body: String,
    issued_at: u64,
    expires_at: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChatPayload {
    body: String,
}

impl CollaborationChatProductPort {
    pub(crate) fn new(core: Arc<CollaborationCore>) -> anyhow::Result<Self> {
        if core.sender_service() != CHAT_SERVICE {
            anyhow::bail!("default collaboration conversation is not owned by Chat");
        }
        Ok(Self {
            core,
            community_connected: Arc::new(AtomicBool::new(false)),
            history_status: Arc::new(Mutex::new(crate::room_service::RoomHistoryView {
                status: "searching".to_string(),
                detail: "Checking recent Community history from online participants.".to_string(),
            })),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_shares_core_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }

    #[cfg(test)]
    pub(crate) fn test_core(&self) -> &Arc<CollaborationCore> {
        &self.core
    }

    #[cfg(test)]
    pub(crate) fn test_person_profile(
        &self,
        display_name: &str,
        handle: Option<&str>,
    ) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        let (profile_key, _) = elastos_runtime::signature::generate_keypair();
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_key,
            display_name,
            handle,
            1,
            None,
            1_800_000_000,
            vec![self.core.test_local_device_did()],
        )
        .unwrap()
    }

    pub(crate) fn conversation_transport_view(&self) -> crate::room_service::RoomTransportView {
        let joined = self.community_membership().joined();
        let connected = self.community_connected.load(Ordering::Relaxed);
        crate::room_service::RoomTransportView {
            configured: true,
            available: joined && connected,
            community_joined: Some(joined),
            status: Some(if !joined {
                crate::collaboration_release_network::COMMUNITY_LEFT_DETAIL.to_string()
            } else if connected {
                "Community is connected.".to_string()
            } else {
                "Community is unreachable. Retained messages stay available.".to_string()
            }),
            history: self.history_status.lock().ok().map(|status| status.clone()),
        }
    }

    pub(crate) fn community_membership(
        &self,
    ) -> &Arc<crate::collaboration_release_network::CommunityMembership> {
        self.core.community_membership()
    }

    pub(crate) fn set_community_joined(&self, joined: bool) -> anyhow::Result<()> {
        self.core.set_community_joined(joined)?;
        self.set_community_connected(false);
        Ok(())
    }

    pub(crate) fn set_community_connected(&self, connected: bool) {
        self.community_connected.store(connected, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn test_live_unresolved_outgoing(&self) -> anyhow::Result<usize> {
        Ok(self.core.summary()?.live_unresolved_outgoing)
    }

    /// Durably prepare one fixed Chat message. The Runtime request binding is
    /// idempotency evidence only; the port supplies all product authority fields.
    pub(crate) fn prepare_message(
        &self,
        operation: EspRequestBinding,
        body: &str,
        profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<PreparedCollaborationChatMessage> {
        let body = normalize_chat_body(body)?;
        let payload = ChatPayload { body };
        let expected = chat_message_request_binding(
            &operation.request_id,
            &operation.principal,
            &payload.body,
            profile,
        )?;
        if operation != expected {
            anyhow::bail!("Chat collaboration request binding is invalid");
        }
        let prepared = self.core.prepare_profile_outgoing(
            operation,
            profile,
            CHAT_PAYLOAD_TYPE,
            serde_json::to_value(payload)?,
            now,
            CHAT_MESSAGE_TTL_SECS,
        )?;
        prepared_chat_message(self.core.clone(), &prepared)
    }

    /// Return only durable handoffs that exactly implement the current Chat payload.
    /// Core retires malformed legacy Chat handoffs and keeps their replay evidence.
    pub fn pending_messages(&self) -> anyhow::Result<Vec<CollaborationChatHandoff>> {
        Ok(self
            .core
            .pending_product_handoffs()?
            .into_iter()
            .filter_map(|pending| chat_handoff(self.core.clone(), pending))
            .collect())
    }

    pub(crate) fn pending_outgoing_messages(
        &self,
        now: u64,
    ) -> anyhow::Result<Vec<PreparedCollaborationChatMessage>> {
        Ok(self
            .core
            .pending_outgoing_product_projections(now)?
            .iter()
            .filter_map(|pending| prepared_chat_message(self.core.clone(), pending.outgoing()).ok())
            .collect())
    }

    /// Project one locally prepared message into Chat's scoped durable read model.
    pub fn project_prepared_message(
        &self,
        data_dir: &Path,
        prepared: &PreparedCollaborationChatMessage,
        local_session_token: Option<&str>,
    ) -> anyhow::Result<crate::room_service::ConversationObjectView> {
        self.require_prepared_message(prepared)?;
        let object = crate::room_service::project_collaboration_text(
            data_dir,
            (&prepared.network_id, &prepared.conversation_id),
            &prepared.envelope_sha256,
            &room_profile_card(&prepared.sender_profile),
            &prepared.body,
            prepared.issued_at,
            CollaborationTextOrigin::Outgoing {
                local_session_token,
            },
        )?;
        self.retain_projected_history(&prepared.envelope_bytes, prepared.issued_at)?;
        self.core
            .acknowledge_outgoing_product_projection(&prepared.envelope_sha256)?;
        Ok(object)
    }

    /// Durably project one incoming message before acknowledging its opaque handoff.
    pub fn project_handoff(
        &self,
        data_dir: &Path,
        handoff: &CollaborationChatHandoff,
    ) -> anyhow::Result<crate::room_service::ConversationObjectView> {
        self.require_handoff(handoff)?;
        let object = crate::room_service::project_collaboration_text(
            data_dir,
            (&handoff.network_id, &handoff.conversation_id),
            &handoff.envelope_sha256,
            &room_profile_card(&handoff.sender_profile),
            &handoff.body,
            handoff.issued_at,
            CollaborationTextOrigin::Incoming,
        )?;
        self.retain_projected_history(&handoff.envelope_bytes, handoff.issued_at)?;
        self.core
            .acknowledge_product_handoff(&handoff.envelope_sha256)?;
        Ok(object)
    }

    fn retain_projected_history(&self, envelope: &[u8], issued_at: u64) -> anyhow::Result<()> {
        let now = now_secs();
        // Room retains the verified local obligation after catch-up retention ends.
        if issued_at.saturating_add(crate::collaboration_core::CONVERSATION_HISTORY_RETENTION_SECS)
            > now
        {
            self.core.retain_history_message(envelope, now)?;
        }
        Ok(())
    }

    pub(crate) fn history_scope(&self) -> (&str, &str) {
        self.core.conversation_scope()
    }

    pub(crate) fn local_device_did(&self) -> String {
        self.core.local_device_did()
    }

    pub(crate) fn retained_history(&self, now: u64) -> anyhow::Result<Vec<Vec<u8>>> {
        let messages = self.core.conversation_history(now)?;
        for bytes in &messages {
            self.history_handoff(bytes, now)?;
        }
        Ok(messages)
    }

    /// Verify the entire bounded batch before any product write. Projection and
    /// retention both use the original message hash, so interrupted/repeated
    /// catch-up resumes through the existing idempotent room projection.
    pub(crate) fn project_history(
        &self,
        data_dir: &Path,
        messages: &[Vec<u8>],
        now: u64,
    ) -> anyhow::Result<()> {
        let handoffs = messages
            .iter()
            .map(|bytes| self.history_handoff(bytes, now))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let candidates = self.core.history_projection_candidates(messages, now)?;
        let hashes = candidates
            .iter()
            .map(|bytes| {
                elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(bytes)
            })
            .collect::<std::collections::HashSet<_>>();
        for handoff in handoffs
            .into_iter()
            .filter(|handoff| hashes.contains(&handoff.envelope_sha256))
        {
            crate::room_service::project_collaboration_text(
                data_dir,
                (&handoff.network_id, &handoff.conversation_id),
                &handoff.envelope_sha256,
                &room_profile_card(&handoff.sender_profile),
                &handoff.body,
                handoff.issued_at,
                CollaborationTextOrigin::Incoming,
            )?;
        }
        self.core.retain_history_messages(&candidates, now)
    }

    fn history_handoff(&self, bytes: &[u8], now: u64) -> anyhow::Result<CollaborationChatHandoff> {
        let authorized = self.core.authorize_history_message(bytes, now)?;
        let message = &authorized.message().envelope().payload;
        validate_chat_projection(&authorized)?;
        let payload = exact_chat_payload(authorized.product_payload())?;
        let profile = authorized.sender_profile().clone();
        Ok(CollaborationChatHandoff {
            core: self.core.clone(),
            network_id: message.network_id.clone(),
            conversation_id: message.conversation_id.clone(),
            envelope_sha256: authorized.message().envelope_sha256().to_string(),
            envelope_bytes: bytes.to_vec(),
            sender_profile_did: profile.document().profile_did.clone(),
            sender_profile: profile,
            body: payload.body,
            issued_at: message.created_at,
            expires_at: message.expires_at,
        })
    }

    pub(crate) fn set_history_searching(&self) {
        if let Ok(mut status) = self.history_status.lock() {
            status.status = "searching".to_string();
            status.detail =
                "Checking recent Community history from online participants.".to_string();
        }
    }

    pub(crate) fn set_history_status(&self, available: bool) {
        if let Ok(mut status) = self.history_status.lock() {
            status.status = if available {
                "available"
            } else {
                "unavailable"
            }
            .to_string();
            status.detail = if available {
                "Recent Community history is available from an online participant."
            } else {
                "Recent Community history is unavailable. Another participant must be online to catch up."
            }.to_string();
        }
    }

    /// Read only this port's verified collaboration namespace from Chat's store.
    pub fn conversation_poll(
        &self,
        data_dir: &Path,
        local_session_token: &str,
        since: u64,
    ) -> anyhow::Result<crate::room_service::RoomPollView> {
        let (network_id, conversation_id) = self.core.conversation_scope();
        crate::room_service::collaboration_room_poll(
            data_dir,
            local_session_token,
            network_id,
            conversation_id,
            since,
        )
    }

    pub(crate) fn community_unread(
        &self,
        data_dir: &Path,
        reader: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<bool> {
        self.sync_community_read(data_dir, reader, None, now)
    }

    /// A visible read acknowledges through the returned verified scoped rows,
    /// including own rows after an older unread remote row has been evicted.
    /// The caller's cursor and latest_seq are not read evidence.
    pub(crate) fn conversation_poll_with_read_intent(
        &self,
        data_dir: &Path,
        token: &str,
        since: u64,
        reader: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        mark_read: bool,
        now: u64,
    ) -> anyhow::Result<crate::room_service::RoomPollView> {
        let delivery = self
            .core
            .outgoing_chat_delivery(&reader.document().profile_did)?;
        let poll = crate::room_service::collaboration_room_poll_with_delivery(
            data_dir,
            token,
            self.core.conversation_scope(),
            since,
            &reader.document().profile_did,
            &delivery,
            now,
        )?;
        if mark_read {
            let read_through = poll
                .objects
                .iter()
                .filter(|object| object.sender_profile_verified == Some(true))
                .map(|object| object.seq)
                .max();
            self.sync_community_read(data_dir, reader, read_through, now)?;
        }
        Ok(poll)
    }

    fn sync_community_read(
        &self,
        data_dir: &Path,
        reader: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        read_through: Option<u64>,
        now: u64,
    ) -> anyhow::Result<bool> {
        let (network_id, conversation_id) = self.core.conversation_scope();
        let reader_did = reader.document().profile_did.as_str();
        let seq = crate::room_service::collaboration_latest_remote_message_seq(
            data_dir,
            network_id,
            conversation_id,
            reader_did,
        )?;
        crate::notifications::sync_community_message_notification(
            data_dir,
            reader_did,
            (network_id, conversation_id),
            seq,
            read_through,
            now,
        )
    }

    fn require_prepared_message(
        &self,
        prepared: &PreparedCollaborationChatMessage,
    ) -> anyhow::Result<()> {
        if !Arc::ptr_eq(&self.core, &prepared.core) {
            anyhow::bail!("prepared Chat collaboration message belongs to another product port");
        }
        let (network_id, conversation_id) = self.core.conversation_scope();
        if prepared.network_id != network_id || prepared.conversation_id != conversation_id {
            anyhow::bail!("prepared Chat collaboration message scope is invalid");
        }
        Ok(())
    }

    fn require_handoff(&self, handoff: &CollaborationChatHandoff) -> anyhow::Result<()> {
        if !Arc::ptr_eq(&self.core, &handoff.core) {
            anyhow::bail!("Chat collaboration handoff belongs to another product port");
        }
        let (network_id, conversation_id) = self.core.conversation_scope();
        if handoff.network_id != network_id || handoff.conversation_id != conversation_id {
            anyhow::bail!("Chat collaboration handoff scope is invalid");
        }
        Ok(())
    }
}

/// Construct the sole canonical Runtime idempotency binding accepted by the
/// current Chat collaboration product.
pub(crate) fn chat_message_request_binding(
    request_id: &str,
    principal: &str,
    body: &str,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) -> anyhow::Result<EspRequestBinding> {
    let body = normalize_chat_body(body)?;
    let payload =
        crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
            profile,
            serde_json::json!({ "body": body }),
        )?;
    let intent = serde_json::json!({
        "payload_type": CHAT_PAYLOAD_TYPE,
        "payload": payload,
        "ttl_secs": CHAT_MESSAGE_TTL_SECS,
    });
    Ok(esp_request_binding(
        request_id,
        principal,
        CHAT_ROOM_CAPSULE,
        Some(CHAT_INTERFACE),
        DEFAULT_CONVERSATION_SEND_METHOD,
        [CHAT_RESOURCE.to_string()],
        &intent,
    ))
}

#[cfg(test)]
pub(crate) fn test_chat_product_port(
    data_root: &Path,
    network_id: &str,
    conversation_id: &str,
) -> CollaborationChatProductPort {
    use sha2::{Digest as _, Sha256};

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

    let grant_bytes = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
        schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
        network_id: network_id.to_string(),
        conversation_id: conversation_id.to_string(),
        sender_service: CHAT_SERVICE.to_string(),
        admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
    })
    .unwrap();
    let grant_digest = Sha256::digest(&grant_bytes);
    let grant_cid = cid::Cid::new_v1(
        0x55,
        cid::multihash::Multihash::<64>::wrap(0x12, grant_digest.as_slice()).unwrap(),
    )
    .to_string();
    let (profile_signer, _) = elastos_runtime::signature::generate_keypair();
    let signer_did = crate::crypto::encode_signing_key_did(&profile_signer);
    let profile_payload = CollaborationNetworkProfile {
        schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
        network_id: network_id.to_string(),
        revision: 1,
        previous_profile_sha256: None,
        signer_did: signer_did.clone(),
        bootstrap_peers: Vec::new(),
        default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
    };
    let payload_bytes =
        canonical_collaboration_network_profile_payload_bytes(&profile_payload).unwrap();
    let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
        &profile_signer,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
        &payload_bytes,
    );
    let profile_bytes = serde_json::to_vec(
        &serde_json::to_value(SignedCollaborationNetworkProfile {
            payload: profile_payload,
            signature,
            signer_did: envelope_signer,
        })
        .unwrap(),
    )
    .unwrap();
    let CollaborationNetworkProfileMode::Configured(profile) =
        validate_collaboration_network_profile(
            Some(&profile_bytes),
            network_id,
            &[signer_did],
            None,
        )
        .unwrap()
    else {
        panic!("test collaboration profile must be configured");
    };
    let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
    let (device_key, _) = elastos_identity::load_or_create_did(data_root).unwrap();
    let core = Arc::new(
        CollaborationCore::new(data_root, device_key, profile, grant, CHAT_ROOM_CAPSULE).unwrap(),
    );
    CollaborationChatProductPort::new(core).unwrap()
}

impl PreparedCollaborationChatMessage {
    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }

    pub fn sender_profile_did(&self) -> &str {
        &self.sender_profile_did
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl std::fmt::Debug for PreparedCollaborationChatMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedCollaborationChatMessage")
            .field("envelope_sha256", &self.envelope_sha256)
            .field("sender_profile_did", &self.sender_profile_did)
            .field("body", &self.body)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl PartialEq for PreparedCollaborationChatMessage {
    fn eq(&self, other: &Self) -> bool {
        self.network_id == other.network_id
            && self.conversation_id == other.conversation_id
            && self.envelope_sha256 == other.envelope_sha256
            && self.sender_profile_did == other.sender_profile_did
            && self.body == other.body
            && self.issued_at == other.issued_at
            && self.expires_at == other.expires_at
    }
}

impl Eq for PreparedCollaborationChatMessage {}

impl CollaborationChatHandoff {
    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }

    pub fn sender_profile_did(&self) -> &str {
        &self.sender_profile_did
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

fn prepared_chat_message(
    core: Arc<CollaborationCore>,
    prepared: &DurableOutgoingMessage,
) -> anyhow::Result<PreparedCollaborationChatMessage> {
    let authorized = core.authorize_stored_product_message(prepared.envelope_bytes())?;
    let message = &authorized.message().envelope().payload;
    validate_chat_projection(&authorized)?;
    let payload = exact_chat_payload(authorized.product_payload())?;
    let sender_profile = authorized.sender_profile().clone();
    let sender_profile_did = sender_profile.document().profile_did.clone();
    Ok(PreparedCollaborationChatMessage {
        core,
        network_id: message.network_id.clone(),
        conversation_id: message.conversation_id.clone(),
        envelope_sha256: prepared.envelope_sha256().to_string(),
        envelope_bytes: prepared.envelope_bytes().to_vec(),
        sender_profile_did,
        sender_profile,
        body: payload.body,
        issued_at: message.created_at,
        expires_at: message.expires_at,
    })
}

fn chat_handoff(
    core: Arc<CollaborationCore>,
    pending: PendingProductHandoff,
) -> Option<CollaborationChatHandoff> {
    let message = pending.authorized_message().message();
    let payload = &message.envelope().payload;
    validate_chat_projection(pending.authorized_message()).ok()?;
    let chat = exact_chat_payload(pending.authorized_message().product_payload()).ok()?;
    let sender_profile = pending.authorized_message().sender_profile().clone();
    let sender_profile_did = sender_profile.document().profile_did.clone();
    Some(CollaborationChatHandoff {
        core,
        network_id: payload.network_id.clone(),
        conversation_id: payload.conversation_id.clone(),
        envelope_sha256: message.envelope_sha256().to_string(),
        envelope_bytes:
            elastos_common::collaboration_protocol::canonical_signed_collaboration_message_bytes(
                message.envelope(),
            )
            .ok()?,
        sender_profile_did,
        sender_profile,
        body: chat.body,
        issued_at: payload.created_at,
        expires_at: payload.expires_at,
    })
}

fn room_profile_card(
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) -> crate::room_service::RoomProfileCardView {
    crate::room_service::RoomProfileCardView {
        schema: "elastos.profile-card/v1".to_string(),
        profile_id: profile.document().profile_did.clone(),
        display_name: profile.document().display_name.clone(),
        handle: profile.document().handle.clone(),
        updated_at: profile.document().updated_at,
    }
}

pub(crate) fn validate_chat_projection(
    authorized: &crate::collaboration_default_conversation::AuthorizedDefaultConversationMessage,
) -> anyhow::Result<()> {
    let message = authorized.message();
    let payload = &message.envelope().payload;
    if payload.payload_type != CHAT_PAYLOAD_TYPE {
        anyhow::bail!("collaboration payload is not the current Chat product");
    }
    let chat = exact_chat_payload(authorized.product_payload())?;
    crate::room_service::validate_collaboration_text(
        (&payload.network_id, &payload.conversation_id),
        message.envelope_sha256(),
        &room_profile_card(authorized.sender_profile()),
        &chat.body,
    )
}

fn exact_chat_payload(payload: &serde_json::Value) -> anyhow::Result<ChatPayload> {
    let payload: ChatPayload = serde_json::from_value(payload.clone())
        .context("collaboration Chat payload is malformed")?;
    let normalized = normalize_chat_body(&payload.body)?;
    if normalized != payload.body {
        anyhow::bail!("collaboration Chat body is not canonical");
    }
    Ok(payload)
}

fn normalize_chat_body(body: &str) -> anyhow::Result<String> {
    let body = body.trim();
    if body.is_empty() {
        anyhow::bail!("Chat message must not be empty");
    }
    if body.chars().count() > crate::room_service::MAX_OBJECT_BODY_LEN {
        anyhow::bail!(
            "Chat message exceeds {} characters",
            crate::room_service::MAX_OBJECT_BODY_LEN
        );
    }
    Ok(body.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    use elastos_runtime::signature::{generate_keypair, SigningKey};
    use sha2::{Digest, Sha256};

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

    const NETWORK: &str = "collaboration-product-test";
    const CONVERSATION: &str = "default-conversation";
    const NOW: u64 = 1_800_000_000;

    struct Fixture {
        _temp: Option<tempfile::TempDir>,
        data_root: std::path::PathBuf,
        device_key: SigningKey,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
        person_profile:
            crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        core: Arc<CollaborationCore>,
        port: CollaborationChatProductPort,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut fixture = fixture_at(temp.path(), NETWORK, CONVERSATION);
        fixture._temp = Some(temp);
        fixture
    }

    fn fixture_at(data_root: &Path, network_id: &str, conversation_id: &str) -> Fixture {
        let grant_bytes = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: network_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sender_service: CHAT_SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        })
        .unwrap();
        let digest = Sha256::digest(&grant_bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        let grant_cid = cid::Cid::new_v1(0x55, multihash).to_string();
        let (profile_signer, _) = generate_keypair();
        let signer_did = crate::crypto::encode_signing_key_did(&profile_signer);
        let profile_payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&profile_payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            &profile_signer,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let profile_bytes = serde_json::to_vec(
            &serde_json::to_value(SignedCollaborationNetworkProfile {
                payload: profile_payload,
                signature,
                signer_did: envelope_signer,
            })
            .unwrap(),
        )
        .unwrap();
        let CollaborationNetworkProfileMode::Configured(profile) =
            validate_collaboration_network_profile(
                Some(&profile_bytes),
                network_id,
                &[signer_did],
                None,
            )
            .unwrap()
        else {
            panic!("expected configured profile");
        };
        let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
        let (device_key, _) = generate_keypair();
        let device_did = crate::crypto::encode_signing_key_did(&device_key);
        let (person_key, _) = generate_keypair();
        let person_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &person_key,
                "Local person",
                Some("local"),
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
        let port = CollaborationChatProductPort::new(core.clone()).unwrap();
        Fixture {
            data_root: data_root.to_path_buf(),
            _temp: None,
            device_key,
            profile,
            grant,
            person_profile,
            core,
            port,
        }
    }

    fn binding(fixture: &Fixture, request_id: &str, body: &str) -> EspRequestBinding {
        chat_message_request_binding(
            request_id,
            "runtime-principal",
            body,
            &fixture.person_profile,
        )
        .unwrap()
    }

    #[test]
    fn community_availability_tracks_current_peers_and_left_takes_priority() {
        let fixture = fixture();
        let other_port = fixture.port.clone();
        assert!(!fixture.port.conversation_transport_view().available);
        assert_eq!(
            fixture.port.conversation_transport_view().community_joined,
            Some(true)
        );
        fixture.port.set_community_connected(true);
        assert!(other_port.conversation_transport_view().available);
        fixture.port.set_community_connected(false);
        assert!(!other_port.conversation_transport_view().available);
        fixture.port.set_community_connected(true);
        fixture.port.set_community_joined(false).unwrap();
        fixture.port.set_community_connected(true);
        let left = other_port.conversation_transport_view();
        assert!(!left.available);
        assert_eq!(left.community_joined, Some(false));
        assert_eq!(
            left.status.as_deref(),
            Some(crate::collaboration_release_network::COMMUNITY_LEFT_DETAIL)
        );
        fixture.port.set_community_joined(true).unwrap();
        assert!(!other_port.conversation_transport_view().available);
        fixture.port.set_community_connected(true);
        assert!(other_port.conversation_transport_view().available);
    }

    #[test]
    fn leaving_community_keeps_original_history_and_refuses_new_shared_intent() {
        let fixture = fixture();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Reader",
            "Community membership test",
        )
        .unwrap();
        let now = now_secs();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "before-leave", "retained history"),
                "retained history",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
            .unwrap();
        let original_history = fixture.core.conversation_history(now).unwrap();
        let original_core = fixture.core.summary().unwrap();
        fixture
            .port
            .community_membership()
            .set_joined(false)
            .unwrap();
        assert!(fixture
            .port
            .prepare_message(
                binding(&fixture, "after-leave", "refused"),
                "refused",
                &fixture.person_profile,
                now,
            )
            .is_err());
        assert_eq!(fixture.core.summary().unwrap(), original_core);
        assert_eq!(
            fixture.core.conversation_history(now).unwrap(),
            original_history
        );
        let view = fixture.port.conversation_transport_view();
        assert_eq!(view.community_joined, Some(false));
        assert!(!view.available);
        let poll = fixture
            .port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(poll.objects.len(), 1);
        assert_eq!(poll.objects[0].body.as_deref(), Some("retained history"));
        fixture
            .port
            .community_membership()
            .set_joined(true)
            .unwrap();
        assert_eq!(
            fixture.port.conversation_transport_view().community_joined,
            Some(true)
        );
        assert_eq!(
            fixture.core.conversation_history(now).unwrap(),
            original_history
        );
        fixture
            .port
            .prepare_message(
                binding(&fixture, "after-rejoin", "joined again"),
                "joined again",
                &fixture.person_profile,
                now + 1,
            )
            .unwrap();
    }

    fn remote_authority(
        fixture: &Fixture,
    ) -> (
        DefaultConversationDeviceAuthority,
        crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    ) {
        let (remote_key, _) = generate_keypair();
        let remote_did = crate::crypto::encode_signing_key_did(&remote_key);
        let (person_key, _) = generate_keypair();
        let person_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &person_key,
                "Remote person",
                Some("remote"),
                1,
                None,
                NOW,
                vec![remote_did],
            )
            .unwrap();
        let authority = DefaultConversationDeviceAuthority::new(
            remote_key,
            fixture.profile.clone(),
            fixture.grant.clone(),
        )
        .unwrap();
        (authority, person_profile)
    }

    fn delivery_poll(
        fixture: &Fixture,
        token: &str,
        since: u64,
        now: u64,
    ) -> crate::room_service::RoomPollView {
        fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                token,
                since,
                &fixture.person_profile,
                false,
                now,
            )
            .unwrap()
    }

    #[test]
    fn community_delivery_receipt_refreshes_own_row_without_settling_other_peers() {
        use crate::room_service::CommunityDeliveryState::{Accepted, Sending};
        let fixture = fixture();
        let now = now_secs();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Local",
            "Delivery test",
        )
        .unwrap();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "delivery-own", "own original"),
                "own original",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        let own = fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
            .unwrap();
        assert_eq!(own.delivery_state, Some(Sending));
        let pending = delivery_poll(&fixture, &session.token, own.seq, now);
        assert!(pending.objects.is_empty());
        assert_eq!(pending.latest_seq, own.seq);
        assert_eq!(pending.own_delivery[0].state, Sending);

        let (remote, remote_profile) = remote_authority(&fixture);
        let accepted = remote
            .authorize_incoming(
                &prepared.envelope_bytes,
                &fixture.core.local_device_did(),
                now + 1,
            )
            .unwrap();
        fixture
            .core
            .record_home_chat_acceptance(
                &remote
                    .prepare_acceptance_receipt(&accepted, now + 1)
                    .unwrap(),
                &prepared.envelope_sha256,
                &remote.local_device_did(),
                now + 1,
            )
            .unwrap();
        let refreshed = delivery_poll(&fixture, &session.token, own.seq, now + 1);
        assert!(refreshed.objects.is_empty());
        assert_eq!(refreshed.latest_seq, own.seq);
        assert_eq!(refreshed.own_delivery[0].state, Accepted);
        let retries = fixture.core.pending_outgoing(now + 1).unwrap();
        assert_eq!(retries.len(), 1);
        assert!(retries[0].accepted_by(&remote.local_device_did()));
        assert!(!retries[0].accepted_by(&remote_authority(&fixture).0.local_device_did()));

        let incoming = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"received original"}),
                now + 2,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        fixture
            .core
            .accept_incoming_from_signed_source_for_test(incoming.envelope_bytes(), now + 2)
            .unwrap();
        fixture
            .port
            .project_handoff(
                &fixture.data_root,
                &fixture.port.pending_messages().unwrap().remove(0),
            )
            .unwrap();
        let view = delivery_poll(&fixture, &session.token, 0, now + 2);
        assert_eq!(view.own_delivery.len(), 1);
        assert_eq!(view.objects[1].delivery_state, None);
        assert_eq!(view.objects[0].body.as_deref(), Some("own original"));
        let private_projection = serde_json::to_value(&view.own_delivery).unwrap();
        assert_eq!(
            private_projection,
            serde_json::json!([{ "seq": own.seq, "state": "accepted" }])
        );
        let later = prepared.expires_at
            + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
            + 1;
        fixture
            .port
            .prepare_message(
                binding(&fixture, "after-accepted", "new intent"),
                "new intent",
                &fixture.person_profile,
                later,
            )
            .unwrap();
        let retained = fixture
            .port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(retained.objects[0].delivery_state, Some(Accepted));
    }

    #[test]
    fn community_delivery_terminal_outcomes_survive_closed_chat_and_restart() {
        use crate::room_service::CommunityDeliveryState::{Accepted, Expired, Sending};
        for accepted in [false, true] {
            let fixture = fixture();
            let now = now_secs();
            let session = crate::room_service::start_local_runtime_session(
                &fixture.data_root,
                &fixture.person_profile.document().profile_did,
                "Local",
                "Closed Chat test",
            )
            .unwrap();
            let prepared = fixture
                .port
                .prepare_message(
                    binding(&fixture, "closed-chat-own", "retained original"),
                    "retained original",
                    &fixture.person_profile,
                    now,
                )
                .unwrap();
            let own = fixture
                .port
                .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
                .unwrap();
            assert_eq!(own.delivery_state, Some(Sending));
            if accepted {
                let (remote, _) = remote_authority(&fixture);
                let incoming = remote
                    .authorize_incoming(
                        &prepared.envelope_bytes,
                        &fixture.core.local_device_did(),
                        now + 1,
                    )
                    .unwrap();
                fixture
                    .core
                    .record_home_chat_acceptance(
                        &remote
                            .prepare_acceptance_receipt(&incoming, now + 1)
                            .unwrap(),
                        &prepared.envelope_sha256,
                        &remote.local_device_did(),
                        now + 1,
                    )
                    .unwrap();
            }
            // Chat stays closed throughout receipt/expiry and Core pruning.
            let later = prepared.expires_at
                + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
                + 1;
            let objects = elastos_common::localhost::rooted_localhost_fs_path(
                &fixture.data_root,
                "localhost://Local/Shared/AppCapsules/chat-room",
            )
            .unwrap()
            .join("room/objects.json");
            let saved_objects = std::fs::read(&objects).unwrap();
            std::fs::write(&objects, b"[").unwrap();
            let failed = fixture.port.prepare_message(
                binding(&fixture, "after-closed-chat", "next intent"),
                "next intent",
                &fixture.person_profile,
                later,
            );
            std::fs::write(&objects, saved_objects).unwrap();
            assert!(failed.is_err());
            let retained = fixture
                .core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap();
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].accepted, accepted);
            assert_eq!(retained[0].envelope_sha256, prepared.envelope_sha256);
            let blocked_temp = objects.with_file_name(".objects.json.tmp");
            std::fs::create_dir(&blocked_temp).unwrap();
            let failed = fixture.port.prepare_message(
                binding(&fixture, "after-closed-chat", "next intent"),
                "next intent",
                &fixture.person_profile,
                later,
            );
            std::fs::remove_dir(&blocked_temp).unwrap();
            assert!(failed.is_err());
            assert_eq!(
                fixture
                    .core
                    .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                    .unwrap()[0]
                    .envelope_sha256,
                prepared.envelope_sha256
            );
            fixture
                .port
                .prepare_message(
                    binding(&fixture, "after-closed-chat", "next intent"),
                    "next intent",
                    &fixture.person_profile,
                    later,
                )
                .unwrap();
            assert!(fixture
                .core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap()
                .is_empty());
            let restarted = CollaborationChatProductPort::new(Arc::new(
                CollaborationCore::new(
                    &fixture.data_root,
                    fixture.device_key.clone(),
                    fixture.profile.clone(),
                    fixture.grant.clone(),
                    CHAT_ROOM_CAPSULE,
                )
                .unwrap(),
            ))
            .unwrap();
            let reopened = restarted
                .conversation_poll_with_read_intent(
                    &fixture.data_root,
                    &session.token,
                    own.seq,
                    &fixture.person_profile,
                    false,
                    later,
                )
                .unwrap();
            assert!(reopened.objects.is_empty());
            assert_eq!(reopened.latest_seq, own.seq);
            assert_eq!(reopened.own_delivery.len(), 1);
            assert_eq!(reopened.own_delivery[0].seq, own.seq);
            assert_eq!(
                reopened.own_delivery[0].state,
                if accepted { Accepted } else { Expired }
            );
        }
    }

    #[test]
    fn community_delivery_recovered_outgoing_survives_closed_chat_pruning_and_restart() {
        use crate::room_service::CommunityDeliveryState::{Accepted, Expired};
        for (accepted, legacy_empty_state) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let fixture = fixture();
            let now = now_secs();
            let prepared = fixture
                .port
                .prepare_message(
                    binding(&fixture, "recover-own", "recovered original"),
                    "recovered original",
                    &fixture.person_profile,
                    now,
                )
                .unwrap();
            // Runtime stops after durable prepare, before the Chat row exists.
            let recovered_core = Arc::new(
                CollaborationCore::new(
                    &fixture.data_root,
                    fixture.device_key.clone(),
                    fixture.profile.clone(),
                    fixture.grant.clone(),
                    CHAT_ROOM_CAPSULE,
                )
                .unwrap(),
            );
            let recovered_port = CollaborationChatProductPort::new(recovered_core.clone()).unwrap();
            let pending = recovered_port.pending_outgoing_messages(now + 1).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].envelope_sha256, prepared.envelope_sha256);
            // The worker has no local Chat session when it projects recovery.
            let own = recovered_port
                .project_prepared_message(&fixture.data_root, &pending[0], None)
                .unwrap();
            let objects_path = elastos_common::localhost::rooted_localhost_fs_path(
                &fixture.data_root,
                crate::room_service::room_root_uri(),
            )
            .unwrap()
            .join("room/objects.json");
            let read_objects = || -> Vec<serde_json::Value> {
                serde_json::from_slice(&std::fs::read(&objects_path).unwrap()).unwrap()
            };
            assert_eq!(read_objects()[0]["delivery_state"], "sending");
            assert!(recovered_port
                .pending_outgoing_messages(now + 1)
                .unwrap()
                .is_empty());
            if legacy_empty_state {
                // Older workers saved this same verified outgoing row without
                // delivery metadata. Core must repair it before losing evidence.
                let mut objects = read_objects();
                objects[0].as_object_mut().unwrap().remove("delivery_state");
                std::fs::write(&objects_path, serde_json::to_vec_pretty(&objects).unwrap())
                    .unwrap();
            }

            let (remote, remote_profile) = remote_authority(&fixture);
            let incoming = remote
                .prepare_profile_outgoing(
                    &remote_profile,
                    CHAT_SERVICE,
                    CHAT_PAYLOAD_TYPE,
                    serde_json::json!({"body":"incoming original"}),
                    now + 2,
                    CHAT_MESSAGE_TTL_SECS,
                )
                .unwrap();
            recovered_core
                .accept_incoming_from_signed_source_for_test(incoming.envelope_bytes(), now + 2)
                .unwrap();
            let handoff = recovered_port.pending_messages().unwrap().remove(0);
            let received = recovered_port
                .project_handoff(&fixture.data_root, &handoff)
                .unwrap();
            if accepted {
                let incoming = remote
                    .authorize_incoming(
                        &prepared.envelope_bytes,
                        &recovered_core.local_device_did(),
                        now + 1,
                    )
                    .unwrap();
                recovered_core
                    .record_home_chat_acceptance(
                        &remote
                            .prepare_acceptance_receipt(&incoming, now + 1)
                            .unwrap(),
                        &prepared.envelope_sha256,
                        &remote.local_device_did(),
                        prepared.expires_at,
                    )
                    .unwrap();
            }
            let evidence = recovered_core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap();
            assert_eq!(evidence.len(), 1);
            assert_eq!(evidence[0].accepted, accepted);
            assert_eq!(evidence[0].envelope_sha256, prepared.envelope_sha256);
            assert_eq!(
                read_objects()[0]["delivery_state"].is_null(),
                legacy_empty_state
            );
            let later = prepared.expires_at
                + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
                + 1;
            // Chat remains closed throughout receipt/expiry and Core pruning.
            recovered_port
                .prepare_message(
                    binding(&fixture, "after-recover", "next intent"),
                    "next intent",
                    &fixture.person_profile,
                    later,
                )
                .unwrap();
            assert!(recovered_core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap()
                .is_empty());
            let restarted = CollaborationChatProductPort::new(Arc::new(
                CollaborationCore::new(
                    &fixture.data_root,
                    fixture.device_key.clone(),
                    fixture.profile.clone(),
                    fixture.grant.clone(),
                    CHAT_ROOM_CAPSULE,
                )
                .unwrap(),
            ))
            .unwrap();
            let session = crate::room_service::start_local_runtime_session(
                &fixture.data_root,
                &fixture.person_profile.document().profile_did,
                "Local",
                "Recovered Chat test",
            )
            .unwrap();
            let reopened = restarted
                .conversation_poll_with_read_intent(
                    &fixture.data_root,
                    &session.token,
                    0,
                    &fixture.person_profile,
                    false,
                    later,
                )
                .unwrap();
            assert_eq!(reopened.own_delivery.len(), 1);
            assert_eq!(reopened.own_delivery[0].seq, own.seq);
            assert_eq!(
                reopened.own_delivery[0].state,
                if accepted { Accepted } else { Expired }
            );
            assert_eq!(reopened.objects.len(), 2);
            assert_eq!(reopened.objects[0].seq, own.seq);
            assert_eq!(
                reopened.objects[0].body.as_deref(),
                Some("recovered original")
            );
            assert_eq!(reopened.objects[1].seq, received.seq);
            assert_eq!(reopened.objects[1].delivery_state, None);
            assert!(read_objects()[1]["delivery_state"].is_null());
        }
    }

    #[test]
    fn community_delivery_delayed_restart_preserves_unprojected_terminal_message() {
        use crate::room_service::CommunityDeliveryState::Expired;
        let skew = elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS;
        for age in [
            CHAT_MESSAGE_TTL_SECS + skew + 1,
            crate::collaboration_core::CONVERSATION_HISTORY_RETENTION_SECS + skew + 1,
        ] {
            let fixture = fixture();
            let now = now_secs();
            let prepared = fixture
                .port
                .prepare_message(
                    binding(&fixture, "delayed-own", "delayed original"),
                    "delayed original",
                    &fixture.person_profile,
                    now - age,
                )
                .unwrap();
            let (remote, remote_profile) = remote_authority(&fixture);
            let incoming = remote
                .prepare_profile_outgoing(
                    &remote_profile,
                    CHAT_SERVICE,
                    CHAT_PAYLOAD_TYPE,
                    serde_json::json!({"body":"delayed incoming"}),
                    now - age,
                    CHAT_MESSAGE_TTL_SECS,
                )
                .unwrap();
            fixture
                .core
                .accept_incoming_from_signed_source_for_test(
                    incoming.envelope_bytes(),
                    now - age + 1,
                )
                .unwrap();
            // Runtime stopped before local projection and restarts after wire expiry.
            let reopen = || {
                CollaborationChatProductPort::new(Arc::new(
                    CollaborationCore::new(
                        &fixture.data_root,
                        fixture.device_key.clone(),
                        fixture.profile.clone(),
                        fixture.grant.clone(),
                        CHAT_ROOM_CAPSULE,
                    )
                    .unwrap(),
                ))
                .unwrap()
            };
            let recovered = reopen();
            let later = now + 1;
            // A new send attempts pruning before the recovery worker projects.
            recovered
                .prepare_message(
                    binding(&fixture, "delayed-next", "next intent"),
                    "next intent",
                    &fixture.person_profile,
                    later,
                )
                .unwrap();
            let pending = recovered.pending_outgoing_messages(later).unwrap();
            let original = pending
                .iter()
                .find(|message| message.envelope_sha256 == prepared.envelope_sha256)
                .unwrap();
            assert_eq!(original.envelope_bytes, prepared.envelope_bytes);
            assert!(recovered
                .core
                .pending_outgoing(later)
                .unwrap()
                .iter()
                .all(|message| message.envelope_sha256() != prepared.envelope_sha256));
            let own = recovered
                .project_prepared_message(&fixture.data_root, original, None)
                .unwrap();
            let handoffs = recovered.pending_messages().unwrap();
            assert_eq!(handoffs.len(), 1);
            let received = recovered
                .project_handoff(&fixture.data_root, &handoffs[0])
                .unwrap();
            assert!(recovered.pending_messages().unwrap().is_empty());
            recovered
                .prepare_message(
                    binding(&fixture, "delayed-after-projection", "after projection"),
                    "after projection",
                    &fixture.person_profile,
                    later + 1,
                )
                .unwrap();
            assert!(recovered
                .core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap()
                .iter()
                .all(|message| message.envelope_sha256 != prepared.envelope_sha256));
            // Chat stayed closed during recovery and pruning; a second restart reads it.
            let restarted = reopen();
            let session = crate::room_service::start_local_runtime_session(
                &fixture.data_root,
                &fixture.person_profile.document().profile_did,
                "Local",
                "Delayed recovery test",
            )
            .unwrap();
            let poll = restarted
                .conversation_poll_with_read_intent(
                    &fixture.data_root,
                    &session.token,
                    0,
                    &fixture.person_profile,
                    false,
                    later + 1,
                )
                .unwrap();
            assert_eq!(poll.objects.len(), 2);
            assert_eq!(poll.objects[0].seq, own.seq);
            assert_eq!(poll.objects[0].body.as_deref(), Some("delayed original"));
            assert_eq!(poll.objects[1].seq, received.seq);
            assert_eq!(poll.objects[1].body.as_deref(), Some("delayed incoming"));
            assert_eq!(poll.objects[1].delivery_state, None);
            assert_eq!(poll.own_delivery.len(), 1);
            assert_eq!(poll.own_delivery[0].seq, own.seq);
            assert_eq!(poll.own_delivery[0].state, Expired);
        }
    }

    #[test]
    fn community_delivery_stale_poll_preserves_terminal_outcomes_after_core_pruning() {
        use crate::room_service::CommunityDeliveryState::{Accepted, Expired, Sending};
        for observe_expiry in [false, true] {
            let fixture = fixture();
            let now = now_secs();
            let session = crate::room_service::start_local_runtime_session(
                &fixture.data_root,
                &fixture.person_profile.document().profile_did,
                "Local",
                "Stale delivery test",
            )
            .unwrap();
            let prepared = fixture
                .port
                .prepare_message(
                    binding(&fixture, "stale-poll-own", "retained original"),
                    "retained original",
                    &fixture.person_profile,
                    now,
                )
                .unwrap();
            let own = fixture
                .port
                .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
                .unwrap();
            assert_eq!(own.delivery_state, Some(Sending));
            // Pause the poll after its real Core snapshot, before the Room lock.
            let stale_delivery = fixture
                .core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap();
            assert_eq!(stale_delivery.len(), 1);
            assert!(!stale_delivery[0].accepted);
            let resume_stale_poll = |at| {
                crate::room_service::collaboration_room_poll_with_delivery(
                    &fixture.data_root,
                    &session.token,
                    fixture.core.conversation_scope(),
                    own.seq,
                    &fixture.person_profile.document().profile_did,
                    &stale_delivery,
                    at,
                )
                .unwrap()
            };
            if observe_expiry {
                let expired = delivery_poll(&fixture, &session.token, own.seq, prepared.expires_at);
                assert_eq!(expired.own_delivery[0].state, Expired);
                assert_eq!(resume_stale_poll(now).own_delivery[0].state, Expired);
            }

            let (remote, _) = remote_authority(&fixture);
            let incoming = remote
                .authorize_incoming(
                    &prepared.envelope_bytes,
                    &fixture.core.local_device_did(),
                    now + 1,
                )
                .unwrap();
            fixture
                .core
                .record_home_chat_acceptance(
                    &remote
                        .prepare_acceptance_receipt(&incoming, now + 1)
                        .unwrap(),
                    &prepared.envelope_sha256,
                    &remote.local_device_did(),
                    prepared.expires_at,
                )
                .unwrap();
            if observe_expiry {
                // A verified delayed receipt promotes the saved Expired row.
                let accepted =
                    delivery_poll(&fixture, &session.token, own.seq, prepared.expires_at);
                assert_eq!(accepted.own_delivery[0].state, Accepted);
            }
            let later = prepared.expires_at
                + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
                + 1;
            fixture
                .port
                .prepare_message(
                    binding(&fixture, "after-stale-poll", "next intent"),
                    "next intent",
                    &fixture.person_profile,
                    later,
                )
                .unwrap();
            assert!(fixture
                .core
                .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
                .unwrap()
                .is_empty());
            let saved = fixture
                .port
                .conversation_poll(&fixture.data_root, &session.token, 0)
                .unwrap();
            assert_eq!(saved.objects[0].delivery_state, Some(Accepted));

            // Core durably saved Accepted and pruned its signed receipt. The
            // older unaccepted snapshot can imply either Expired or Sending.
            for at in [later, now] {
                let resumed = resume_stale_poll(at);
                assert!(resumed.objects.is_empty());
                assert_eq!(resumed.latest_seq, own.seq);
                assert_eq!(resumed.own_delivery[0].state, Accepted);
            }
            let restarted = CollaborationChatProductPort::new(Arc::new(
                CollaborationCore::new(
                    &fixture.data_root,
                    fixture.device_key.clone(),
                    fixture.profile.clone(),
                    fixture.grant.clone(),
                    CHAT_ROOM_CAPSULE,
                )
                .unwrap(),
            ))
            .unwrap();
            let reopened = restarted
                .conversation_poll_with_read_intent(
                    &fixture.data_root,
                    &session.token,
                    0,
                    &fixture.person_profile,
                    false,
                    later,
                )
                .unwrap();
            assert_eq!(reopened.objects[0].delivery_state, Some(Accepted));
            assert_eq!(reopened.objects[0].seq, own.seq);
            assert_eq!(
                reopened.objects[0].body.as_deref(),
                Some("retained original")
            );
        }
    }

    #[test]
    fn community_delivery_observed_expiry_survives_core_pruning_and_restart() {
        use crate::room_service::CommunityDeliveryState::Expired;
        let fixture = fixture();
        let now = now_secs();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Local",
            "Expiry test",
        )
        .unwrap();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "expiry-own", "retained original"),
                "retained original",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        let own = fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
            .unwrap();
        let history = fixture.core.conversation_history(now).unwrap();
        let expired = delivery_poll(&fixture, &session.token, own.seq, prepared.expires_at);
        assert!(expired.objects.is_empty());
        assert_eq!(expired.own_delivery[0].state, Expired);
        let later = prepared.expires_at
            + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
            + 1;
        let blocked_temp = elastos_common::localhost::rooted_localhost_fs_path(
            &fixture.data_root,
            "localhost://Local/Shared/AppCapsules/chat-room",
        )
        .unwrap()
        .join("room/.objects.json.tmp");
        std::fs::create_dir(&blocked_temp).unwrap();
        let failed = fixture.port.prepare_message(
            binding(&fixture, "after-expiry", "new intent"),
            "new intent",
            &fixture.person_profile,
            later,
        );
        std::fs::remove_dir(&blocked_temp).unwrap();
        // The existing Expired row also needs a durable write before prune.
        assert!(failed.is_err());
        fixture
            .port
            .prepare_message(
                binding(&fixture, "after-expiry", "new intent"),
                "new intent",
                &fixture.person_profile,
                later,
            )
            .unwrap();
        assert!(fixture
            .core
            .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
            .unwrap()
            .is_empty());
        let restarted = CollaborationChatProductPort::new(Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        ))
        .unwrap();
        let reopened = restarted
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &session.token,
                0,
                &fixture.person_profile,
                false,
                later,
            )
            .unwrap();
        assert_eq!(reopened.objects[0].delivery_state, Some(Expired));
        assert_eq!(reopened.objects[0].seq, own.seq);
        assert_eq!(
            reopened.objects[0].body.as_deref(),
            Some("retained original")
        );
        assert_eq!(fixture.core.conversation_history(later).unwrap(), history);
    }

    #[test]
    fn community_delivery_lost_evidence_is_unknown_without_labeling_unobserved_history() {
        use crate::room_service::CommunityDeliveryState::{Sending, Unknown};
        let fixture = fixture();
        let now = now_secs();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Local",
            "Unknown test",
        )
        .unwrap();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "unknown-own", "observed original"),
                "observed original",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        let own = fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
            .unwrap();
        assert_eq!(
            delivery_poll(&fixture, &session.token, own.seq, now).own_delivery[0].state,
            Sending
        );
        // A cached signed original has no durable outgoing prepare evidence.
        let old = DefaultConversationDeviceAuthority::new(
            fixture.device_key.clone(),
            fixture.profile.clone(),
            fixture.grant.clone(),
        )
        .unwrap()
        .prepare_profile_outgoing(
            &fixture.person_profile,
            CHAT_SERVICE,
            CHAT_PAYLOAD_TYPE,
            serde_json::json!({"body":"unobserved original"}),
            now + 1,
            CHAT_MESSAGE_TTL_SECS,
        )
        .unwrap();
        fixture
            .port
            .project_history(
                &fixture.data_root,
                &[old.envelope_bytes().to_vec()],
                now + 1,
            )
            .unwrap();
        let later = old.verified_message().envelope().payload.expires_at
            + elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS
            + 1;
        // Simulate genuinely missing evidence, rather than normal pruning,
        // which records terminal outcomes even while Chat stays closed.
        let namespace =
            std::fs::read_dir(fixture.data_root.join("collaboration/default-conversation"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
        let path = namespace.join("state-v1.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        state["outgoing"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        fixture
            .port
            .prepare_message(
                binding(&fixture, "after-unknown", "new intent"),
                "new intent",
                &fixture.person_profile,
                later,
            )
            .unwrap();
        let view = delivery_poll(&fixture, &session.token, 0, later);
        assert_eq!(view.own_delivery.len(), 1);
        assert_eq!(view.own_delivery[0].state, Unknown);
        assert_eq!(view.objects[0].delivery_state, Some(Unknown));
        assert_eq!(view.objects[1].delivery_state, None);
        assert_eq!(view.objects.len(), 2);
    }

    #[test]
    fn community_delivery_is_private_to_the_current_profile_and_original_scope() {
        let fixture = fixture();
        let now = now_secs();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Local",
            "Profile test",
        )
        .unwrap();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "private-own", "private original"),
                "private original",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, Some(&session.token))
            .unwrap();
        let other = fixture
            .port
            .test_person_profile("Another profile", Some("another"));
        let other_session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &other.document().profile_did,
            "Another profile",
            "Profile test",
        )
        .unwrap();
        let other_view = fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &other_session.token,
                0,
                &other,
                false,
                now,
            )
            .unwrap();
        assert!(other_view.own_delivery.is_empty());
        assert_eq!(other_view.objects[0].delivery_state, None);
        assert!(fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &session.token,
                0,
                &other,
                false,
                now,
            )
            .is_err());
        let snapshot = fixture
            .core
            .outgoing_chat_delivery(&fixture.person_profile.document().profile_did)
            .unwrap();
        let foreign_scope = crate::room_service::collaboration_room_poll_with_delivery(
            &fixture.data_root,
            &session.token,
            ("foreign-network", "foreign-conversation"),
            0,
            &fixture.person_profile.document().profile_did,
            &snapshot,
            now,
        )
        .unwrap();
        assert!(foreign_scope.objects.is_empty());
        assert!(foreign_scope.own_delivery.is_empty());
    }

    #[test]
    fn community_unread_uses_scoped_other_authors_and_only_visible_returned_rows() {
        let fixture = fixture();
        let now = now_secs();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            "Reader",
            "Community unread test",
        )
        .unwrap();
        let own = fixture
            .port
            .prepare_message(
                binding(&fixture, "community-own", "own text"),
                "own text",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        fixture
            .port
            .project_prepared_message(&fixture.data_root, &own, Some(&session.token))
            .unwrap();
        assert!(!fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        let (remote, remote_profile) = remote_authority(&fixture);
        crate::room_service::project_collaboration_text(
            &fixture.data_root,
            ("foreign-network", "foreign-conversation"),
            &format!("sha256:{}", "f".repeat(64)),
            &room_profile_card(&remote_profile),
            "foreign text",
            now,
            CollaborationTextOrigin::Incoming,
        )
        .unwrap();
        assert!(!fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        let incoming = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"remote text"}),
                now,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        fixture
            .core
            .accept_incoming_from_signed_source_for_test(incoming.envelope_bytes(), now)
            .unwrap();
        let handoff = fixture.port.pending_messages().unwrap().remove(0);
        fixture
            .port
            .project_handoff(&fixture.data_root, &handoff)
            .unwrap();
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        // For the remote account, the first author's text is also unread.
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &remote_profile, now)
            .unwrap());
        let background = fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &session.token,
                0,
                &fixture.person_profile,
                false,
                now,
            )
            .unwrap();
        assert_eq!(background.objects.len(), 2);
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        // A client cannot turn its invented high cursor into read evidence.
        let empty = fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &session.token,
                u64::MAX,
                &fixture.person_profile,
                true,
                now,
            )
            .unwrap();
        assert!(empty.objects.is_empty());
        assert_eq!(empty.latest_seq, u64::MAX);
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        assert!(fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                "invalid-session",
                0,
                &fixture.person_profile,
                true,
                now,
            )
            .is_err());
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        let visible = fixture
            .port
            .conversation_poll_with_read_intent(
                &fixture.data_root,
                &session.token,
                0,
                &fixture.person_profile,
                true,
                now,
            )
            .unwrap();
        assert_eq!(visible.objects.len(), 2);
        assert!(!fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now)
            .unwrap());
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &remote_profile, now)
            .unwrap());
        let next = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"next remote text"}),
                now + 1,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        fixture
            .core
            .accept_incoming_from_signed_source_for_test(next.envelope_bytes(), now + 1)
            .unwrap();
        let handoff = fixture.port.pending_messages().unwrap().remove(0);
        fixture
            .port
            .project_handoff(&fixture.data_root, &handoff)
            .unwrap();
        assert!(fixture
            .port
            .community_unread(&fixture.data_root, &fixture.person_profile, now + 1)
            .unwrap());
    }

    #[test]
    fn product_retains_original_after_durable_projection_and_retries_a_failed_sidecar_write() {
        let fixture = fixture();
        let now = now_secs();
        let prepared = fixture
            .port
            .prepare_message(
                binding(&fixture, "history-product", "retained text"),
                "retained text",
                &fixture.person_profile,
                now,
            )
            .unwrap();
        assert!(fixture.port.retained_history(now).unwrap().is_empty());
        let namespace =
            std::fs::read_dir(fixture.data_root.join("collaboration/default-conversation"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
        let history_path = namespace.join("history-v1.json");
        // A storage refusal after the room write leaves the live projection
        // pending. Repairing storage lets the same exact request finish once.
        std::fs::create_dir(&history_path).unwrap();
        assert!(fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, None)
            .is_err());
        assert_eq!(
            fixture.port.pending_outgoing_messages(now).unwrap().len(),
            1
        );
        std::fs::remove_dir(&history_path).unwrap();
        let object = fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, None)
            .unwrap();
        let retry = fixture
            .port
            .project_prepared_message(&fixture.data_root, &prepared, None)
            .unwrap();
        assert_eq!(object.seq, retry.seq);
        assert!(fixture
            .port
            .pending_outgoing_messages(now)
            .unwrap()
            .is_empty());
        let bytes = fixture.port.retained_history(now).unwrap();
        assert_eq!(bytes, vec![prepared.envelope_bytes.clone()]);
        let restarted = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        assert_eq!(
            CollaborationChatProductPort::new(restarted)
                .unwrap()
                .retained_history(now)
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn outgoing_chat_is_fixed_bounded_and_restart_idempotent() {
        let fixture = fixture();
        let first = fixture
            .port
            .prepare_message(
                binding(&fixture, "chat-request-1", "hello"),
                "hello",
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        assert_eq!(first.body(), "hello");
        assert_eq!(first.issued_at(), NOW);
        assert_eq!(first.expires_at(), NOW + CHAT_MESSAGE_TTL_SECS);
        assert_eq!(first.envelope_sha256().len(), "sha256:".len() + 64);
        assert_eq!(
            first.sender_profile_did(),
            fixture.person_profile.document().profile_did
        );
        assert_eq!(
            fixture
                .port
                .prepare_message(
                    binding(&fixture, "chat-request-1", "hello"),
                    "hello",
                    &fixture.person_profile,
                    NOW,
                )
                .unwrap(),
            first
        );

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
        let restarted_port = CollaborationChatProductPort::new(restarted_core).unwrap();
        assert_eq!(
            restarted_port
                .prepare_message(
                    binding(&fixture, "chat-request-1", "hello"),
                    "hello",
                    &fixture.person_profile,
                    NOW,
                )
                .unwrap(),
            first
        );

        assert!(fixture
            .port
            .prepare_message(
                binding(&fixture, "empty", "placeholder"),
                "",
                &fixture.person_profile,
                NOW,
            )
            .is_err());
        let maximum = "x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN);
        assert_eq!(
            fixture
                .port
                .prepare_message(
                    binding(&fixture, "maximum", &maximum),
                    &maximum,
                    &fixture.person_profile,
                    NOW,
                )
                .unwrap()
                .body()
                .chars()
                .count(),
            crate::room_service::MAX_OBJECT_BODY_LEN
        );
        let oversized = "x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN + 1);
        assert!(fixture
            .port
            .prepare_message(
                binding(&fixture, "oversized", "placeholder"),
                &oversized,
                &fixture.person_profile,
                NOW,
            )
            .is_err());
        let mut wrong_binding = binding(&fixture, "wrong-context", "hello");
        wrong_binding.method = "another.action".to_string();
        assert!(fixture
            .port
            .prepare_message(wrong_binding, "hello", &fixture.person_profile, NOW)
            .is_err());
        let mut wrong_resource = binding(&fixture, "wrong-resource", "hello");
        wrong_resource.resources = vec!["elastos://chat/another".to_string()];
        assert!(fixture
            .port
            .prepare_message(wrong_resource, "hello", &fixture.person_profile, NOW)
            .is_err());
        let mut wrong_service = binding(&fixture, "wrong-service", "hello");
        wrong_service.capsule = CHAT_SERVICE.to_string();
        assert!(fixture
            .port
            .prepare_message(wrong_service, "hello", &fixture.person_profile, NOW)
            .is_err());
        let mut other_capsule = binding(&fixture, "other-capsule", "hello");
        other_capsule.capsule = "people".to_string();
        assert!(fixture
            .port
            .prepare_message(other_capsule, "hello", &fixture.person_profile, NOW)
            .is_err());
        assert!(fixture
            .port
            .prepare_message(
                binding(&fixture, "chat-request-1", "changed"),
                "changed",
                &fixture.person_profile,
                NOW,
            )
            .is_err());
        assert_ne!(
            fixture
                .port
                .prepare_message(
                    binding(&fixture, "chat-request-2", "hello"),
                    "hello",
                    &fixture.person_profile,
                    NOW,
                )
                .unwrap()
                .envelope_sha256(),
            first.envelope_sha256()
        );

        let wrong_capsule_core = CollaborationCore::new(
            &fixture.data_root,
            fixture.device_key.clone(),
            fixture.profile.clone(),
            fixture.grant.clone(),
            CHAT_SERVICE,
        )
        .unwrap();
        assert!(wrong_capsule_core.summary().is_err());
    }

    #[test]
    fn incoming_chat_projection_is_typed_opaque_and_restart_safe() {
        let fixture = fixture();
        let (remote, remote_profile) = remote_authority(&fixture);
        let known = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"known"}),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        assert!(remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                "elastos.chat.future/v1",
                serde_json::json!({"body":"future"}),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .is_err());
        let malformed = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"text":"not-body"}),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        let extra_field = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"not-exact","extra":true}),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        let oversized = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({
                    "body":"x".repeat(crate::room_service::MAX_OBJECT_BODY_LEN + 1)
                }),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        fixture
            .core
            .accept_incoming_from_signed_source_for_test(known.envelope_bytes(), NOW + 1)
            .unwrap();
        for envelope in [&malformed, &extra_field, &oversized] {
            assert!(fixture
                .core
                .accept_incoming_from_signed_source_for_test(envelope.envelope_bytes(), NOW + 1)
                .is_err());
        }

        let handoffs = fixture.port.pending_messages().unwrap();
        assert_eq!(handoffs.len(), 1);
        let handoff = &handoffs[0];
        assert_eq!(handoff.body(), "known");
        assert_eq!(handoff.issued_at(), NOW);
        assert_eq!(handoff.expires_at(), NOW + CHAT_MESSAGE_TTL_SECS);
        assert_eq!(
            handoff.sender_profile_did(),
            remote_profile.document().profile_did
        );
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 1);

        let other_temp = tempfile::tempdir().unwrap();
        let other_core = Arc::new(
            CollaborationCore::new(
                other_temp.path(),
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let other_port = CollaborationChatProductPort::new(other_core).unwrap();
        assert!(other_port
            .project_handoff(&fixture.data_root, handoff)
            .is_err());
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 1);

        let restarted = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_port = CollaborationChatProductPort::new(restarted.clone()).unwrap();
        let restarted_handoffs = restarted_port.pending_messages().unwrap();
        assert_eq!(restarted_handoffs.len(), 1);
        let restarted_handoff = &restarted_handoffs[0];
        assert_eq!(
            restarted_handoff.envelope_sha256(),
            handoff.envelope_sha256()
        );
        restarted_port
            .project_handoff(&fixture.data_root, restarted_handoff)
            .unwrap();
        restarted_port
            .project_handoff(&fixture.data_root, restarted_handoff)
            .unwrap();
        assert!(restarted_port.pending_messages().unwrap().is_empty());
        assert_eq!(restarted.summary().unwrap().pending_product_handoffs, 0);

        let restarted_again = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_again_port = CollaborationChatProductPort::new(restarted_again).unwrap();
        assert!(restarted_again_port.pending_messages().unwrap().is_empty());
    }

    #[test]
    fn chat_accepts_full_profile_names_and_projects_every_handoff_after_restart() {
        let fixture = fixture();
        let now = now_secs();
        let names = [
            "a".repeat(49),
            "b".repeat(64),
            "é".repeat(32),
            "c".repeat(63),
        ];
        for (profile_index, name) in names.iter().enumerate() {
            let (remote_key, _) = generate_keypair();
            let remote_did = crate::crypto::encode_signing_key_did(&remote_key);
            let (person_key, _) = generate_keypair();
            let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
                &person_key,
                name,
                None,
                1,
                None,
                now,
                vec![remote_did],
            )
            .unwrap();
            let authority = DefaultConversationDeviceAuthority::new(
                remote_key,
                fixture.profile.clone(),
                fixture.grant.clone(),
            )
            .unwrap();
            for post in 0..8 {
                let prepared = authority
                    .prepare_profile_outgoing(
                        &profile,
                        CHAT_SERVICE,
                        CHAT_PAYLOAD_TYPE,
                        serde_json::json!({"body": format!("post {profile_index}-{post}")}),
                        now,
                        CHAT_MESSAGE_TTL_SECS,
                    )
                    .unwrap();
                fixture
                    .core
                    .accept_incoming_from_signed_source_for_test(prepared.envelope_bytes(), now)
                    .unwrap();
            }
        }
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 32);
        let restarted = Arc::new(
            CollaborationCore::new(
                &fixture.data_root,
                fixture.device_key.clone(),
                fixture.profile.clone(),
                fixture.grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let port = CollaborationChatProductPort::new(restarted.clone()).unwrap();
        let handoffs = port.pending_messages().unwrap();
        assert_eq!(handoffs.len(), 32);
        for handoff in &handoffs {
            let projected = port.project_handoff(&fixture.data_root, handoff).unwrap();
            assert!(names.contains(&projected.sender));
        }
        assert!(port.pending_messages().unwrap().is_empty());
        assert_eq!(restarted.summary().unwrap().pending_product_handoffs, 0);
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &fixture.person_profile.document().profile_did,
            &names[1],
            "Long Profile name regression",
        )
        .unwrap();
        let poll = port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(poll.objects.len(), 32);
        assert_eq!(poll.display_name, names[1]);
        assert!(poll
            .objects
            .iter()
            .all(|object| names.contains(&object.sender)));
    }

    #[test]
    fn chat_projection_is_port_bound_restart_idempotent_and_scope_isolated() {
        let fixture = fixture();
        let other = fixture_at(
            &fixture.data_root,
            "other-collaboration-network",
            "other-default-conversation",
        );
        let local_did = fixture.person_profile.document().profile_did.clone();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &local_did,
            "Local runtime",
            "ElastOS shell",
        )
        .unwrap();
        let _ = crate::room_service::append_object(
            &fixture.data_root,
            &session.token,
            "legacy local history",
        )
        .unwrap();

        let first = fixture
            .port
            .prepare_message(
                binding(&fixture, "scoped-one", "first"),
                "first",
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        let second = other
            .port
            .prepare_message(
                chat_message_request_binding(
                    "scoped-two",
                    "runtime-principal",
                    "second",
                    &other.person_profile,
                )
                .unwrap(),
                "second",
                &other.person_profile,
                NOW + 1,
            )
            .unwrap();
        assert_eq!(
            fixture.port.pending_outgoing_messages(NOW).unwrap().len(),
            1
        );
        assert!(fixture.core.pending_outgoing(NOW).unwrap().is_empty());
        assert!(other
            .port
            .project_prepared_message(&fixture.data_root, &first, Some(&session.token))
            .is_err());
        assert!(fixture
            .port
            .project_prepared_message(&fixture.data_root, &second, Some(&session.token))
            .is_err());

        let first_object = fixture
            .port
            .project_prepared_message(&fixture.data_root, &first, Some(&session.token))
            .unwrap();
        assert!(fixture
            .port
            .pending_outgoing_messages(NOW)
            .unwrap()
            .is_empty());
        assert_eq!(fixture.core.pending_outgoing(NOW).unwrap().len(), 1);
        assert!(first_object.from_current_session);
        assert_eq!(
            first_object.sender_member_did.as_deref(),
            Some(local_did.as_str())
        );
        let second_object = other
            .port
            .project_prepared_message(&fixture.data_root, &second, None)
            .unwrap();
        assert!(!second_object.from_current_session);

        let first_feed = fixture
            .port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(first_feed.objects.len(), 1);
        assert_eq!(first_feed.objects[0].body.as_deref(), Some("first"));
        assert_eq!(first_feed.participants.len(), 1);
        assert_eq!(
            first_feed.participants[0].member_did.as_deref(),
            Some(local_did.as_str())
        );
        assert_eq!(first_feed.participants[0].role, None);
        let second_feed = other
            .port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(second_feed.objects.len(), 1);
        assert_eq!(second_feed.objects[0].body.as_deref(), Some("second"));
        assert_eq!(second_feed.participants.len(), 1);
        assert_eq!(
            second_feed.participants[0].member_did.as_deref(),
            Some(second.sender_profile_did())
        );
        assert_eq!(second_feed.participants[0].role, None);
        let legacy_feed =
            crate::room_service::conversation_feed(&fixture.data_root, &session.token, 0).unwrap();
        assert_eq!(legacy_feed.objects.len(), 2);
        assert!(legacy_feed
            .objects
            .iter()
            .all(|object| object.body.as_deref() != Some("first")
                && object.body.as_deref() != Some("second")));

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
        let restarted_port = CollaborationChatProductPort::new(restarted_core).unwrap();
        let restarted_prepared = restarted_port
            .prepare_message(
                binding(&fixture, "scoped-one", "first"),
                "first",
                &fixture.person_profile,
                NOW,
            )
            .unwrap();
        let replay = restarted_port
            .project_prepared_message(
                &fixture.data_root,
                &restarted_prepared,
                Some(&session.token),
            )
            .unwrap();
        assert_eq!(replay.seq, first_object.seq);
        assert_eq!(
            restarted_port
                .conversation_poll(&fixture.data_root, &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );
    }

    #[test]
    fn incoming_projection_writes_before_ack_and_retries_one_chat_object() {
        let fixture = fixture();
        let (remote, remote_profile) = remote_authority(&fixture);
        let incoming = remote
            .prepare_profile_outgoing(
                &remote_profile,
                CHAT_SERVICE,
                CHAT_PAYLOAD_TYPE,
                serde_json::json!({"body":"durable incoming"}),
                NOW,
                CHAT_MESSAGE_TTL_SECS,
            )
            .unwrap();
        fixture
            .core
            .accept_incoming_from_signed_source_for_test(incoming.envelope_bytes(), NOW + 1)
            .unwrap();
        let handoffs = fixture.port.pending_messages().unwrap();
        let handoff = &handoffs[0];

        let invalid_room_root = fixture.data_root.join("not-a-directory");
        std::fs::write(&invalid_room_root, b"file").unwrap();
        assert!(fixture
            .port
            .project_handoff(&invalid_room_root, handoff)
            .is_err());
        assert_eq!(fixture.port.pending_messages().unwrap().len(), 1);

        let core_namespace =
            std::fs::read_dir(fixture.data_root.join("collaboration/default-conversation"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
        let core_state = core_namespace.join("state-v1.json");
        let original_core_state = std::fs::read(&core_state).unwrap();
        std::fs::write(&core_state, b"{}").unwrap();
        assert!(fixture
            .port
            .project_handoff(&fixture.data_root, handoff)
            .is_err());
        std::fs::write(&core_state, &original_core_state).unwrap();
        assert_eq!(fixture.port.pending_messages().unwrap().len(), 1);

        let projected = fixture
            .port
            .project_handoff(&fixture.data_root, handoff)
            .unwrap();
        let replay = fixture
            .port
            .project_handoff(&fixture.data_root, handoff)
            .unwrap();
        assert_eq!(replay.seq, projected.seq);
        assert!(fixture.port.pending_messages().unwrap().is_empty());

        let remote_profile_did = remote_profile.document().profile_did.clone();
        let session = crate::room_service::start_local_runtime_session(
            &fixture.data_root,
            &remote_profile_did,
            "Remote person",
            "ElastOS shell",
        )
        .unwrap();
        let feed = fixture
            .port
            .conversation_poll(&fixture.data_root, &session.token, 0)
            .unwrap();
        assert_eq!(feed.objects.len(), 1);
        assert_eq!(feed.objects[0].seq, projected.seq);
        assert_eq!(feed.objects[0].body.as_deref(), Some("durable incoming"));
    }
}
