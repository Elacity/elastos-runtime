//! Crate-private signing authority for the verified open default conversation.

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    canonical_collaboration_acceptance_receipt_bytes, canonical_collaboration_message_bytes,
    canonical_signed_collaboration_acceptance_receipt_bytes,
    canonical_signed_collaboration_message_bytes, CollaborationAcceptanceReceipt,
    CollaborationMessage, CollaborationRecipient, CollaborationRecipientKind,
    SignedCollaborationAcceptanceReceipt, SignedCollaborationMessage,
    COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1,
    COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1, COLLABORATION_MESSAGE_SCHEMA_V1,
    COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1, MAX_COLLABORATION_MESSAGE_LIFETIME_SECS,
    MAX_COLLABORATION_PAYLOAD_BYTES,
};
use elastos_runtime::signature::SigningKey;

use crate::collaboration_default_conversation::{
    authorize_default_conversation_message, authorize_default_conversation_transport_message,
    profile_authenticated_conversation_payload, AuthorizedDefaultConversationMessage,
    VerifiedDefaultConversationGrant,
};
use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_protocol::{
    sign_collaboration_transport_frame, validate_payload_type,
    verify_collaboration_acceptance_receipt, verify_collaboration_message,
    VerifiedCollaborationMessage,
};

pub(crate) struct DefaultConversationDeviceAuthority {
    signing_key: SigningKey,
    profile: VerifiedCollaborationNetworkProfile,
    grant: VerifiedDefaultConversationGrant,
}

pub(crate) struct PreparedDefaultConversationMessage {
    envelope_bytes: Vec<u8>,
    authorized: AuthorizedDefaultConversationMessage,
}

impl PreparedDefaultConversationMessage {
    pub(crate) fn envelope_bytes(&self) -> &[u8] {
        &self.envelope_bytes
    }

    pub(crate) fn envelope_sha256(&self) -> &str {
        self.authorized.message().envelope_sha256()
    }

    pub(crate) fn verified_message(&self) -> &VerifiedCollaborationMessage {
        self.authorized.message()
    }
}

impl DefaultConversationDeviceAuthority {
    pub(crate) fn new(
        signing_key: SigningKey,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
    ) -> anyhow::Result<Self> {
        if profile.profile().network_id != grant.grant().network_id {
            anyhow::bail!("default-conversation device authority network mismatch");
        }
        let descriptor = profile
            .profile()
            .default_conversation
            .as_ref()
            .context("device authority profile has no default-conversation descriptor")?;
        if descriptor.grant_cid != grant.grant_cid() {
            anyhow::bail!("default-conversation device authority grant CID mismatch");
        }
        Ok(Self {
            signing_key,
            profile,
            grant,
        })
    }

    pub(crate) fn prepare_profile_outgoing(
        &self,
        sender_profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        runtime_service: &str,
        payload_type: &str,
        payload: serde_json::Value,
        now: u64,
        ttl_secs: u64,
    ) -> anyhow::Result<PreparedDefaultConversationMessage> {
        let grant = self.grant.grant();
        if runtime_service != grant.sender_service {
            anyhow::bail!("Runtime-bound service does not match default-conversation authority");
        }
        validate_payload_type(payload_type)?;
        if serde_json::to_vec(&payload)?.len() > MAX_COLLABORATION_PAYLOAD_BYTES {
            anyhow::bail!("collaboration message payload is too large");
        }
        if ttl_secs == 0 || ttl_secs > MAX_COLLABORATION_MESSAGE_LIFETIME_SECS {
            anyhow::bail!("collaboration message TTL is outside the allowed range");
        }
        let expires_at = now
            .checked_add(ttl_secs)
            .context("collaboration message TTL overflows its timestamp")?;

        let signer_did = crate::crypto::encode_did_key(&self.signing_key.verifying_key());
        if !sender_profile.authorizes_endpoint(&signer_did)
            || !sender_profile.authorizes_signer(&signer_did, runtime_service, payload_type)
        {
            anyhow::bail!("sender Profile does not authorize this collaboration message");
        }
        let payload = profile_authenticated_conversation_payload(sender_profile, payload)?;
        let message = CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: grant.network_id.clone(),
            conversation_id: grant.conversation_id.clone(),
            message_id: random_128_bit_hex()?,
            nonce: random_128_bit_hex()?,
            created_at: now,
            expires_at,
            sender_profile_did: sender_profile.document().profile_did.clone(),
            sender_service: grant.sender_service.clone(),
            recipient: CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: grant.conversation_id.clone(),
            },
            payload_type: payload_type.to_string(),
            payload,
        };
        let payload_bytes = canonical_collaboration_message_bytes(&message)?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &self.signing_key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        let envelope_bytes =
            canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
                payload: message,
                signature,
                signer_did,
            })?;
        let verified = verify_collaboration_message(
            &envelope_bytes,
            &self.profile,
            &grant.sender_service,
            now,
        )?;
        let authorized = authorize_default_conversation_message(&self.grant, &verified)?;

        Ok(PreparedDefaultConversationMessage {
            envelope_bytes,
            authorized,
        })
    }

    #[cfg(test)]
    pub(crate) fn prepare_outgoing(
        &self,
        runtime_service: &str,
        payload_type: &str,
        payload: serde_json::Value,
        now: u64,
        ttl_secs: u64,
    ) -> anyhow::Result<PreparedDefaultConversationMessage> {
        let profile = self.sender_profile_for_test()?;
        self.prepare_profile_outgoing(
            &profile,
            runtime_service,
            payload_type,
            payload,
            now,
            ttl_secs,
        )
    }

    #[cfg(test)]
    pub(crate) fn sender_profile_for_test(
        &self,
    ) -> anyhow::Result<crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument>
    {
        let mut profile_seed = self.signing_key.to_bytes();
        profile_seed[0] ^= 0xa5;
        profile_seed[31] ^= 0x5a;
        let profile_key = SigningKey::from_bytes(&profile_seed);
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_key,
            "Test Profile",
            None,
            1,
            None,
            1_800_000_000,
            vec![self.local_device_did()],
        )
    }

    pub(crate) fn prepare_acceptance_receipt(
        &self,
        message: &AuthorizedDefaultConversationMessage,
        accepted_at: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let message = message.message();
        let payload = CollaborationAcceptanceReceipt {
            schema: COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1.to_string(),
            network_id: message.envelope().payload.network_id.clone(),
            message_envelope_sha256: message.envelope_sha256().to_string(),
            conversation_id: message.envelope().payload.conversation_id.clone(),
            sender_profile_did: message.envelope().payload.sender_profile_did.clone(),
            message_id: message.envelope().payload.message_id.clone(),
            message_nonce: message.envelope().payload.nonce.clone(),
            recipient_endpoint_did: self.local_device_did(),
            accepted_at,
        };
        let payload_bytes = canonical_collaboration_acceptance_receipt_bytes(&payload)?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &self.signing_key,
            COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        let receipt_bytes = canonical_signed_collaboration_acceptance_receipt_bytes(
            &SignedCollaborationAcceptanceReceipt {
                payload,
                signature,
                signer_did,
            },
        )?;
        verify_collaboration_acceptance_receipt(&receipt_bytes, message, accepted_at)?;
        Ok(receipt_bytes)
    }

    pub(crate) fn prepare_transport_frame(&self, envelope_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        sign_collaboration_transport_frame(&self.signing_key, envelope_bytes)
    }

    pub(crate) fn authorize_incoming(
        &self,
        envelope_bytes: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<AuthorizedDefaultConversationMessage> {
        let verified = verify_collaboration_message(
            envelope_bytes,
            &self.profile,
            &self.grant.grant().sender_service,
            now,
        )?;
        authorize_default_conversation_transport_message(
            &self.grant,
            &verified,
            source_endpoint_did,
        )
    }

    pub(crate) fn network_id(&self) -> &str {
        &self.profile.profile().network_id
    }

    pub(crate) fn grant_cid(&self) -> &str {
        self.grant.grant_cid()
    }

    pub(crate) fn sender_service(&self) -> &str {
        &self.grant.grant().sender_service
    }

    pub(crate) fn local_device_did(&self) -> String {
        crate::crypto::encode_did_key(&self.signing_key.verifying_key())
    }

    pub(crate) fn profile(&self) -> &VerifiedCollaborationNetworkProfile {
        &self.profile
    }

    pub(crate) fn grant(&self) -> &VerifiedDefaultConversationGrant {
        &self.grant
    }
}

fn random_128_bit_hex() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).context("OS randomness unavailable for collaboration ID")?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use elastos_common::collaboration_protocol::{
        collaboration_message_envelope_sha256, CollaborationRecipient,
        MAX_COLLABORATION_ENVELOPE_BYTES, MAX_COLLABORATION_PAYLOAD_TYPE_BYTES,
    };
    use elastos_runtime::signature::generate_keypair;
    use sha2::Digest;

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

    const NETWORK: &str = "collaboration-device-authority-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const NOW: u64 = 1_800_000_000;

    fn raw_sha256_cid(bytes: &[u8]) -> String {
        let digest = sha2::Sha256::digest(bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, multihash).to_string()
    }

    fn grant(network_id: &str) -> DefaultConversationGrant {
        DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: network_id.to_string(),
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

    fn authority(network_id: &str) -> (DefaultConversationDeviceAuthority, String, String) {
        let (profile_signer, _) = generate_keypair();
        let profile_signer_did = crate::crypto::encode_did_key(&profile_signer.verifying_key());
        let grant_bytes = canonical_default_conversation_grant_bytes(&grant(network_id)).unwrap();
        let profile = verified_profile(
            &profile_signer,
            network_id,
            1,
            None,
            Some(raw_sha256_cid(&grant_bytes)),
        );
        let verified_grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
        let (device_key, _) = generate_keypair();
        let device_did = crate::crypto::encode_did_key(&device_key.verifying_key());
        (
            DefaultConversationDeviceAuthority::new(device_key, profile, verified_grant).unwrap(),
            profile_signer_did,
            device_did,
        )
    }

    fn message(
        sender: &SigningKey,
        network_id: &str,
        conversation_id: &str,
        sender_service: &str,
        recipient: CollaborationRecipient,
    ) -> CollaborationMessage {
        let (profile_key, _) = generate_keypair();
        let sender_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &profile_key,
                "Remote Profile",
                None,
                1,
                None,
                NOW,
                vec![crate::crypto::encode_did_key(&sender.verifying_key())],
            )
            .unwrap();
        CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: network_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: "0123456789abcdef0123456789abcdef".to_string(),
            nonce: "abcdef0123456789abcdef0123456789".to_string(),
            created_at: NOW,
            expires_at: NOW + 300,
            sender_profile_did: sender_profile.document().profile_did.clone(),
            sender_service: sender_service.to_string(),
            recipient,
            payload_type: "elastos.chat.message/v1".to_string(),
            payload: profile_authenticated_conversation_payload(
                &sender_profile,
                serde_json::json!({"content": "hello"}),
            )
            .unwrap(),
        }
    }

    fn sign_message(sender: &SigningKey, message: CollaborationMessage) -> Vec<u8> {
        let payload_bytes = canonical_collaboration_message_bytes(&message).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            sender,
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

    fn conversation_recipient(conversation_id: &str) -> CollaborationRecipient {
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Conversation,
            id: conversation_id.to_string(),
        }
    }

    #[test]
    fn prepared_messages_derive_authority_sign_and_use_independent_random_ids() {
        let (authority, profile_signer_did, device_did) = authority(NETWORK);
        let sender_profile = authority.sender_profile_for_test().unwrap();
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("unchanged");
        std::fs::write(&marker, b"original").unwrap();

        let first = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"content": "one"}),
                NOW,
                300,
            )
            .unwrap();
        let second = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"content": "two"}),
                NOW,
                300,
            )
            .unwrap();
        let first_envelope: SignedCollaborationMessage =
            serde_json::from_slice(first.envelope_bytes()).unwrap();
        let second_envelope: SignedCollaborationMessage =
            serde_json::from_slice(second.envelope_bytes()).unwrap();
        let ids = HashSet::from([
            first_envelope.payload.message_id.as_str(),
            first_envelope.payload.nonce.as_str(),
            second_envelope.payload.message_id.as_str(),
            second_envelope.payload.nonce.as_str(),
        ]);

        assert_eq!(ids.len(), 4);
        assert!(ids.iter().all(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }));
        assert_eq!(first_envelope.payload.network_id, NETWORK);
        assert_eq!(first_envelope.payload.conversation_id, CONVERSATION);
        assert_eq!(first_envelope.payload.sender_service, SERVICE);
        assert_eq!(
            first_envelope.payload.sender_profile_did,
            sender_profile.document().profile_did
        );
        assert_eq!(first_envelope.signer_did, device_did);
        assert_ne!(first_envelope.payload.sender_profile_did, device_did);
        assert_ne!(first_envelope.signer_did, profile_signer_did);
        assert_eq!(
            first_envelope.payload.recipient,
            conversation_recipient(CONVERSATION)
        );
        assert_eq!(
            first.envelope_sha256(),
            collaboration_message_envelope_sha256(first.envelope_bytes())
        );
        assert_eq!(first.verified_message().envelope(), &first_envelope);
        assert!(authority
            .authorize_incoming(first.envelope_bytes(), &device_did, NOW)
            .is_ok());
        assert_eq!(std::fs::read(&marker).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(marker_dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn outgoing_rejects_service_ttl_and_payload_inputs_before_returning_a_handle() {
        let (authority, _, _) = authority(NETWORK);
        let payload = serde_json::json!({"content": "hello"});

        assert!(authority
            .prepare_outgoing("people", "elastos.chat.message/v1", payload.clone(), NOW, 1)
            .is_err());
        assert!(authority
            .prepare_outgoing(SERVICE, "elastos.chat.message/v1", payload.clone(), NOW, 0)
            .is_err());
        assert!(authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                payload.clone(),
                NOW,
                MAX_COLLABORATION_MESSAGE_LIFETIME_SECS + 1,
            )
            .is_err());
        assert!(authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                payload.clone(),
                u64::MAX,
                1,
            )
            .is_err());
        assert!(authority
            .prepare_outgoing(SERVICE, "Not-Canonical", payload.clone(), NOW, 1)
            .is_err());
        assert!(authority
            .prepare_outgoing(
                SERVICE,
                &"a".repeat(MAX_COLLABORATION_PAYLOAD_TYPE_BYTES + 1),
                payload,
                NOW,
                1,
            )
            .is_err());
        assert!(authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({
                    "content": "a".repeat(MAX_COLLABORATION_PAYLOAD_BYTES),
                }),
                NOW,
                1,
            )
            .is_err());
    }

    #[test]
    fn incoming_accepts_exact_signed_key_and_rejects_wrong_or_invalid_envelopes() {
        let (authority, _, _) = authority(NETWORK);
        let (remote_key, _) = generate_keypair();
        let exact = sign_message(
            &remote_key,
            message(
                &remote_key,
                NETWORK,
                CONVERSATION,
                SERVICE,
                conversation_recipient(CONVERSATION),
            ),
        );
        let authorized = authority
            .authorize_incoming(
                &exact,
                &crate::crypto::encode_did_key(&remote_key.verifying_key()),
                NOW,
            )
            .unwrap();
        assert_eq!(
            authorized.message().envelope().signer_did,
            crate::crypto::encode_did_key(&remote_key.verifying_key())
        );

        let wrong_network = sign_message(
            &remote_key,
            message(
                &remote_key,
                "another-network",
                CONVERSATION,
                SERVICE,
                conversation_recipient(CONVERSATION),
            ),
        );
        let wrong_conversation = sign_message(
            &remote_key,
            message(
                &remote_key,
                NETWORK,
                "another-conversation",
                SERVICE,
                conversation_recipient("another-conversation"),
            ),
        );
        let wrong_service = sign_message(
            &remote_key,
            message(
                &remote_key,
                NETWORK,
                CONVERSATION,
                "people",
                conversation_recipient(CONVERSATION),
            ),
        );
        let wrong_recipient = sign_message(
            &remote_key,
            message(
                &remote_key,
                NETWORK,
                CONVERSATION,
                SERVICE,
                conversation_recipient("another-conversation"),
            ),
        );
        let direct_recipient = sign_message(
            &remote_key,
            message(
                &remote_key,
                NETWORK,
                CONVERSATION,
                SERVICE,
                CollaborationRecipient {
                    kind: CollaborationRecipientKind::Profile,
                    id: crate::crypto::encode_did_key(&remote_key.verifying_key()),
                },
            ),
        );
        let mut expired_message = message(
            &remote_key,
            NETWORK,
            CONVERSATION,
            SERVICE,
            conversation_recipient(CONVERSATION),
        );
        expired_message.created_at = NOW - 300;
        expired_message.expires_at = NOW;
        let expired = sign_message(&remote_key, expired_message);
        for invalid in [
            wrong_network,
            wrong_conversation,
            wrong_service,
            wrong_recipient,
            direct_recipient,
            expired,
        ] {
            assert!(authority
                .authorize_incoming(
                    &invalid,
                    &crate::crypto::encode_did_key(&remote_key.verifying_key()),
                    NOW,
                )
                .is_err());
        }

        let mut tampered: SignedCollaborationMessage = serde_json::from_slice(&exact).unwrap();
        tampered.payload.payload["content"] = serde_json::json!("tampered");
        assert!(authority
            .authorize_incoming(
                &canonical_signed_collaboration_message_bytes(&tampered).unwrap(),
                &crate::crypto::encode_did_key(&remote_key.verifying_key()),
                NOW,
            )
            .is_err());
        let mut noncanonical = exact.clone();
        noncanonical.push(b'\n');
        assert!(authority
            .authorize_incoming(
                &noncanonical,
                &crate::crypto::encode_did_key(&remote_key.verifying_key()),
                NOW,
            )
            .is_err());
        assert!(authority
            .authorize_incoming(
                &vec![0; MAX_COLLABORATION_ENVELOPE_BYTES + 1],
                &crate::crypto::encode_did_key(&remote_key.verifying_key()),
                NOW,
            )
            .is_err());

        let (substituted_endpoint, _) = generate_keypair();
        assert!(authority
            .authorize_incoming(
                &exact,
                &crate::crypto::encode_did_key(&substituted_endpoint.verifying_key()),
                NOW,
            )
            .is_err());
    }

    #[test]
    fn construction_requires_the_exact_profile_authenticated_grant() {
        let (profile_signer, _) = generate_keypair();
        let other_grant_bytes =
            canonical_default_conversation_grant_bytes(&grant("another-network")).unwrap();
        let other_profile = verified_profile(
            &profile_signer,
            "another-network",
            1,
            None,
            Some(raw_sha256_cid(&other_grant_bytes)),
        );
        let other_grant =
            verify_default_conversation_grant(&other_profile, &other_grant_bytes).unwrap();

        let local_grant_bytes =
            canonical_default_conversation_grant_bytes(&grant(NETWORK)).unwrap();
        let local_grant_cid = raw_sha256_cid(&local_grant_bytes);
        let local_profile = verified_profile(
            &profile_signer,
            NETWORK,
            1,
            None,
            Some(local_grant_cid.clone()),
        );
        let local_grant =
            verify_default_conversation_grant(&local_profile, &local_grant_bytes).unwrap();
        let different_cid_profile = verified_profile(
            &profile_signer,
            NETWORK,
            1,
            None,
            Some(raw_sha256_cid(b"different default conversation grant")),
        );
        let missing_descriptor_profile = verified_profile(&profile_signer, NETWORK, 1, None, None);
        let revised_matching_profile = verified_profile(
            &profile_signer,
            NETWORK,
            2,
            Some(&local_profile),
            Some(local_grant_cid),
        );
        let (device_key, _) = generate_keypair();
        let (different_cid_device_key, _) = generate_keypair();
        let (missing_descriptor_device_key, _) = generate_keypair();
        let (revised_device_key, _) = generate_keypair();

        assert!(
            DefaultConversationDeviceAuthority::new(device_key, local_profile, other_grant)
                .is_err()
        );
        assert!(DefaultConversationDeviceAuthority::new(
            different_cid_device_key,
            different_cid_profile,
            local_grant.clone(),
        )
        .is_err());
        assert!(DefaultConversationDeviceAuthority::new(
            missing_descriptor_device_key,
            missing_descriptor_profile,
            local_grant.clone(),
        )
        .is_err());
        assert!(DefaultConversationDeviceAuthority::new(
            revised_device_key,
            revised_matching_profile,
            local_grant,
        )
        .is_ok());
    }

    #[test]
    fn crate_private_api_accepts_no_caller_selected_authority_fields() {
        let _prepare: fn(
            &DefaultConversationDeviceAuthority,
            &str,
            &str,
            serde_json::Value,
            u64,
            u64,
        ) -> anyhow::Result<PreparedDefaultConversationMessage> =
            DefaultConversationDeviceAuthority::prepare_outgoing;
        let _incoming: fn(
            &DefaultConversationDeviceAuthority,
            &[u8],
            &str,
            u64,
        ) -> anyhow::Result<AuthorizedDefaultConversationMessage> =
            DefaultConversationDeviceAuthority::authorize_incoming;
    }
}
