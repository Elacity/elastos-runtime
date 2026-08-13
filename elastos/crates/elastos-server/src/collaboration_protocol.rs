//! Pure verification for signed collaboration messages and acceptance receipts.

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    canonical_signed_collaboration_acceptance_receipt_bytes,
    canonical_signed_collaboration_message_bytes, collaboration_message_envelope_sha256,
    CollaborationAcceptanceReceipt, CollaborationMessage, CollaborationRecipientKind,
    SignedCollaborationAcceptanceReceipt, SignedCollaborationMessage,
    COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1,
    COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1, COLLABORATION_MESSAGE_SCHEMA_V1,
    COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1, MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES,
    MAX_COLLABORATION_CLOCK_SKEW_SECS, MAX_COLLABORATION_ENVELOPE_BYTES,
    MAX_COLLABORATION_ID_BYTES, MAX_COLLABORATION_MESSAGE_LIFETIME_SECS,
    MAX_COLLABORATION_PAYLOAD_BYTES, MAX_COLLABORATION_PAYLOAD_TYPE_BYTES,
    MAX_COLLABORATION_SERVICE_BYTES,
};
use elastos_runtime::signature::SigningKey;

use crate::crypto::{
    decode_did_key, domain_separated_sign, encode_signing_key_did,
    verify_domain_separated_signature, verify_signed_json_envelope_against_dids,
};

pub(crate) const COLLABORATION_TRANSPORT_FRAME_SCHEMA_V1: &str =
    "elastos.collaboration.transport-frame/v1";
const COLLABORATION_TRANSPORT_FRAME_SIGNATURE_DOMAIN_V1: &str =
    "elastos.collaboration.transport-frame.sig/v1";
pub(crate) const MAX_COLLABORATION_TRANSPORT_FRAME_BYTES: usize =
    MAX_COLLABORATION_ENVELOPE_BYTES + 2 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationMessage {
    envelope: SignedCollaborationMessage,
    envelope_sha256: String,
}

impl VerifiedCollaborationMessage {
    pub fn envelope(&self) -> &SignedCollaborationMessage {
        &self.envelope
    }

    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationAcceptanceReceipt {
    envelope: SignedCollaborationAcceptanceReceipt,
}

impl VerifiedCollaborationAcceptanceReceipt {
    pub fn envelope(&self) -> &SignedCollaborationAcceptanceReceipt {
        &self.envelope
    }

    pub(crate) fn accepting_endpoint_did(&self) -> &str {
        &self.envelope.payload.recipient_endpoint_did
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedCollaborationTransportFrame {
    envelope_bytes: Vec<u8>,
    source_endpoint_did: String,
}

impl VerifiedCollaborationTransportFrame {
    pub(crate) fn envelope_bytes(&self) -> &[u8] {
        &self.envelope_bytes
    }

    pub(crate) fn source_endpoint_did(&self) -> &str {
        &self.source_endpoint_did
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CollaborationTransportFramePayload {
    schema: String,
    source_endpoint_did: String,
    envelope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedCollaborationTransportFrame {
    payload: CollaborationTransportFramePayload,
    signature: String,
    signer_did: String,
}

/// Returns the peer endpoint authenticated by the receiving Carrier
/// connection. The sending provider must place `null` on the wire; only the
/// receiving Runtime may replace it with this exact internal fact.
pub(crate) fn authenticated_carrier_source_endpoint(
    value: Option<&serde_json::Value>,
) -> anyhow::Result<String> {
    let carrier = value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("authenticated Carrier source endpoint is missing"))?;
    if carrier.len() != 1 {
        anyhow::bail!("authenticated Carrier source endpoint metadata is invalid");
    }
    let source_endpoint_did = carrier
        .get("source_endpoint_did")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("authenticated Carrier source endpoint is missing"))?;
    validate_canonical_did(
        source_endpoint_did,
        "authenticated Carrier source endpoint DID",
    )?;
    Ok(source_endpoint_did.to_string())
}

pub(crate) fn sign_collaboration_transport_frame(
    signing_key: &SigningKey,
    envelope_bytes: &[u8],
) -> anyhow::Result<Vec<u8>> {
    validate_transport_envelope_bytes(envelope_bytes)?;
    let payload = CollaborationTransportFramePayload {
        schema: COLLABORATION_TRANSPORT_FRAME_SCHEMA_V1.to_string(),
        source_endpoint_did: encode_signing_key_did(signing_key),
        envelope: std::str::from_utf8(envelope_bytes)
            .context("collaboration transport frame envelope is not UTF-8")?
            .to_string(),
    };
    let canonical_payload = serde_json::to_string(&payload)
        .context("collaboration transport frame is not canonical")?;
    let (signature, signer_did) = domain_separated_sign(
        signing_key,
        COLLABORATION_TRANSPORT_FRAME_SIGNATURE_DOMAIN_V1,
        canonical_payload.as_bytes(),
    );
    let frame = SignedCollaborationTransportFrame {
        payload,
        signature,
        signer_did,
    };
    let frame_bytes =
        serde_json::to_vec(&frame).context("collaboration transport frame serialization failed")?;
    verify_collaboration_transport_frame(&frame_bytes)?;
    Ok(frame_bytes)
}

pub(crate) fn verify_collaboration_transport_frame(
    frame_bytes: &[u8],
) -> anyhow::Result<VerifiedCollaborationTransportFrame> {
    if frame_bytes.is_empty() || frame_bytes.len() > MAX_COLLABORATION_TRANSPORT_FRAME_BYTES {
        anyhow::bail!("collaboration transport frame exceeds the byte limit");
    }
    let envelope: SignedCollaborationTransportFrame =
        serde_json::from_slice(frame_bytes).context("invalid collaboration transport frame")?;
    let canonical_envelope =
        serde_json::to_vec(&envelope).context("collaboration transport frame is not canonical")?;
    if canonical_envelope != frame_bytes {
        anyhow::bail!("collaboration transport frame is not canonical JSON");
    }
    if envelope.payload.schema != COLLABORATION_TRANSPORT_FRAME_SCHEMA_V1 {
        anyhow::bail!("unsupported collaboration transport frame schema");
    }
    if envelope.signer_did != envelope.payload.source_endpoint_did {
        anyhow::bail!("collaboration transport frame signer does not match source endpoint DID");
    }
    validate_canonical_did(
        &envelope.payload.source_endpoint_did,
        "collaboration transport frame source endpoint DID",
    )?;
    let canonical_payload = serde_json::to_string(&envelope.payload)
        .context("collaboration transport frame payload is not canonical")?;
    verify_domain_separated_signature(
        &envelope.payload.source_endpoint_did,
        COLLABORATION_TRANSPORT_FRAME_SIGNATURE_DOMAIN_V1,
        canonical_payload.as_bytes(),
        &envelope.signature,
    )?;
    let envelope_bytes = envelope.payload.envelope.into_bytes();
    validate_transport_envelope_bytes(&envelope_bytes)?;
    Ok(VerifiedCollaborationTransportFrame {
        envelope_bytes,
        source_endpoint_did: envelope.payload.source_endpoint_did,
    })
}

/// Verify message authorship, canonical encoding, network binding, and lifetime.
///
/// The caller supplies a verified network profile and the Runtime-bound sender
/// service. Success does not establish contact, membership, permission,
/// transport, or delivery.
pub fn verify_collaboration_message(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    expected_sender_service: &str,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationMessage> {
    let verified = verify_stored_collaboration_message(
        envelope_bytes,
        network_profile,
        expected_sender_service,
    )?;
    validate_message_admissibility(&verified.envelope.payload, now)?;
    Ok(verified)
}

/// Reverify canonical stored bytes without making wall-clock expiry a storage format error.
pub(crate) fn verify_stored_collaboration_message(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    expected_sender_service: &str,
) -> anyhow::Result<VerifiedCollaborationMessage> {
    validate_envelope_size(envelope_bytes)?;
    let expected_network_id = network_profile.profile().network_id.as_str();
    validate_service(expected_sender_service)?;

    let envelope: SignedCollaborationMessage =
        serde_json::from_slice(envelope_bytes).context("invalid collaboration message envelope")?;
    let canonical_envelope = canonical_signed_collaboration_message_bytes(&envelope)?;
    if canonical_envelope != envelope_bytes {
        anyhow::bail!("collaboration message envelope is not canonical JSON");
    }
    validate_signature_shape(&envelope.signature)?;
    validate_message(
        &envelope.payload,
        expected_network_id,
        expected_sender_service,
    )?;
    validate_canonical_did(&envelope.signer_did, "collaboration message signer")?;

    verify_signed_json_envelope_against_dids(
        envelope_bytes,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        std::slice::from_ref(&envelope.signer_did),
    )?;

    Ok(VerifiedCollaborationMessage {
        envelope,
        envelope_sha256: collaboration_message_envelope_sha256(&canonical_envelope),
    })
}

/// Verify a Runtime endpoint's signed durable acceptance of one exact envelope.
///
/// Success proves only that the signing endpoint durably accepted the exact
/// verified envelope. It is not a person-level delivery/read receipt and does
/// not prove Chat display, reading, contact, membership, or permission.
pub fn verify_collaboration_acceptance_receipt(
    envelope_bytes: &[u8],
    message: &VerifiedCollaborationMessage,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt> {
    let verified = verify_stored_collaboration_acceptance_receipt(envelope_bytes, message)?;
    if verified.envelope.payload.accepted_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
    {
        anyhow::bail!("collaboration acceptance receipt acceptance time is invalid");
    }
    Ok(verified)
}

/// Reverify a stored exact-message acceptance without applying current wall-clock expiry.
pub(crate) fn verify_stored_collaboration_acceptance_receipt(
    envelope_bytes: &[u8],
    message: &VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt> {
    let verified = verify_stored_acceptance_receipt_envelope(envelope_bytes)?;
    validate_receipt_against_message(&verified.envelope.payload, message)?;
    Ok(verified)
}

/// Reverify a compact stored acceptance tombstone from its canonical signed bytes.
pub(crate) fn verify_stored_acceptance_receipt_envelope(
    envelope_bytes: &[u8],
) -> anyhow::Result<VerifiedCollaborationAcceptanceReceipt> {
    if envelope_bytes.is_empty()
        || envelope_bytes.len() > MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES
    {
        anyhow::bail!("collaboration acceptance receipt exceeds the byte limit");
    }
    let envelope: SignedCollaborationAcceptanceReceipt = serde_json::from_slice(envelope_bytes)
        .context("invalid collaboration acceptance receipt envelope")?;
    let canonical_envelope = canonical_signed_collaboration_acceptance_receipt_bytes(&envelope)?;
    if canonical_envelope != envelope_bytes {
        anyhow::bail!("collaboration acceptance receipt envelope is not canonical JSON");
    }
    validate_signature_shape(&envelope.signature)?;
    validate_receipt_shape(&envelope.payload)?;
    if envelope.signer_did != envelope.payload.recipient_endpoint_did {
        anyhow::bail!(
            "collaboration acceptance receipt signer does not match recipient endpoint DID"
        );
    }
    validate_canonical_did(
        &envelope.signer_did,
        "collaboration acceptance receipt signer",
    )?;
    verify_signed_json_envelope_against_dids(
        envelope_bytes,
        COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
        std::slice::from_ref(&envelope.signer_did),
    )?;
    Ok(VerifiedCollaborationAcceptanceReceipt { envelope })
}

fn validate_message(
    message: &CollaborationMessage,
    expected_network_id: &str,
    expected_sender_service: &str,
) -> anyhow::Result<()> {
    if message.schema != COLLABORATION_MESSAGE_SCHEMA_V1 {
        anyhow::bail!("unsupported collaboration message schema");
    }
    crate::collaboration_network::validate_network_id(&message.network_id)?;
    if message.network_id != expected_network_id {
        anyhow::bail!("collaboration message belongs to another network");
    }
    validate_id(&message.conversation_id, "conversation_id")?;
    validate_strong_id(&message.message_id, "message_id")?;
    validate_strong_id(&message.nonce, "nonce")?;
    validate_message_lifetime(message.created_at, message.expires_at)?;
    validate_canonical_did(&message.sender_profile_did, "sender Profile DID")?;
    validate_service(&message.sender_service)?;
    if message.sender_service != expected_sender_service {
        anyhow::bail!("collaboration message sender service does not match Runtime context");
    }
    validate_recipient(message)?;
    validate_payload_type(&message.payload_type)?;
    if serde_json::to_vec(&message.payload)?.len() > MAX_COLLABORATION_PAYLOAD_BYTES {
        anyhow::bail!("collaboration message payload is too large");
    }
    Ok(())
}

fn validate_receipt_shape(receipt: &CollaborationAcceptanceReceipt) -> anyhow::Result<()> {
    if receipt.schema != COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1 {
        anyhow::bail!("unsupported collaboration acceptance receipt schema");
    }
    crate::collaboration_network::validate_network_id(&receipt.network_id)?;
    validate_sha256_label(&receipt.message_envelope_sha256)?;
    validate_id(
        &receipt.conversation_id,
        "acceptance receipt conversation_id",
    )?;
    validate_canonical_did(
        &receipt.sender_profile_did,
        "acceptance receipt sender Profile DID",
    )?;
    validate_strong_id(&receipt.message_id, "acceptance receipt message_id")?;
    validate_strong_id(&receipt.message_nonce, "acceptance receipt message nonce")?;
    validate_canonical_did(
        &receipt.recipient_endpoint_did,
        "acceptance receipt recipient endpoint DID",
    )?;
    if receipt.accepted_at == 0 {
        anyhow::bail!("collaboration acceptance receipt acceptance time is invalid");
    }
    Ok(())
}

fn validate_receipt_against_message(
    receipt: &CollaborationAcceptanceReceipt,
    message: &VerifiedCollaborationMessage,
) -> anyhow::Result<()> {
    if receipt.network_id != message.envelope.payload.network_id {
        anyhow::bail!("collaboration acceptance receipt belongs to another network");
    }
    if receipt.message_envelope_sha256 != message.envelope_sha256 {
        anyhow::bail!("collaboration acceptance receipt message hash mismatch");
    }
    if receipt.conversation_id != message.envelope.payload.conversation_id {
        anyhow::bail!("collaboration acceptance receipt conversation mismatch");
    }
    if receipt.message_id != message.envelope.payload.message_id {
        anyhow::bail!("collaboration acceptance receipt message_id mismatch");
    }
    if receipt.sender_profile_did != message.envelope.payload.sender_profile_did {
        anyhow::bail!("collaboration acceptance receipt sender Profile DID mismatch");
    }
    if receipt.message_nonce != message.envelope.payload.nonce {
        anyhow::bail!("collaboration acceptance receipt message nonce mismatch");
    }
    if receipt
        .accepted_at
        .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
        < message.envelope.payload.created_at
        || receipt.accepted_at
            > message
                .envelope
                .payload
                .expires_at
                .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
    {
        anyhow::bail!("collaboration acceptance receipt acceptance time is invalid");
    }
    Ok(())
}

fn validate_transport_envelope_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_COLLABORATION_ENVELOPE_BYTES {
        anyhow::bail!("collaboration transport frame envelope exceeds the byte limit");
    }
    std::str::from_utf8(bytes).context("collaboration transport frame envelope is not UTF-8")?;
    Ok(())
}

fn validate_message_lifetime(created_at: u64, expires_at: u64) -> anyhow::Result<()> {
    if expires_at <= created_at {
        anyhow::bail!("collaboration message lifetime must be greater than zero");
    }
    if expires_at - created_at > MAX_COLLABORATION_MESSAGE_LIFETIME_SECS {
        anyhow::bail!("collaboration message lifetime is too long");
    }
    Ok(())
}

fn validate_message_admissibility(message: &CollaborationMessage, now: u64) -> anyhow::Result<()> {
    if message.created_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) {
        anyhow::bail!("collaboration message is not yet valid");
    }
    if message.expires_at <= now {
        anyhow::bail!("collaboration message is expired");
    }
    Ok(())
}

fn validate_recipient(message: &CollaborationMessage) -> anyhow::Result<()> {
    match message.recipient.kind {
        CollaborationRecipientKind::Profile => {
            validate_canonical_did(&message.recipient.id, "recipient Profile DID")
        }
        CollaborationRecipientKind::Conversation => {
            validate_id(&message.recipient.id, "recipient conversation_id")?;
            if message.recipient.id != message.conversation_id {
                anyhow::bail!("collaboration message has conflicting conversation recipients");
            }
            Ok(())
        }
    }
}

pub(crate) fn validate_id(value: &str, field: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > MAX_COLLABORATION_ID_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b':')
        })
    {
        anyhow::bail!("collaboration {field} is not a canonical identifier");
    }
    Ok(())
}

fn validate_strong_id(value: &str, field: &str) -> anyhow::Result<()> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("collaboration {field} must be 128-bit lowercase hex");
    }
    Ok(())
}

pub(crate) fn validate_service(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > MAX_COLLABORATION_SERVICE_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        anyhow::bail!("collaboration sender service is not canonical");
    }
    Ok(())
}

pub(crate) fn validate_payload_type(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > MAX_COLLABORATION_PAYLOAD_TYPE_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/')
        })
    {
        anyhow::bail!("collaboration payload_type is not canonical");
    }
    Ok(())
}

fn validate_canonical_did(value: &str, field: &str) -> anyhow::Result<()> {
    decode_did_key(value)
        .with_context(|| format!("invalid {field}"))
        .map(|_| ())
}

fn validate_signature_shape(signature: &str) -> anyhow::Result<()> {
    if signature.len() != 128
        || !signature
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("collaboration signature must be lowercase Ed25519 hex");
    }
    Ok(())
}

fn validate_sha256_label(value: &str) -> anyhow::Result<()> {
    if !value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        anyhow::bail!("collaboration message envelope hash is not canonical SHA-256");
    }
    Ok(())
}

fn validate_envelope_size(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_COLLABORATION_ENVELOPE_BYTES {
        anyhow::bail!("collaboration signed envelope has an invalid byte length");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, SignedCollaborationNetworkProfile,
        VerifiedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use elastos_common::collaboration_protocol::{
        canonical_collaboration_acceptance_receipt_bytes, canonical_collaboration_message_bytes,
        CollaborationRecipient, SignedCollaborationAcceptanceReceipt,
    };
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    const NOW: u64 = 1_800_000_000;
    const NETWORK: &str = "elastos-collaboration-test";
    const SERVICE: &str = "chat";

    fn did(signing_key: &SigningKey) -> String {
        encode_signing_key_did(signing_key)
    }

    fn verified_network_profile(network_id: &str) -> VerifiedCollaborationNetworkProfile {
        let (signing_key, _) = generate_keypair();
        let signer_did = did(&signing_key);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: None,
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            &signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        assert_eq!(envelope_signer, signer_did);
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did: envelope_signer.clone(),
        };
        let envelope_bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        match validate_collaboration_network_profile(
            Some(&envelope_bytes),
            network_id,
            &[envelope_signer],
            None,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured network"),
        }
    }

    fn verify_collaboration_message(
        envelope_bytes: &[u8],
        network_id: &str,
        expected_sender_service: &str,
        now: u64,
    ) -> anyhow::Result<VerifiedCollaborationMessage> {
        super::verify_collaboration_message(
            envelope_bytes,
            &verified_network_profile(network_id),
            expected_sender_service,
            now,
        )
    }

    fn message(sender: &SigningKey, recipient_did: &str) -> CollaborationMessage {
        CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: "conversation-1".to_string(),
            message_id: "0123456789abcdef0123456789abcdef".to_string(),
            nonce: "abcdef0123456789abcdef0123456789".to_string(),
            created_at: NOW,
            expires_at: NOW + 300,
            sender_profile_did: profile_did_for_endpoint(sender),
            sender_service: SERVICE.to_string(),
            recipient: CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: recipient_did.to_string(),
            },
            payload_type: "elastos.chat.message/v1".to_string(),
            payload: serde_json::json!({"content":"hello"}),
        }
    }

    fn profile_did_for_endpoint(endpoint: &SigningKey) -> String {
        let mut seed = endpoint.to_bytes();
        seed[0] ^= 0xA5;
        seed[31] ^= 0x5A;
        did(&SigningKey::from_bytes(&seed))
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

    fn receipt(
        recipient: &SigningKey,
        message: &VerifiedCollaborationMessage,
    ) -> CollaborationAcceptanceReceipt {
        CollaborationAcceptanceReceipt {
            schema: COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            message_envelope_sha256: message.envelope_sha256.clone(),
            conversation_id: message.envelope.payload.conversation_id.clone(),
            sender_profile_did: message.envelope.payload.sender_profile_did.clone(),
            message_id: message.envelope.payload.message_id.clone(),
            message_nonce: message.envelope.payload.nonce.clone(),
            recipient_endpoint_did: did(recipient),
            accepted_at: NOW + 1,
        }
    }

    fn sign_receipt(recipient: &SigningKey, payload: CollaborationAcceptanceReceipt) -> Vec<u8> {
        let payload_bytes = canonical_collaboration_acceptance_receipt_bytes(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            recipient,
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

    fn fixture() -> (
        SigningKey,
        SigningKey,
        Vec<u8>,
        VerifiedCollaborationMessage,
    ) {
        let (sender, _) = generate_keypair();
        let (recipient, _) = generate_keypair();
        let bytes = sign_message(
            &sender,
            message(&sender, &profile_did_for_endpoint(&recipient)),
        );
        let verified = verify_collaboration_message(&bytes, NETWORK, SERVICE, NOW).unwrap();
        (sender, recipient, bytes, verified)
    }

    #[test]
    fn transport_frame_verification_requires_exact_source_endpoint_signer_and_canonical_envelope() {
        let (endpoint, _) = generate_keypair();
        let (recipient, _) = generate_keypair();
        let message_bytes = sign_message(
            &endpoint,
            message(&endpoint, &profile_did_for_endpoint(&recipient)),
        );
        let frame = super::sign_collaboration_transport_frame(&endpoint, &message_bytes).unwrap();
        let verified = super::verify_collaboration_transport_frame(&frame).unwrap();
        assert_eq!(verified.envelope_bytes(), message_bytes);
        assert_eq!(verified.source_endpoint_did(), did(&endpoint));

        let mut substituted: SignedCollaborationTransportFrame =
            serde_json::from_slice(&frame).unwrap();
        let (other, _) = generate_keypair();
        substituted.payload.source_endpoint_did = did(&other);
        let substituted = serde_json::to_vec(&substituted).unwrap();
        assert!(super::verify_collaboration_transport_frame(&substituted).is_err());

        let mut unknown_field: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        unknown_field["unexpected"] = serde_json::json!(true);
        let unknown_field = serde_json::to_vec(&unknown_field).unwrap();
        assert!(super::verify_collaboration_transport_frame(&unknown_field).is_err());

        let mut noncanonical: SignedCollaborationTransportFrame =
            serde_json::from_slice(&frame).unwrap();
        noncanonical.payload.envelope.push('\n');
        let noncanonical = serde_json::to_vec(&noncanonical).unwrap();
        assert!(super::verify_collaboration_transport_frame(&noncanonical).is_err());
    }

    #[test]
    fn message_verification_requires_the_configured_verified_network() {
        let (sender, _) = generate_keypair();
        let (recipient, _) = generate_keypair();
        let bytes = sign_message(
            &sender,
            message(&sender, &profile_did_for_endpoint(&recipient)),
        );
        let configured = verified_network_profile(NETWORK);
        let other_network = verified_network_profile("another-network");

        assert!(super::verify_collaboration_message(&bytes, &configured, SERVICE, NOW).is_ok());
        assert!(
            super::verify_collaboration_message(&bytes, &other_network, SERVICE, NOW)
                .unwrap_err()
                .to_string()
                .contains("another network")
        );

        let isolated = validate_collaboration_network_profile(None, NETWORK, &[], None).unwrap();
        assert_eq!(isolated, CollaborationNetworkProfileMode::Isolated);
    }

    #[test]
    fn verifies_message_and_exact_acceptance_receipt_deterministically() {
        let (_, recipient, message_bytes, verified_message) = fixture();
        let receipt_bytes = sign_receipt(&recipient, receipt(&recipient, &verified_message));
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("unchanged");
        std::fs::write(&marker, b"original").unwrap();

        let message_again =
            verify_collaboration_message(&message_bytes, NETWORK, SERVICE, NOW).unwrap();
        let first =
            verify_collaboration_acceptance_receipt(&receipt_bytes, &verified_message, NOW + 1)
                .unwrap();
        let second =
            verify_collaboration_acceptance_receipt(&receipt_bytes, &message_again, NOW + 1)
                .unwrap();

        assert_eq!(verified_message, message_again);
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&marker).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(marker_dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_wrong_schema_network_signer_sender_service_and_substitutions() {
        let (sender, recipient, bytes, _) = fixture();
        let mut envelope: SignedCollaborationMessage = serde_json::from_slice(&bytes).unwrap();

        let mut wrong_schema = envelope.payload.clone();
        wrong_schema.schema = "elastos.collaboration.message/v2".to_string();
        assert!(verify_collaboration_message(
            &sign_message(&sender, wrong_schema),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err()
        .to_string()
        .contains("schema"));

        let mut wrong_network = envelope.payload.clone();
        wrong_network.network_id = "another-network".to_string();
        assert!(verify_collaboration_message(
            &sign_message(&sender, wrong_network),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err()
        .to_string()
        .contains("another network"));

        envelope.signer_did = did(&recipient);
        let wrong_signer = canonical_signed_collaboration_message_bytes(&envelope).unwrap();
        assert!(
            verify_collaboration_message(&wrong_signer, NETWORK, SERVICE, NOW)
                .unwrap_err()
                .to_string()
                .contains("signature")
        );

        let original: SignedCollaborationMessage = serde_json::from_slice(&bytes).unwrap();
        let mut sender_substitution = original.clone();
        sender_substitution.payload.sender_profile_did = did(&recipient);
        let sender_substitution =
            canonical_signed_collaboration_message_bytes(&sender_substitution).unwrap();
        assert!(verify_collaboration_message(&sender_substitution, NETWORK, SERVICE, NOW).is_err());

        assert!(verify_collaboration_message(&bytes, NETWORK, "people", NOW)
            .unwrap_err()
            .to_string()
            .contains("Runtime context"));

        let mut recipient_substitution = original;
        recipient_substitution.payload.recipient.id = did(&sender);
        let recipient_substitution =
            canonical_signed_collaboration_message_bytes(&recipient_substitution).unwrap();
        assert!(
            verify_collaboration_message(&recipient_substitution, NETWORK, SERVICE, NOW).is_err()
        );
    }

    #[test]
    fn rejects_tampering_noncanonical_json_unknown_fields_and_ambiguous_recipient() {
        let (_, _, bytes, _) = fixture();
        let mut tampered: SignedCollaborationMessage = serde_json::from_slice(&bytes).unwrap();
        tampered.payload.payload["content"] = serde_json::json!("changed");
        assert!(verify_collaboration_message(
            &canonical_signed_collaboration_message_bytes(&tampered).unwrap(),
            NETWORK,
            SERVICE,
            NOW,
        )
        .is_err());

        let mut noncanonical = bytes.clone();
        noncanonical.push(b'\n');
        assert!(
            verify_collaboration_message(&noncanonical, NETWORK, SERVICE, NOW)
                .unwrap_err()
                .to_string()
                .contains("not canonical")
        );

        let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        unknown["payload"]["transport"] = serde_json::json!("forbidden");
        let err = verify_collaboration_message(
            &serde_json::to_vec(&unknown).unwrap(),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));

        let ambiguous = String::from_utf8(bytes)
            .unwrap()
            .replace("\"id\":\"", "\"id\":\"duplicate\",\"id\":\"");
        let err =
            verify_collaboration_message(ambiguous.as_bytes(), NETWORK, SERVICE, NOW).unwrap_err();
        assert!(format!("{err:#}").contains("duplicate field"));
    }

    #[test]
    fn rejects_invalid_message_time_windows() {
        let (sender, _) = generate_keypair();
        let (recipient, _) = generate_keypair();
        for (created_at, expires_at, expected) in [
            (NOW - 301, NOW, "expired"),
            (
                NOW + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1,
                NOW + MAX_COLLABORATION_CLOCK_SKEW_SECS + 301,
                "not yet valid",
            ),
            (NOW, NOW, "greater than zero"),
            (
                NOW,
                NOW + MAX_COLLABORATION_MESSAGE_LIFETIME_SECS + 1,
                "too long",
            ),
        ] {
            let mut candidate = message(&sender, &profile_did_for_endpoint(&recipient));
            candidate.created_at = created_at;
            candidate.expires_at = expires_at;
            let err = verify_collaboration_message(
                &sign_message(&sender, candidate),
                NETWORK,
                SERVICE,
                NOW,
            )
            .unwrap_err();
            assert!(err.to_string().contains(expected), "{err:#}");
        }
    }

    #[test]
    fn rejects_oversized_fields_payload_invalid_dids_and_conflicting_recipient() {
        let (sender, _) = generate_keypair();
        let (recipient, _) = generate_keypair();
        let base = message(&sender, &profile_did_for_endpoint(&recipient));

        let mut oversized_field = base.clone();
        oversized_field.conversation_id = "a".repeat(MAX_COLLABORATION_ID_BYTES + 1);
        assert!(verify_collaboration_message(
            &sign_message(&sender, oversized_field),
            NETWORK,
            SERVICE,
            NOW,
        )
        .is_err());

        let mut oversized_payload = base.clone();
        oversized_payload.payload =
            serde_json::json!({"content":"a".repeat(MAX_COLLABORATION_PAYLOAD_BYTES)});
        assert!(verify_collaboration_message(
            &sign_message(&sender, oversized_payload),
            NETWORK,
            SERVICE,
            NOW,
        )
        .is_err());

        let mut invalid_did = base.clone();
        invalid_did.sender_profile_did = "did:key:invalid".to_string();
        assert!(verify_collaboration_message(
            &sign_message(&sender, invalid_did),
            NETWORK,
            SERVICE,
            NOW,
        )
        .is_err());

        let mut weak_message_id = base.clone();
        weak_message_id.message_id = "message-1".to_string();
        assert!(verify_collaboration_message(
            &sign_message(&sender, weak_message_id),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err()
        .to_string()
        .contains("128-bit"));

        let mut weak_nonce = base.clone();
        weak_nonce.nonce = "nonce-1".to_string();
        assert!(verify_collaboration_message(
            &sign_message(&sender, weak_nonce),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err()
        .to_string()
        .contains("128-bit"));

        let mut conflicting = base;
        conflicting.recipient = CollaborationRecipient {
            kind: CollaborationRecipientKind::Conversation,
            id: "another-conversation".to_string(),
        };
        assert!(verify_collaboration_message(
            &sign_message(&sender, conflicting),
            NETWORK,
            SERVICE,
            NOW,
        )
        .unwrap_err()
        .to_string()
        .contains("conflicting"));
    }

    #[test]
    fn receipt_is_bound_to_exact_message_and_its_own_accepting_endpoint() {
        let (_, recipient, _, verified_message) = fixture();
        let base = receipt(&recipient, &verified_message);
        for (candidate, expected) in [
            (
                CollaborationAcceptanceReceipt {
                    schema: "elastos.collaboration.acceptance-receipt/v2".to_string(),
                    ..base.clone()
                },
                "schema",
            ),
            (
                CollaborationAcceptanceReceipt {
                    network_id: "another-network".to_string(),
                    ..base.clone()
                },
                "another network",
            ),
            (
                CollaborationAcceptanceReceipt {
                    message_envelope_sha256: format!("sha256:{}", "0".repeat(64)),
                    ..base.clone()
                },
                "hash mismatch",
            ),
            (
                CollaborationAcceptanceReceipt {
                    conversation_id: "another-conversation".to_string(),
                    ..base.clone()
                },
                "conversation mismatch",
            ),
            (
                CollaborationAcceptanceReceipt {
                    message_id: "fedcba9876543210fedcba9876543210".to_string(),
                    ..base.clone()
                },
                "message_id mismatch",
            ),
            (
                CollaborationAcceptanceReceipt {
                    sender_profile_did: did(&recipient),
                    ..base.clone()
                },
                "sender Profile DID mismatch",
            ),
            (
                CollaborationAcceptanceReceipt {
                    message_nonce: "fedcba9876543210fedcba9876543210".to_string(),
                    ..base.clone()
                },
                "message nonce mismatch",
            ),
        ] {
            let err = verify_collaboration_acceptance_receipt(
                &sign_receipt(&recipient, candidate),
                &verified_message,
                NOW + 1,
            )
            .unwrap_err();
            assert!(err.to_string().contains(expected), "{err:#}");
        }

        let (other, _) = generate_keypair();
        let other_endpoint = sign_receipt(&other, receipt(&other, &verified_message));
        let verified_other =
            verify_collaboration_acceptance_receipt(&other_endpoint, &verified_message, NOW + 1)
                .unwrap();
        assert_eq!(verified_other.accepting_endpoint_did(), did(&other));

        let valid_bytes = sign_receipt(&recipient, base.clone());
        let mut wrong_signer: SignedCollaborationAcceptanceReceipt =
            serde_json::from_slice(&valid_bytes).unwrap();
        wrong_signer.signer_did = did(&other);
        let wrong_signer =
            canonical_signed_collaboration_acceptance_receipt_bytes(&wrong_signer).unwrap();
        assert!(
            verify_collaboration_acceptance_receipt(&wrong_signer, &verified_message, NOW + 1,)
                .unwrap_err()
                .to_string()
                .contains("does not match recipient")
        );

        let mut noncanonical = valid_bytes.clone();
        noncanonical.push(b'\n');
        assert!(
            verify_collaboration_acceptance_receipt(&noncanonical, &verified_message, NOW + 1,)
                .unwrap_err()
                .to_string()
                .contains("not canonical")
        );

        let mut unknown: serde_json::Value = serde_json::from_slice(&valid_bytes).unwrap();
        unknown["payload"]["state"] = serde_json::json!("delivered");
        let err = verify_collaboration_acceptance_receipt(
            &serde_json::to_vec(&unknown).unwrap(),
            &verified_message,
            NOW + 1,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));

        for accepted_at in [
            verified_message.envelope.payload.created_at - MAX_COLLABORATION_CLOCK_SKEW_SECS - 1,
            NOW + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1,
        ] {
            let invalid_time = CollaborationAcceptanceReceipt {
                accepted_at,
                ..base.clone()
            };
            assert!(verify_collaboration_acceptance_receipt(
                &sign_receipt(&recipient, invalid_time),
                &verified_message,
                NOW,
            )
            .unwrap_err()
            .to_string()
            .contains("acceptance time"));
        }
    }

    #[test]
    fn acceptance_receipt_has_its_own_exact_wire_bound() {
        let at_limit = vec![b' '; MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES];
        let at_limit_error = verify_stored_acceptance_receipt_envelope(&at_limit).unwrap_err();
        assert!(!at_limit_error.to_string().contains("byte limit"));

        let over_limit = vec![b' '; MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES + 1];
        assert!(verify_stored_acceptance_receipt_envelope(&over_limit)
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
    }

    #[test]
    fn wire_contract_contains_no_runtime_or_carrier_authority_fields() {
        let (_, recipient, message_bytes, verified_message) = fixture();
        let receipt_bytes = sign_receipt(&recipient, receipt(&recipient, &verified_message));
        let serialized = format!(
            "{}{}",
            String::from_utf8(message_bytes).unwrap(),
            String::from_utf8(receipt_bytes).unwrap()
        );
        for forbidden in [
            "principal_id",
            "session",
            "launch_token",
            "connect_ticket",
            "peer_endpoint",
            "carrier_topic",
        ] {
            assert!(!serialized.contains(forbidden), "found {forbidden}");
        }
    }
}
