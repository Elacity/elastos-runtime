//! Pure authority checks for the profile-authenticated default conversation.

use anyhow::Context;
use elastos_common::collaboration_protocol::CollaborationRecipientKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_profile_authority::SignedCollaborationProfileDocument;
use crate::collaboration_protocol::{validate_id, validate_service, VerifiedCollaborationMessage};

pub const DEFAULT_CONVERSATION_GRANT_SCHEMA_V1: &str =
    "elastos.collaboration.default-conversation-grant/v1";

pub(crate) const MAX_DEFAULT_CONVERSATION_GRANT_BYTES: usize = 8 * 1024;
const RAW_CID_CODEC: u64 = 0x55;
const SHA2_256_MULTIHASH_CODE: u64 = 0x12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultConversationAdmissionPolicy {
    ProfileScopedSigner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultConversationGrant {
    pub schema: String,
    pub network_id: String,
    pub conversation_id: String,
    pub sender_service: String,
    pub admission_policy: DefaultConversationAdmissionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDefaultConversationGrant {
    grant: DefaultConversationGrant,
    grant_cid: String,
}

impl VerifiedDefaultConversationGrant {
    pub fn grant(&self) -> &DefaultConversationGrant {
        &self.grant
    }

    pub(crate) fn grant_cid(&self) -> &str {
        &self.grant_cid
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedDefaultConversationMessage {
    message: VerifiedCollaborationMessage,
    product_payload: serde_json::Value,
    sender_profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
}

impl AuthorizedDefaultConversationMessage {
    pub fn message(&self) -> &VerifiedCollaborationMessage {
        &self.message
    }

    pub(crate) fn product_payload(&self) -> &serde_json::Value {
        &self.product_payload
    }

    pub(crate) fn sender_profile(
        &self,
    ) -> &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        &self.sender_profile
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileAuthenticatedConversationPayload {
    product: serde_json::Value,
    signed_profile: SignedCollaborationProfileDocument,
}

pub(crate) fn profile_authenticated_conversation_payload(
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    product: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::to_value(
        ProfileAuthenticatedConversationPayload {
            product,
            signed_profile: profile.signed_envelope().clone(),
        },
    )?)
}

pub fn canonical_default_conversation_grant_bytes(
    grant: &DefaultConversationGrant,
) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::to_value(grant)?)
}

/// Verify exact grant bytes authenticated by the signed profile's raw SHA-256 CID.
///
/// The grant is not separately signed. Its `profile_scoped_signer` policy is an
/// open network-room policy: the message must carry a valid signed Profile that
/// authorizes both the sending endpoint and the scoped application signer. It
/// does not prove contact, private membership, delivery, or broader trust.
pub fn verify_default_conversation_grant(
    profile: &VerifiedCollaborationNetworkProfile,
    grant_bytes: &[u8],
) -> anyhow::Result<VerifiedDefaultConversationGrant> {
    if grant_bytes.is_empty() || grant_bytes.len() > MAX_DEFAULT_CONVERSATION_GRANT_BYTES {
        anyhow::bail!("default-conversation grant has an invalid byte length");
    }
    let descriptor = profile
        .profile()
        .default_conversation
        .as_ref()
        .context("collaboration profile has no default-conversation grant descriptor")?;
    if raw_sha256_cid(grant_bytes)? != descriptor.grant_cid {
        anyhow::bail!("default-conversation grant CID does not match the signed profile");
    }

    let grant: DefaultConversationGrant =
        serde_json::from_slice(grant_bytes).context("invalid default-conversation grant")?;
    if canonical_default_conversation_grant_bytes(&grant)? != grant_bytes {
        anyhow::bail!("default-conversation grant is not canonical JSON");
    }
    if grant.schema != DEFAULT_CONVERSATION_GRANT_SCHEMA_V1 {
        anyhow::bail!("unsupported default-conversation grant schema");
    }
    crate::collaboration_network::validate_network_id(&grant.network_id)?;
    if grant.network_id != profile.profile().network_id {
        anyhow::bail!("default-conversation grant belongs to another network");
    }
    validate_id(&grant.conversation_id, "default conversation_id")?;
    validate_service(&grant.sender_service)?;
    match grant.admission_policy {
        DefaultConversationAdmissionPolicy::ProfileScopedSigner => {}
    }

    Ok(VerifiedDefaultConversationGrant {
        grant,
        grant_cid: descriptor.grant_cid.clone(),
    })
}

/// Apply the verified open-room policy to one already verified signed message.
///
/// ```compile_fail
/// use elastos_common::collaboration_protocol::CollaborationMessage;
/// use elastos_server::collaboration_default_conversation::{
///     authorize_default_conversation_message, DefaultConversationGrant,
/// };
///
/// fn raw_values_are_not_authority(
///     raw_grant: &DefaultConversationGrant,
///     raw_message: &CollaborationMessage,
/// ) {
///     authorize_default_conversation_message(raw_grant, raw_message).unwrap();
/// }
/// ```
pub fn authorize_default_conversation_message(
    grant: &VerifiedDefaultConversationGrant,
    message: &VerifiedCollaborationMessage,
) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
    let expected = grant.grant();
    let envelope = message.envelope();
    if envelope.payload.network_id != expected.network_id {
        anyhow::bail!("collaboration message belongs to another default-conversation network");
    }
    if envelope.payload.conversation_id != expected.conversation_id {
        anyhow::bail!("collaboration message targets another default conversation");
    }
    if envelope.payload.sender_service != expected.sender_service {
        anyhow::bail!("collaboration message uses another default-conversation service");
    }
    if envelope.payload.recipient.kind != CollaborationRecipientKind::Conversation
        || envelope.payload.recipient.id != expected.conversation_id
    {
        anyhow::bail!("default-conversation grant requires its exact conversation recipient");
    }
    let wrapped: ProfileAuthenticatedConversationPayload =
        serde_json::from_value(envelope.payload.payload.clone())
            .context("default-conversation Profile payload is malformed")?;
    if serde_json::to_value(&wrapped)? != envelope.payload.payload {
        anyhow::bail!("default-conversation Profile payload is not canonical");
    }
    let profile = crate::collaboration_profile_authority::verify_signed_profile_document(
        &wrapped.signed_profile,
    )
    .context("default-conversation message has an invalid signed Profile")?;
    if profile.document().profile_did != envelope.payload.sender_profile_did {
        anyhow::bail!("default-conversation sender Profile does not match its signed Profile");
    }
    if !profile.authorizes_signer(
        &envelope.signer_did,
        &envelope.payload.sender_service,
        &envelope.payload.payload_type,
    ) {
        anyhow::bail!("default-conversation signer is not authorized by its Profile");
    }

    Ok(AuthorizedDefaultConversationMessage {
        message: message.clone(),
        product_payload: wrapped.product,
        sender_profile: profile,
    })
}

pub(crate) fn authorize_default_conversation_transport_message(
    grant: &VerifiedDefaultConversationGrant,
    message: &VerifiedCollaborationMessage,
    source_endpoint_did: &str,
) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
    let authorized = authorize_default_conversation_message(grant, message)?;
    if !authorized
        .sender_profile
        .authorizes_endpoint(source_endpoint_did)
    {
        anyhow::bail!("default-conversation source endpoint is not authorized by its Profile");
    }
    Ok(authorized)
}

pub(crate) fn raw_sha256_cid(bytes: &[u8]) -> anyhow::Result<String> {
    let digest = Sha256::digest(bytes);
    let multihash =
        cid::multihash::Multihash::<64>::wrap(SHA2_256_MULTIHASH_CODE, digest.as_slice())?;
    Ok(cid::Cid::new_v1(RAW_CID_CODEC, multihash).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use elastos_common::collaboration_protocol::{
        canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
        CollaborationMessage, CollaborationRecipient, SignedCollaborationMessage,
        COLLABORATION_MESSAGE_SCHEMA_V1, COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
    };
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, DefaultConversationGrantDescriptor,
        SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_protocol::verify_collaboration_message;

    const NETWORK: &str = "collaboration-default-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const NOW: u64 = 1_800_000_000;

    fn grant() -> DefaultConversationGrant {
        DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: CONVERSATION.to_string(),
            sender_service: SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        }
    }

    fn verified_profile(
        signing_key: &SigningKey,
        network_id: &str,
        revision: u64,
        previous: Option<&VerifiedCollaborationNetworkProfile>,
        grant_cid: Option<String>,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision,
            previous_profile_sha256: previous.map(|profile| profile.profile_sha256().to_string()),
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: grant_cid
                .map(|grant_cid| DefaultConversationGrantDescriptor { grant_cid }),
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
        match validate_collaboration_network_profile(
            Some(&bytes),
            network_id,
            &[signer_did],
            previous,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn profile_for_bytes(
        signing_key: &SigningKey,
        grant_bytes: &[u8],
    ) -> VerifiedCollaborationNetworkProfile {
        verified_profile(
            signing_key,
            NETWORK,
            1,
            None,
            Some(raw_sha256_cid(grant_bytes).unwrap()),
        )
    }

    fn message(
        _sender: &SigningKey,
        person_profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        network_id: &str,
        conversation_id: &str,
        sender_service: &str,
        recipient: CollaborationRecipient,
    ) -> CollaborationMessage {
        CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: network_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: "0123456789abcdef0123456789abcdef".to_string(),
            nonce: "abcdef0123456789abcdef0123456789".to_string(),
            created_at: NOW,
            expires_at: NOW + 300,
            sender_profile_did: person_profile.document().profile_did.clone(),
            sender_service: sender_service.to_string(),
            recipient,
            payload_type: "elastos.chat.message/v1".to_string(),
            payload: profile_authenticated_conversation_payload(
                person_profile,
                serde_json::json!({"body": "hello"}),
            )
            .unwrap(),
        }
    }

    fn test_person_profile(
        endpoint: &SigningKey,
    ) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        let (profile_key, _) = generate_keypair();
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_key,
            "Alice",
            Some("alice"),
            1,
            None,
            NOW,
            vec![crate::crypto::encode_did_key(&endpoint.verifying_key())],
        )
        .unwrap()
    }

    fn sign_message(sender: &SigningKey, payload: CollaborationMessage) -> Vec<u8> {
        let payload_bytes = canonical_collaboration_message_bytes(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            sender,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
            payload,
            signature,
            signer_did,
        })
        .unwrap()
    }

    fn conversation_recipient(conversation_id: &str) -> CollaborationRecipient {
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Conversation,
            id: conversation_id.to_string(),
        }
    }

    #[test]
    fn exact_canonical_grant_and_profile_cid_validate_deterministically_without_side_effects() {
        let (profile_signer, _) = generate_keypair();
        let bytes = canonical_default_conversation_grant_bytes(&grant()).unwrap();
        let profile = profile_for_bytes(&profile_signer, &bytes);
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("unchanged");
        std::fs::write(&marker, b"original").unwrap();

        let first = verify_default_conversation_grant(&profile, &bytes).unwrap();
        let second = verify_default_conversation_grant(&profile, &bytes).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.grant(), &grant());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["admission_policy"],
            "profile_scoped_signer"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(marker_dir.path()).unwrap().count(), 1);

        let mut altered = bytes.clone();
        altered.push(b' ');
        assert!(verify_default_conversation_grant(&profile, &altered).is_err());

        let wrong_cid_profile = verified_profile(
            &profile_signer,
            NETWORK,
            1,
            None,
            Some(raw_sha256_cid(b"other grant").unwrap()),
        );
        assert!(verify_default_conversation_grant(&wrong_cid_profile, &bytes).is_err());

        let missing = verified_profile(&profile_signer, NETWORK, 1, None, None);
        assert!(verify_default_conversation_grant(&missing, &bytes).is_err());
    }

    #[test]
    fn grant_rejects_noncanonical_unknown_and_invalid_contract_fields() {
        let (profile_signer, _) = generate_keypair();
        let canonical = canonical_default_conversation_grant_bytes(&grant()).unwrap();

        let mut noncanonical = canonical.clone();
        noncanonical.push(b'\n');
        assert!(verify_default_conversation_grant(
            &profile_for_bytes(&profile_signer, &noncanonical),
            &noncanonical,
        )
        .unwrap_err()
        .to_string()
        .contains("canonical"));

        let mut unknown: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        unknown["title"] = serde_json::json!("forbidden");
        let unknown = serde_json::to_vec(&unknown).unwrap();
        assert!(verify_default_conversation_grant(
            &profile_for_bytes(&profile_signer, &unknown),
            &unknown,
        )
        .is_err());

        for candidate in [
            DefaultConversationGrant {
                schema: "elastos.collaboration.default-conversation-grant/v2".to_string(),
                ..grant()
            },
            DefaultConversationGrant {
                network_id: "another-network".to_string(),
                ..grant()
            },
            DefaultConversationGrant {
                conversation_id: "Not-Canonical".to_string(),
                ..grant()
            },
            DefaultConversationGrant {
                sender_service: "chat/service".to_string(),
                ..grant()
            },
        ] {
            let bytes = canonical_default_conversation_grant_bytes(&candidate).unwrap();
            assert!(verify_default_conversation_grant(
                &profile_for_bytes(&profile_signer, &bytes),
                &bytes,
            )
            .is_err());
        }

        let mut invalid_policy: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        invalid_policy["admission_policy"] = serde_json::json!("members_only");
        let invalid_policy = serde_json::to_vec(&invalid_policy).unwrap();
        assert!(verify_default_conversation_grant(
            &profile_for_bytes(&profile_signer, &invalid_policy),
            &invalid_policy,
        )
        .is_err());

        let oversized = vec![b' '; MAX_DEFAULT_CONVERSATION_GRANT_BYTES + 1];
        assert!(verify_default_conversation_grant(
            &profile_for_bytes(&profile_signer, &oversized),
            &oversized,
        )
        .is_err());
    }

    #[test]
    fn grant_remains_valid_across_profile_revision_with_same_network_and_cid() {
        let (profile_signer, _) = generate_keypair();
        let bytes = canonical_default_conversation_grant_bytes(&grant()).unwrap();
        let grant_cid = raw_sha256_cid(&bytes).unwrap();
        let initial = verified_profile(&profile_signer, NETWORK, 1, None, Some(grant_cid.clone()));
        let updated =
            verified_profile(&profile_signer, NETWORK, 2, Some(&initial), Some(grant_cid));

        assert_eq!(
            verify_default_conversation_grant(&initial, &bytes).unwrap(),
            verify_default_conversation_grant(&updated, &bytes).unwrap()
        );
    }

    #[test]
    fn open_room_grant_requires_profile_authorized_transport_endpoint_and_signer_for_exact_target()
    {
        let (profile_signer, _) = generate_keypair();
        let grant_bytes = canonical_default_conversation_grant_bytes(&grant()).unwrap();
        let profile = profile_for_bytes(&profile_signer, &grant_bytes);
        let verified_grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
        let (device, _) = generate_keypair();
        let person_profile = test_person_profile(&device);
        let valid_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                NETWORK,
                CONVERSATION,
                SERVICE,
                conversation_recipient(CONVERSATION),
            ),
        );
        let verified = verify_collaboration_message(&valid_bytes, &profile, SERVICE, NOW).unwrap();

        let authorized = authorize_default_conversation_transport_message(
            &verified_grant,
            &verified,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .unwrap();
        assert_eq!(authorized.message(), &verified);

        let wrong_conversation_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                NETWORK,
                "another-conversation",
                SERVICE,
                conversation_recipient("another-conversation"),
            ),
        );
        let wrong_conversation =
            verify_collaboration_message(&wrong_conversation_bytes, &profile, SERVICE, NOW)
                .unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &wrong_conversation,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let wrong_service_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                NETWORK,
                CONVERSATION,
                "people",
                conversation_recipient(CONVERSATION),
            ),
        );
        let wrong_service =
            verify_collaboration_message(&wrong_service_bytes, &profile, "people", NOW).unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &wrong_service,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let other_profile = verified_profile(&profile_signer, "another-network", 1, None, None);
        let wrong_network_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                "another-network",
                CONVERSATION,
                SERVICE,
                conversation_recipient(CONVERSATION),
            ),
        );
        let wrong_network =
            verify_collaboration_message(&wrong_network_bytes, &other_profile, SERVICE, NOW)
                .unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &wrong_network,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let direct_device_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                NETWORK,
                CONVERSATION,
                SERVICE,
                CollaborationRecipient {
                    kind: CollaborationRecipientKind::Profile,
                    id: crate::crypto::encode_did_key(&device.verifying_key()),
                },
            ),
        );
        let direct_device =
            verify_collaboration_message(&direct_device_bytes, &profile, SERVICE, NOW).unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &direct_device,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let mut missing_profile: SignedCollaborationMessage =
            serde_json::from_slice(&valid_bytes).unwrap();
        missing_profile.payload.payload = serde_json::json!({"body":"hello"});
        let missing_profile = verify_collaboration_message(
            &sign_message(&device, missing_profile.payload),
            &profile,
            SERVICE,
            NOW,
        )
        .unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &missing_profile,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let (foreign_endpoint, _) = generate_keypair();
        let foreign_endpoint_profile = test_person_profile(&foreign_endpoint);
        let wrong_endpoint = verify_collaboration_message(
            &sign_message(
                &device,
                message(
                    &device,
                    &foreign_endpoint_profile,
                    NETWORK,
                    CONVERSATION,
                    SERVICE,
                    conversation_recipient(CONVERSATION),
                ),
            ),
            &profile,
            SERVICE,
            NOW,
        )
        .unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &wrong_endpoint,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let (foreign_signer, _) = generate_keypair();
        let (wrong_signer_profile_key, _) = generate_keypair();
        let wrong_signer_profile =
            crate::collaboration_profile_authority::signed_profile_document_with_authority_for_test(
                &wrong_signer_profile_key,
                "Alice",
                Some("alice"),
                1,
                None,
                NOW,
                crate::collaboration_profile_authority::ProfileAuthorityForTest {
                    endpoint_dids: vec![crate::crypto::encode_did_key(
                        &device.verifying_key(),
                    )],
                    signer_dids: vec![crate::crypto::encode_did_key(
                        &foreign_signer.verifying_key(),
                    )],
                },
            )
            .unwrap();
        let wrong_signer = verify_collaboration_message(
            &sign_message(
                &device,
                message(
                    &device,
                    &wrong_signer_profile,
                    NETWORK,
                    CONVERSATION,
                    SERVICE,
                    conversation_recipient(CONVERSATION),
                ),
            ),
            &profile,
            SERVICE,
            NOW,
        )
        .unwrap();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &wrong_signer,
            &crate::crypto::encode_did_key(&device.verifying_key()),
        )
        .is_err());

        let (transport_substitute, _) = generate_keypair();
        assert!(authorize_default_conversation_transport_message(
            &verified_grant,
            &verified,
            &crate::crypto::encode_did_key(&transport_substitute.verifying_key()),
        )
        .is_err());
    }

    #[test]
    fn tampered_expired_and_unverified_messages_cannot_reach_authorization() {
        let (profile_signer, _) = generate_keypair();
        let grant_bytes = canonical_default_conversation_grant_bytes(&grant()).unwrap();
        let profile = profile_for_bytes(&profile_signer, &grant_bytes);
        let (device, _) = generate_keypair();
        let person_profile = test_person_profile(&device);
        let valid_bytes = sign_message(
            &device,
            message(
                &device,
                &person_profile,
                NETWORK,
                CONVERSATION,
                SERVICE,
                conversation_recipient(CONVERSATION),
            ),
        );

        let mut tampered: SignedCollaborationMessage =
            serde_json::from_slice(&valid_bytes).unwrap();
        tampered.payload.payload["content"] = serde_json::json!("tampered");
        assert!(verify_collaboration_message(
            &canonical_signed_collaboration_message_bytes(&tampered).unwrap(),
            &profile,
            SERVICE,
            NOW,
        )
        .is_err());

        let mut expired = message(
            &device,
            &person_profile,
            NETWORK,
            CONVERSATION,
            SERVICE,
            conversation_recipient(CONVERSATION),
        );
        expired.created_at = NOW - 300;
        expired.expires_at = NOW;
        assert!(verify_collaboration_message(
            &sign_message(&device, expired),
            &profile,
            SERVICE,
            NOW,
        )
        .is_err());
    }
}
