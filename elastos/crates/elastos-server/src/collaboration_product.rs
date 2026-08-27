//! Typed product boundary for the verified default collaboration conversation.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::collaboration_core::{
    CollaborationCore, DurableOutgoingMessage, PendingProductHandoff,
    DEFAULT_CONVERSATION_SEND_METHOD,
};
use crate::esp_binding::{esp_request_binding, EspRequestBinding};

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
}

/// Read-only result of durably preparing one outgoing Chat message.
#[derive(Clone)]
pub struct PreparedCollaborationChatMessage {
    core: Arc<CollaborationCore>,
    network_id: String,
    conversation_id: String,
    envelope_sha256: String,
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
        Ok(Self { core })
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
        crate::room_service::RoomTransportView {
            configured: true,
            available: true,
            status: Some("Collaboration is configured.".to_string()),
        }
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
    /// Unknown or malformed product payloads remain pending in the core.
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
            local_session_token,
        )?;
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
            None,
        )?;
        self.core
            .acknowledge_product_handoff(&handoff.envelope_sha256)?;
        Ok(object)
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
    let signer_did = crate::crypto::encode_did_key(&profile_signer.verifying_key());
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
    if message.payload_type != CHAT_PAYLOAD_TYPE {
        anyhow::bail!("collaboration outgoing payload is not the current Chat product");
    }
    let payload = exact_chat_payload(authorized.product_payload())?;
    let sender_profile = authorized.sender_profile().clone();
    let sender_profile_did = sender_profile.document().profile_did.clone();
    Ok(PreparedCollaborationChatMessage {
        core,
        network_id: message.network_id.clone(),
        conversation_id: message.conversation_id.clone(),
        envelope_sha256: prepared.envelope_sha256().to_string(),
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
    if payload.payload_type != CHAT_PAYLOAD_TYPE {
        return None;
    }
    let chat = exact_chat_payload(pending.authorized_message().product_payload()).ok()?;
    let sender_profile = pending.authorized_message().sender_profile().clone();
    let sender_profile_did = sender_profile.document().profile_did.clone();
    Some(CollaborationChatHandoff {
        core,
        network_id: payload.network_id.clone(),
        conversation_id: payload.conversation_id.clone(),
        envelope_sha256: message.envelope_sha256().to_string(),
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
        let signer_did = crate::crypto::encode_did_key(&profile_signer.verifying_key());
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
        let device_did = crate::crypto::encode_did_key(&device_key.verifying_key());
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

    fn remote_authority(
        fixture: &Fixture,
    ) -> (
        DefaultConversationDeviceAuthority,
        crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    ) {
        let (remote_key, _) = generate_keypair();
        let remote_did = crate::crypto::encode_did_key(&remote_key.verifying_key());
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
        for envelope in [&known, &malformed, &extra_field, &oversized] {
            fixture
                .core
                .accept_incoming_from_signed_source_for_test(envelope.envelope_bytes(), NOW + 1)
                .unwrap();
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
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 4);

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
        assert_eq!(fixture.core.summary().unwrap().pending_product_handoffs, 4);

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
        assert_eq!(restarted.summary().unwrap().pending_product_handoffs, 3);

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
