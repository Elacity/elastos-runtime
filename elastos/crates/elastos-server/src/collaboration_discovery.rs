//! Pure validation for signed collaboration discovery advertisements, contact
//! requests, and receipt-bound contact decisions.

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    CollaborationRecipientKind, MAX_COLLABORATION_CLOCK_SKEW_SECS,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::collaboration_profile_authority::{
    verify_signed_profile_document, SignedCollaborationProfileDocument,
    VerifiedCollaborationProfileDocument,
};
use crate::collaboration_protocol::{
    verify_collaboration_message, verify_stored_collaboration_message, VerifiedCollaborationMessage,
};
use crate::crypto::{decode_did_key, verify_signed_json_envelope_against_dids};

pub const COLLABORATION_DISCOVERY_SERVICE: &str = "people";
pub const COLLABORATION_DISCOVERY_DIRECTORY_ID: &str = "elastos.people.discovery.directory";
pub const COLLABORATION_DISCOVERY_CONTACT_ID: &str = "elastos.people.contact";
pub const COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE: &str =
    "elastos.people.discovery.advertisement/v1";
pub const COLLABORATION_DISCOVERY_WITHDRAWAL_PAYLOAD_TYPE: &str =
    "elastos.people.discovery.withdrawal/v1";
pub const COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE: &str =
    "elastos.people.discovery.mailbox-poll/v1";
pub const COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE: &str =
    "elastos.people.contact-request/v1";
pub const COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS: u64 = 10 * 60;
pub const COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS: u64 = 60 * 60;
pub const COLLABORATION_DISCOVERY_CONTROL_TTL_SECS: u64 = 2 * 60;
pub const COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE: &str =
    "elastos.people.contact-revocation/v1";
/// A revocation rides the direct pair channel and retries like a direct
/// message, so it shares the direct-message lifetime ceiling.
pub const COLLABORATION_CONTACT_REVOCATION_TTL_SECS: u64 = 24 * 60 * 60;
pub const COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1: &str =
    "elastos.people.contact-decision-receipt/v1";
pub const COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1: &str =
    "elastos.people.contact-decision-receipt.v1";
pub const MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationDiscoveryAdvertisementPayload {
    pub signed_profile: SignedCollaborationProfileDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationContactRequestPayload {
    pub advertisement_envelope_sha256: String,
    pub signed_profile: SignedCollaborationProfileDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationContactDecision {
    Accepted,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationContactDecisionReceipt {
    pub schema: String,
    pub network_id: String,
    pub request_envelope_sha256: String,
    pub conversation_id: String,
    pub requester_profile_did: String,
    pub requester_endpoint_did: String,
    pub request_message_id: String,
    pub request_message_nonce: String,
    pub recipient_profile_did: String,
    pub recipient_endpoint_did: String,
    pub decision: CollaborationContactDecision,
    pub decided_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCollaborationContactDecisionReceipt {
    pub payload: CollaborationContactDecisionReceipt,
    pub signature: String,
    pub signer_did: String,
}

/// Pair-scoped end of an accepted relationship. `end_verb` names the verb so
/// the state machine has room for a future silent block without changing the
/// wire shape; today only "remove" is valid, and remove is deliberately
/// disclosed to the other side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationContactRevocationPayload {
    // Field order is alphabetical: the canonical-payload check re-serializes
    // the struct and compares bytes against the JSON value's sorted keys.
    pub end_verb: String,
    pub removed_at: u64,
    pub revoked_profile_did: String,
    pub revoking_profile_did: String,
}

pub const COLLABORATION_CONTACT_END_VERB_REMOVE: &str = "remove";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationContactRevocation {
    message: VerifiedCollaborationMessage,
    payload: CollaborationContactRevocationPayload,
}

impl VerifiedCollaborationContactRevocation {
    pub fn message(&self) -> &VerifiedCollaborationMessage {
        &self.message
    }

    pub fn revoking_profile_did(&self) -> &str {
        &self.payload.revoking_profile_did
    }

    pub fn revoked_profile_did(&self) -> &str {
        &self.payload.revoked_profile_did
    }

    pub fn removed_at(&self) -> u64 {
        self.payload.removed_at
    }
}

/// Verifies the network envelope and the payload shape. Relationship binding —
/// that the pair is a known accepted or removed relationship, that the sender
/// device is authorized by the revoking Profile's current head, and that the
/// conversation is the pair's stable id — is the contact store's authority and
/// happens where that knowledge lives.
pub fn verify_collaboration_contact_revocation(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationContactRevocation> {
    let message = verify_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
        now,
    )?;
    verify_contact_revocation_from_message(message)
}

fn verify_contact_revocation_from_message(
    message: VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationContactRevocation> {
    let envelope = &message.envelope().payload;
    if envelope.payload_type != COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE {
        anyhow::bail!("unsupported collaboration contact revocation payload_type");
    }
    if envelope.recipient.kind != CollaborationRecipientKind::Profile {
        anyhow::bail!("collaboration contact revocation recipient must be a Profile");
    }
    let payload = decode_canonical_payload::<CollaborationContactRevocationPayload>(
        &envelope.payload,
        "collaboration contact revocation",
    )?;
    validate_canonical_did(
        &payload.revoking_profile_did,
        "contact revocation revoking profile DID",
    )?;
    validate_canonical_did(
        &payload.revoked_profile_did,
        "contact revocation revoked profile DID",
    )?;
    if payload.revoking_profile_did == payload.revoked_profile_did {
        anyhow::bail!("collaboration contact revocation cannot target its own profile");
    }
    if payload.end_verb != COLLABORATION_CONTACT_END_VERB_REMOVE {
        anyhow::bail!("unsupported collaboration contact end verb");
    }
    if payload.removed_at > envelope.expires_at {
        anyhow::bail!("collaboration contact revocation removal time is invalid");
    }
    validate_product_ttl(
        envelope.created_at,
        envelope.expires_at,
        COLLABORATION_CONTACT_REVOCATION_TTL_SECS,
        "collaboration contact revocation",
    )?;
    Ok(VerifiedCollaborationContactRevocation { message, payload })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationDiscoveryWithdrawalPayload {
    pub advertisement_envelope_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationDiscoveryMailboxKind {
    Requests,
    Decisions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationDiscoveryMailboxPollPayload {
    pub mailbox_kind: CollaborationDiscoveryMailboxKind,
    pub profile_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationDiscoveryAdvertisement {
    message: VerifiedCollaborationMessage,
    payload: CollaborationDiscoveryAdvertisementPayload,
    profile: VerifiedCollaborationProfileDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationContactRequest {
    message: VerifiedCollaborationMessage,
    payload: CollaborationContactRequestPayload,
    profile: VerifiedCollaborationProfileDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationDiscoveryWithdrawal {
    message: VerifiedCollaborationMessage,
    payload: CollaborationDiscoveryWithdrawalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationDiscoveryMailboxPoll {
    message: VerifiedCollaborationMessage,
    payload: CollaborationDiscoveryMailboxPollPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationContactDecisionReceipt {
    envelope: SignedCollaborationContactDecisionReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedStoredUnboundCollaborationContactRequest {
    message: VerifiedCollaborationMessage,
    payload: CollaborationContactRequestPayload,
    profile: VerifiedCollaborationProfileDocument,
}

impl VerifiedCollaborationDiscoveryAdvertisement {
    pub fn message(&self) -> &VerifiedCollaborationMessage {
        &self.message
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.profile.document().display_name
    }

    pub(crate) fn handle(&self) -> Option<&str> {
        self.profile.document().handle.as_deref()
    }

    pub(crate) fn profile_did(&self) -> &str {
        &self.profile.document().profile_did
    }

    pub(crate) fn profile_revision(&self) -> u64 {
        self.profile.document().revision
    }

    pub(crate) fn signed_profile(&self) -> &SignedCollaborationProfileDocument {
        self.profile.signed_envelope()
    }

    pub(crate) fn profile_envelope_sha256(&self) -> anyhow::Result<String> {
        Ok(sha256_label(&serde_json::to_vec(
            self.profile.signed_envelope(),
        )?))
    }

    pub(crate) fn profile_authorizes_device(&self, device_did: &str) -> bool {
        self.profile.authorizes_endpoint(device_did)
    }

    pub(crate) fn route_endpoint_did(&self) -> anyhow::Result<&str> {
        self.profile.sole_endpoint_did()
    }
}

impl VerifiedCollaborationDiscoveryMailboxPoll {
    pub fn message(&self) -> &VerifiedCollaborationMessage {
        &self.message
    }

    pub(crate) fn mailbox_kind(&self) -> CollaborationDiscoveryMailboxKind {
        self.payload.mailbox_kind
    }

    pub(crate) fn profile_did(&self) -> &str {
        &self.payload.profile_did
    }
}

impl VerifiedCollaborationContactRequest {
    pub fn message(&self) -> &VerifiedCollaborationMessage {
        &self.message
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.profile.document().display_name
    }

    pub(crate) fn handle(&self) -> Option<&str> {
        self.profile.document().handle.as_deref()
    }

    pub(crate) fn advertisement_envelope_sha256(&self) -> &str {
        &self.payload.advertisement_envelope_sha256
    }

    pub(crate) fn requester_profile_did(&self) -> &str {
        &self.profile.document().profile_did
    }

    pub(crate) fn profile_revision(&self) -> u64 {
        self.profile.document().revision
    }

    pub(crate) fn profile_envelope_sha256(&self) -> anyhow::Result<String> {
        Ok(sha256_label(&serde_json::to_vec(
            self.profile.signed_envelope(),
        )?))
    }

    pub(crate) fn signed_profile(&self) -> &SignedCollaborationProfileDocument {
        self.profile.signed_envelope()
    }

    pub(crate) fn profile_authorizes_device(&self, device_did: &str) -> bool {
        self.profile.authorizes_endpoint(device_did)
    }

    pub(crate) fn route_endpoint_did(&self) -> anyhow::Result<&str> {
        self.profile.sole_endpoint_did()
    }
}

impl VerifiedCollaborationContactDecisionReceipt {
    pub fn envelope(&self) -> &SignedCollaborationContactDecisionReceipt {
        &self.envelope
    }
}

impl VerifiedStoredUnboundCollaborationContactRequest {
    pub(crate) fn advertisement_envelope_sha256(&self) -> &str {
        &self.payload.advertisement_envelope_sha256
    }
}

pub fn verify_collaboration_discovery_advertisement(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationDiscoveryAdvertisement> {
    let message = verify_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
        now,
    )?;
    verify_discovery_advertisement_from_message(message)
}

pub fn verify_collaboration_contact_request(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationContactRequest> {
    validate_discovery_advertisement_admissibility(advertisement, now)?;
    let message = verify_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
        now,
    )?;
    let request = verify_contact_request_from_message(message)?;
    bind_contact_request_to_advertisement(advertisement, &request)?;
    validate_contact_request_relative_timing(advertisement, &request)?;
    Ok(request)
}

pub(crate) fn verify_stored_collaboration_contact_revocation(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
) -> anyhow::Result<VerifiedCollaborationContactRevocation> {
    let message = verify_stored_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
    )?;
    verify_contact_revocation_from_message(message)
}

pub(crate) fn verify_stored_collaboration_discovery_advertisement(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
) -> anyhow::Result<VerifiedCollaborationDiscoveryAdvertisement> {
    let message = verify_stored_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
    )?;
    verify_discovery_advertisement_from_message(message)
}

pub(crate) fn verify_stored_unbound_collaboration_contact_request(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
) -> anyhow::Result<VerifiedStoredUnboundCollaborationContactRequest> {
    let message = verify_stored_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
    )?;
    let request = verify_contact_request_from_message(message)?;
    Ok(VerifiedStoredUnboundCollaborationContactRequest {
        message: request.message,
        payload: request.payload,
        profile: request.profile,
    })
}

pub(crate) fn bind_stored_collaboration_contact_request(
    request: VerifiedStoredUnboundCollaborationContactRequest,
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
) -> anyhow::Result<VerifiedCollaborationContactRequest> {
    let request = VerifiedCollaborationContactRequest {
        message: request.message,
        payload: request.payload,
        profile: request.profile,
    };
    bind_contact_request_to_advertisement(advertisement, &request)?;
    validate_contact_request_relative_timing(advertisement, &request)?;
    Ok(request)
}

pub fn verify_collaboration_contact_decision_receipt(
    envelope_bytes: &[u8],
    request: &VerifiedCollaborationContactRequest,
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationContactDecisionReceipt> {
    let verified = verify_stored_collaboration_contact_decision_receipt(
        envelope_bytes,
        request,
        advertisement,
    )?;
    if verified.envelope.payload.decided_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
    {
        anyhow::bail!("collaboration contact decision receipt time is invalid");
    }
    Ok(verified)
}

pub fn verify_collaboration_discovery_withdrawal(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    current_advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationDiscoveryWithdrawal> {
    validate_discovery_advertisement_admissibility(current_advertisement, now)?;
    let message = verify_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
        now,
    )?;
    let withdrawal = verify_discovery_withdrawal_from_message(message)?;
    let advertisement = current_advertisement
        .message()
        .envelope()
        .payload
        .sender_profile_did
        .as_str();
    if withdrawal.message.envelope().payload.sender_profile_did != advertisement {
        anyhow::bail!(
            "collaboration discovery withdrawal Profile does not match the advertisement"
        );
    }
    if !current_advertisement.profile.authorizes_signer(
        &withdrawal.message.envelope().signer_did,
        COLLABORATION_DISCOVERY_SERVICE,
        COLLABORATION_DISCOVERY_WITHDRAWAL_PAYLOAD_TYPE,
    ) {
        anyhow::bail!("collaboration discovery withdrawal signer is not authorized");
    }
    if withdrawal.payload.advertisement_envelope_sha256
        != current_advertisement.message().envelope_sha256()
    {
        anyhow::bail!("collaboration discovery withdrawal advertisement hash mismatch");
    }
    Ok(withdrawal)
}

pub fn verify_collaboration_discovery_mailbox_poll(
    envelope_bytes: &[u8],
    network_profile: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    now: u64,
) -> anyhow::Result<VerifiedCollaborationDiscoveryMailboxPoll> {
    let message = verify_collaboration_message(
        envelope_bytes,
        network_profile,
        COLLABORATION_DISCOVERY_SERVICE,
        now,
    )?;
    verify_discovery_mailbox_poll_from_message(message)
}

fn verify_discovery_advertisement_from_message(
    message: VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationDiscoveryAdvertisement> {
    let envelope = &message.envelope().payload;
    if envelope.payload_type != COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE {
        anyhow::bail!("unsupported collaboration discovery advertisement payload_type");
    }
    if envelope.conversation_id != COLLABORATION_DISCOVERY_DIRECTORY_ID {
        anyhow::bail!("collaboration discovery advertisement belongs to another scope");
    }
    if envelope.recipient.kind != CollaborationRecipientKind::Conversation
        || envelope.recipient.id != COLLABORATION_DISCOVERY_DIRECTORY_ID
    {
        anyhow::bail!("collaboration discovery advertisement recipient is invalid");
    }
    let payload = decode_canonical_payload::<CollaborationDiscoveryAdvertisementPayload>(
        &envelope.payload,
        "collaboration discovery advertisement",
    )?;
    validate_product_ttl(
        envelope.created_at,
        envelope.expires_at,
        COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
        "collaboration discovery advertisement",
    )?;
    let profile = verify_signed_profile_document(&payload.signed_profile)
        .context("invalid collaboration discovery advertisement profile")?;
    require_profile_message_authority(&profile, &message, "collaboration discovery advertisement")?;
    Ok(VerifiedCollaborationDiscoveryAdvertisement {
        message,
        payload,
        profile,
    })
}

fn verify_contact_request_from_message(
    message: VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationContactRequest> {
    let envelope = &message.envelope().payload;
    if envelope.payload_type != COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE {
        anyhow::bail!("unsupported collaboration contact request payload_type");
    }
    if envelope.conversation_id != COLLABORATION_DISCOVERY_CONTACT_ID {
        anyhow::bail!("collaboration contact request belongs to another scope");
    }
    if envelope.recipient.kind != CollaborationRecipientKind::Profile {
        anyhow::bail!("collaboration contact request recipient is invalid");
    }
    if envelope.sender_profile_did == envelope.recipient.id {
        anyhow::bail!("collaboration contact request cannot target the sender Profile");
    }
    let payload = decode_canonical_payload::<CollaborationContactRequestPayload>(
        &envelope.payload,
        "collaboration contact request",
    )?;
    validate_sha256_label(
        &payload.advertisement_envelope_sha256,
        "collaboration contact request advertisement hash",
    )?;
    validate_product_ttl(
        envelope.created_at,
        envelope.expires_at,
        COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
        "collaboration contact request",
    )?;
    let profile = verify_signed_profile_document(&payload.signed_profile)
        .context("invalid collaboration contact request profile")?;
    require_profile_message_authority(&profile, &message, "collaboration contact request")?;
    Ok(VerifiedCollaborationContactRequest {
        message,
        payload,
        profile,
    })
}

fn verify_discovery_withdrawal_from_message(
    message: VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationDiscoveryWithdrawal> {
    let envelope = &message.envelope().payload;
    if envelope.payload_type != COLLABORATION_DISCOVERY_WITHDRAWAL_PAYLOAD_TYPE {
        anyhow::bail!("unsupported collaboration discovery withdrawal payload_type");
    }
    if envelope.conversation_id != COLLABORATION_DISCOVERY_DIRECTORY_ID {
        anyhow::bail!("collaboration discovery withdrawal belongs to another scope");
    }
    if envelope.recipient.kind != CollaborationRecipientKind::Conversation
        || envelope.recipient.id != COLLABORATION_DISCOVERY_DIRECTORY_ID
    {
        anyhow::bail!("collaboration discovery withdrawal recipient is invalid");
    }
    let payload = decode_canonical_payload::<CollaborationDiscoveryWithdrawalPayload>(
        &envelope.payload,
        "collaboration discovery withdrawal",
    )?;
    validate_sha256_label(
        &payload.advertisement_envelope_sha256,
        "collaboration discovery withdrawal advertisement hash",
    )?;
    validate_product_ttl(
        envelope.created_at,
        envelope.expires_at,
        COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
        "collaboration discovery withdrawal",
    )?;
    Ok(VerifiedCollaborationDiscoveryWithdrawal { message, payload })
}

fn verify_discovery_mailbox_poll_from_message(
    message: VerifiedCollaborationMessage,
) -> anyhow::Result<VerifiedCollaborationDiscoveryMailboxPoll> {
    let envelope = &message.envelope().payload;
    if envelope.payload_type != COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE {
        anyhow::bail!("unsupported collaboration discovery mailbox poll payload_type");
    }
    if envelope.conversation_id != COLLABORATION_DISCOVERY_DIRECTORY_ID {
        anyhow::bail!("collaboration discovery mailbox poll belongs to another scope");
    }
    if envelope.recipient.kind != CollaborationRecipientKind::Profile
        || envelope.recipient.id != envelope.sender_profile_did
    {
        anyhow::bail!("collaboration discovery mailbox poll recipient is invalid");
    }
    let payload = decode_canonical_payload::<CollaborationDiscoveryMailboxPollPayload>(
        &envelope.payload,
        "collaboration discovery mailbox poll",
    )?;
    validate_canonical_did(
        &payload.profile_did,
        "collaboration discovery mailbox poll profile DID",
    )?;
    if payload.profile_did != envelope.sender_profile_did {
        anyhow::bail!("collaboration discovery mailbox poll Profile mismatch");
    }
    validate_product_ttl(
        envelope.created_at,
        envelope.expires_at,
        COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
        "collaboration discovery mailbox poll",
    )?;
    Ok(VerifiedCollaborationDiscoveryMailboxPoll { message, payload })
}

fn bind_contact_request_to_advertisement(
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    request: &VerifiedCollaborationContactRequest,
) -> anyhow::Result<()> {
    let advertisement_envelope = &advertisement.message.envelope().payload;
    let request_envelope = &request.message.envelope().payload;
    if request.payload.advertisement_envelope_sha256 != advertisement.message.envelope_sha256() {
        anyhow::bail!("collaboration contact request advertisement hash mismatch");
    }
    if request_envelope.recipient.id != advertisement_envelope.sender_profile_did {
        anyhow::bail!(
            "collaboration contact request recipient does not match the advertisement sender"
        );
    }
    if request_envelope.network_id != advertisement_envelope.network_id {
        anyhow::bail!("collaboration contact request belongs to another network");
    }
    Ok(())
}

pub(crate) fn verify_stored_collaboration_contact_decision_receipt(
    envelope_bytes: &[u8],
    request: &VerifiedCollaborationContactRequest,
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
) -> anyhow::Result<VerifiedCollaborationContactDecisionReceipt> {
    let verified = verify_stored_contact_decision_receipt_envelope(envelope_bytes)?;
    validate_contact_decision_receipt_against_request(&verified.envelope, request, advertisement)?;
    Ok(verified)
}

pub(crate) fn verify_stored_contact_decision_receipt_envelope(
    envelope_bytes: &[u8],
) -> anyhow::Result<VerifiedCollaborationContactDecisionReceipt> {
    if envelope_bytes.is_empty()
        || envelope_bytes.len() > MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES
    {
        anyhow::bail!("collaboration contact decision receipt exceeds the byte limit");
    }
    let envelope: SignedCollaborationContactDecisionReceipt =
        serde_json::from_slice(envelope_bytes)
            .context("invalid collaboration contact decision receipt envelope")?;
    let canonical_envelope =
        canonical_signed_collaboration_contact_decision_receipt_bytes(&envelope)?;
    if canonical_envelope != envelope_bytes {
        anyhow::bail!("collaboration contact decision receipt envelope is not canonical JSON");
    }
    validate_signature_shape(&envelope.signature)?;
    validate_contact_decision_receipt_shape(&envelope.payload)?;
    if envelope.signer_did != envelope.payload.recipient_endpoint_did {
        anyhow::bail!(
            "collaboration contact decision receipt signer does not match recipient endpoint DID"
        );
    }
    validate_canonical_did(
        &envelope.signer_did,
        "collaboration contact decision receipt signer",
    )?;
    verify_signed_json_envelope_against_dids(
        envelope_bytes,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
        std::slice::from_ref(&envelope.signer_did),
    )?;
    Ok(VerifiedCollaborationContactDecisionReceipt { envelope })
}

fn validate_discovery_advertisement_admissibility(
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    now: u64,
) -> anyhow::Result<()> {
    let envelope = &advertisement.message.envelope().payload;
    if envelope.created_at > now.saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS) {
        anyhow::bail!("collaboration discovery advertisement is not yet valid");
    }
    if envelope.expires_at <= now {
        anyhow::bail!("collaboration discovery advertisement is expired");
    }
    Ok(())
}

fn validate_contact_request_relative_timing(
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    request: &VerifiedCollaborationContactRequest,
) -> anyhow::Result<()> {
    let advertisement_envelope = &advertisement.message.envelope().payload;
    let request_envelope = &request.message.envelope().payload;
    if request_envelope
        .created_at
        .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
        < advertisement_envelope.created_at
    {
        anyhow::bail!("collaboration contact request predates the discovery advertisement");
    }
    if request_envelope.created_at
        > advertisement_envelope
            .expires_at
            .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
    {
        anyhow::bail!(
            "collaboration contact request arrives after the discovery advertisement expired"
        );
    }
    Ok(())
}

fn require_profile_message_authority(
    profile: &VerifiedCollaborationProfileDocument,
    message: &VerifiedCollaborationMessage,
    label: &str,
) -> anyhow::Result<()> {
    let envelope = message.envelope();
    if profile.document().profile_did != envelope.payload.sender_profile_did {
        anyhow::bail!("{label} sender Profile does not match the signed Profile");
    }
    if !profile.authorizes_endpoint(&envelope.signer_did) {
        anyhow::bail!("{label} sender endpoint is not authorized by the signed profile");
    }
    if !profile.authorizes_signer(
        &envelope.signer_did,
        &envelope.payload.sender_service,
        &envelope.payload.payload_type,
    ) {
        anyhow::bail!("{label} signer is not authorized by the signed profile");
    }
    Ok(())
}

fn decode_canonical_payload<T>(value: &serde_json::Value, label: &str) -> anyhow::Result<T>
where
    T: DeserializeOwned + Serialize,
{
    let raw = serde_json::to_vec(value)?;
    let payload: T =
        serde_json::from_slice(&raw).with_context(|| format!("invalid {label} payload"))?;
    if serde_json::to_vec(&payload)? != raw {
        anyhow::bail!("{label} payload is not canonical JSON");
    }
    Ok(payload)
}

fn validate_sha256_label(value: &str, label: &str) -> anyhow::Result<()> {
    if value.len() != "sha256:".len() + 64
        || !value.starts_with("sha256:")
        || !value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("{label} is invalid");
    }
    Ok(())
}

fn validate_product_ttl(
    created_at: u64,
    expires_at: u64,
    max_ttl_secs: u64,
    label: &str,
) -> anyhow::Result<()> {
    if expires_at <= created_at {
        anyhow::bail!("{label} lifetime must be greater than zero");
    }
    if expires_at - created_at > max_ttl_secs {
        anyhow::bail!("{label} lifetime is too long");
    }
    Ok(())
}

pub fn canonical_signed_collaboration_contact_decision_receipt_bytes(
    receipt: &SignedCollaborationContactDecisionReceipt,
) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::to_value(receipt)?)
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)))
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

fn validate_contact_decision_receipt_shape(
    receipt: &CollaborationContactDecisionReceipt,
) -> anyhow::Result<()> {
    if receipt.schema != COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1 {
        anyhow::bail!("unsupported collaboration contact decision receipt schema");
    }
    crate::collaboration_network::validate_network_id(&receipt.network_id)?;
    validate_sha256_label(
        &receipt.request_envelope_sha256,
        "collaboration contact decision receipt request hash",
    )?;
    crate::collaboration_protocol::validate_id(
        &receipt.conversation_id,
        "contact decision receipt conversation_id",
    )?;
    validate_canonical_did(
        &receipt.requester_profile_did,
        "contact decision receipt requester profile DID",
    )?;
    validate_canonical_did(
        &receipt.requester_endpoint_did,
        "contact decision receipt requester endpoint DID",
    )?;
    validate_strong_id(
        &receipt.request_message_id,
        "contact decision receipt message_id",
    )?;
    validate_strong_id(
        &receipt.request_message_nonce,
        "contact decision receipt message nonce",
    )?;
    validate_canonical_did(
        &receipt.recipient_profile_did,
        "contact decision receipt recipient profile DID",
    )?;
    validate_canonical_did(
        &receipt.recipient_endpoint_did,
        "contact decision receipt recipient endpoint DID",
    )?;
    if receipt.decided_at == 0 {
        anyhow::bail!("collaboration contact decision receipt time is invalid");
    }
    Ok(())
}

fn validate_contact_decision_receipt_against_request(
    envelope: &SignedCollaborationContactDecisionReceipt,
    request: &VerifiedCollaborationContactRequest,
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
) -> anyhow::Result<()> {
    let receipt = &envelope.payload;
    let request_envelope = &request.message().envelope().payload;
    if receipt.network_id != request_envelope.network_id {
        anyhow::bail!("collaboration contact decision receipt belongs to another network");
    }
    if receipt.request_envelope_sha256 != request.message().envelope_sha256() {
        anyhow::bail!("collaboration contact decision receipt request hash mismatch");
    }
    if receipt.conversation_id != request_envelope.conversation_id {
        anyhow::bail!("collaboration contact decision receipt conversation mismatch");
    }
    if receipt.request_message_id != request_envelope.message_id {
        anyhow::bail!("collaboration contact decision receipt message_id mismatch");
    }
    if receipt.request_message_nonce != request_envelope.nonce {
        anyhow::bail!("collaboration contact decision receipt message nonce mismatch");
    }
    if receipt.requester_endpoint_did != request.route_endpoint_did()? {
        anyhow::bail!("collaboration contact decision receipt requester endpoint DID mismatch");
    }
    if receipt.requester_profile_did != request.requester_profile_did() {
        anyhow::bail!("collaboration contact decision receipt requester profile DID mismatch");
    }
    if receipt.recipient_profile_did != advertisement.profile_did() {
        anyhow::bail!("collaboration contact decision receipt recipient profile DID mismatch");
    }
    if !advertisement
        .profile
        .authorizes_endpoint(&receipt.recipient_endpoint_did)
    {
        anyhow::bail!("collaboration contact decision receipt endpoint is not authorized");
    }
    if !advertisement.profile.authorizes_signer(
        &envelope.signer_did,
        COLLABORATION_DISCOVERY_SERVICE,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1,
    ) {
        anyhow::bail!("collaboration contact decision receipt signer is not authorized");
    }
    if receipt
        .decided_at
        .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
        < request_envelope.created_at
        || receipt.decided_at
            > request_envelope
                .expires_at
                .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
    {
        anyhow::bail!("collaboration contact decision receipt time is invalid");
    }
    Ok(())
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
        CollaborationNetworkProfileMode, SignedCollaborationNetworkProfile,
        VerifiedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_protocol::verify_stored_collaboration_message;

    const NETWORK: &str = "collaboration-discovery-test";
    const NOW: u64 = 1_800_000_000;

    fn device_did(signing_key: &SigningKey) -> String {
        crate::crypto::encode_signing_key_did(signing_key)
    }

    fn verified_profile(signing_key: &SigningKey) -> VerifiedCollaborationNetworkProfile {
        verified_profile_with_network(signing_key, NETWORK)
    }

    fn verified_profile_with_network(
        signing_key: &SigningKey,
        network_id: &str,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = device_did(signing_key);
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
        let (signature, envelope_signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did: envelope_signer_did.clone(),
        };
        let bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        match validate_collaboration_network_profile(
            Some(&bytes),
            network_id,
            &[envelope_signer_did],
            None,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn signed_message(
        signing_key: &SigningKey,
        conversation_id: &str,
        recipient: CollaborationRecipient,
        payload_type: &str,
        payload: serde_json::Value,
        created_at: u64,
        expires_at: u64,
    ) -> Vec<u8> {
        let sender_profile_did = device_did(&profile_signing_key_for_device(signing_key));
        let message = CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: crate::collaboration_core::random_hex_128().unwrap(),
            nonce: crate::collaboration_core::random_hex_128().unwrap(),
            created_at,
            expires_at,
            sender_profile_did,
            sender_service: COLLABORATION_DISCOVERY_SERVICE.to_string(),
            recipient,
            payload_type: payload_type.to_string(),
            payload,
        };
        let payload_bytes = canonical_collaboration_message_bytes(&message).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
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

    fn profile_signing_key_for_device(device_signing_key: &SigningKey) -> SigningKey {
        let mut seed = device_signing_key.to_bytes();
        seed[0] ^= 0xA5;
        seed[31] ^= 0x5A;
        SigningKey::from_bytes(&seed)
    }

    fn signed_profile_for_device(
        device_signing_key: &SigningKey,
        display_name: &str,
        handle: Option<&str>,
        revision: u64,
        previous_profile_sha256: Option<&str>,
        updated_at: u64,
    ) -> SignedCollaborationProfileDocument {
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_signing_key_for_device(device_signing_key),
            display_name,
            handle,
            revision,
            previous_profile_sha256,
            updated_at,
            vec![device_did(device_signing_key)],
        )
        .unwrap()
        .signed_envelope()
        .clone()
    }

    fn canonical_payload_value<T: Serialize>(payload: &T) -> serde_json::Value {
        serde_json::from_slice(&serde_json::to_vec(payload).unwrap()).unwrap()
    }

    fn advertisement_bytes_with_window(
        signing_key: &SigningKey,
        created_at: u64,
        expires_at: u64,
    ) -> Vec<u8> {
        signed_message(
            signing_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationDiscoveryAdvertisementPayload {
                signed_profile: signed_profile_for_device(
                    signing_key,
                    "Bob",
                    Some("bob"),
                    1,
                    None,
                    created_at,
                ),
            }),
            created_at,
            expires_at,
        )
    }

    fn advertisement_bytes(signing_key: &SigningKey) -> Vec<u8> {
        advertisement_bytes_with_window(signing_key, NOW, NOW + 60)
    }

    fn request_bytes_for_advertisement(
        signing_key: &SigningKey,
        recipient_profile_did: &str,
        advertisement_envelope_sha256: &str,
        created_at: u64,
        expires_at: u64,
    ) -> Vec<u8> {
        signed_message(
            signing_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: recipient_profile_did.to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement_envelope_sha256.to_string(),
                signed_profile: signed_profile_for_device(
                    signing_key,
                    "Alice",
                    Some("alice"),
                    1,
                    None,
                    created_at,
                ),
            }),
            created_at,
            expires_at,
        )
    }

    fn decision_receipt_bytes(
        signing_key: &SigningKey,
        request: &VerifiedCollaborationContactRequest,
        advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
        decision: CollaborationContactDecision,
        decided_at: u64,
    ) -> Vec<u8> {
        decision_receipt_bytes_with(
            signing_key,
            request,
            advertisement,
            decision,
            decided_at,
            |_| {},
        )
    }

    fn decision_receipt_bytes_with(
        signing_key: &SigningKey,
        request: &VerifiedCollaborationContactRequest,
        advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
        decision: CollaborationContactDecision,
        decided_at: u64,
        mutate: impl FnOnce(&mut CollaborationContactDecisionReceipt),
    ) -> Vec<u8> {
        let request_envelope = &request.message().envelope().payload;
        let request_payload: CollaborationContactRequestPayload =
            serde_json::from_value(request_envelope.payload.clone()).unwrap();
        let requester_profile =
            crate::collaboration_profile_authority::verify_signed_profile_document(
                &request_payload.signed_profile,
            )
            .unwrap();
        let mut receipt = CollaborationContactDecisionReceipt {
            schema: COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1.to_string(),
            network_id: request_envelope.network_id.clone(),
            request_envelope_sha256: request.message().envelope_sha256().to_string(),
            conversation_id: request_envelope.conversation_id.clone(),
            requester_profile_did: requester_profile.document().profile_did.clone(),
            requester_endpoint_did: request.message().envelope().signer_did.clone(),
            request_message_id: request_envelope.message_id.clone(),
            request_message_nonce: request_envelope.nonce.clone(),
            recipient_profile_did: advertisement.profile_did().to_string(),
            recipient_endpoint_did: device_did(signing_key),
            decision,
            decided_at,
        };
        mutate(&mut receipt);
        let payload_bytes = serde_json::to_vec(&serde_json::to_value(&receipt).unwrap()).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_contact_decision_receipt_bytes(
            &SignedCollaborationContactDecisionReceipt {
                payload: receipt,
                signature,
                signer_did,
            },
        )
        .unwrap()
    }

    fn stored_advertisement(
        envelope_bytes: &[u8],
        profile: &VerifiedCollaborationNetworkProfile,
    ) -> anyhow::Result<VerifiedCollaborationDiscoveryAdvertisement> {
        let message = verify_stored_collaboration_message(
            envelope_bytes,
            profile,
            COLLABORATION_DISCOVERY_SERVICE,
        )?;
        verify_discovery_advertisement_from_message(message)
    }

    fn stored_request(
        envelope_bytes: &[u8],
        profile: &VerifiedCollaborationNetworkProfile,
        advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    ) -> anyhow::Result<VerifiedCollaborationContactRequest> {
        let request = verify_stored_unbound_collaboration_contact_request(envelope_bytes, profile)?;
        bind_stored_collaboration_contact_request(request, advertisement)
    }

    #[test]
    fn verifies_valid_advertisement_request_and_decision_receipt() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let requester_did = device_did(&requester_key);
        let (recipient_key, _) = generate_keypair();

        let advertisement_bytes = advertisement_bytes(&recipient_key);
        let advertisement =
            verify_collaboration_discovery_advertisement(&advertisement_bytes, &profile, NOW)
                .unwrap();
        let request_bytes = request_bytes_for_advertisement(
            &requester_key,
            advertisement.profile_did(),
            advertisement.message().envelope_sha256(),
            NOW,
            NOW + 60,
        );
        let request =
            verify_collaboration_contact_request(&request_bytes, &profile, &advertisement, NOW)
                .unwrap();
        assert_eq!(
            request.requester_profile_did(),
            device_did(&profile_signing_key_for_device(&requester_key))
        );
        assert_eq!(
            request.message().envelope().payload.recipient.id,
            advertisement.profile_did()
        );
        assert_eq!(request.route_endpoint_did().unwrap(), requester_did);

        let receipt_bytes = decision_receipt_bytes(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
        );
        let live_receipt = verify_collaboration_contact_decision_receipt(
            &receipt_bytes,
            &request,
            &advertisement,
            NOW + 1,
        )
        .unwrap();
        let stored_advertisement = stored_advertisement(&advertisement_bytes, &profile).unwrap();
        let stored_request =
            stored_request(&request_bytes, &profile, &stored_advertisement).unwrap();
        let stored_receipt = verify_stored_collaboration_contact_decision_receipt(
            &receipt_bytes,
            &stored_request,
            &stored_advertisement,
        )
        .unwrap();
        assert_eq!(stored_receipt, live_receipt);
        assert_eq!(
            verify_stored_collaboration_contact_decision_receipt(
                &receipt_bytes,
                &stored_request,
                &stored_advertisement,
            )
            .unwrap(),
            stored_receipt
        );
    }

    #[test]
    fn mailbox_poll_requires_a_canonical_profile_did() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (device_key, _) = generate_keypair();
        let signed_profile =
            signed_profile_for_device(&device_key, "Alice", Some("alice"), 1, None, NOW);
        let profile_did =
            crate::collaboration_profile_authority::verify_signed_profile_document(&signed_profile)
                .unwrap()
                .document()
                .profile_did
                .clone();
        let poll = signed_message(
            &device_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: profile_did.clone(),
            },
            COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
            serde_json::json!({
                "mailbox_kind": "requests",
                "profile_did": profile_did,
            }),
            NOW,
            NOW + 30,
        );
        let verified = verify_collaboration_discovery_mailbox_poll(&poll, &profile, NOW).unwrap();
        assert_eq!(
            verified.mailbox_kind(),
            CollaborationDiscoveryMailboxKind::Requests
        );

        let device_only_poll = signed_message(
            &device_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: profile_did,
            },
            COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
            serde_json::json!({"mailbox_kind": "requests"}),
            NOW,
            NOW + 30,
        );
        assert!(
            verify_collaboration_discovery_mailbox_poll(&device_only_poll, &profile, NOW)
                .unwrap_err()
                .to_string()
                .contains("invalid collaboration discovery mailbox poll payload")
        );
    }

    #[test]
    fn rejects_noncanonical_payloads_unknown_fields_and_invalid_signed_profiles() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let (recipient_key, _) = generate_keypair();
        let recipient_did = device_did(&recipient_key);
        let advertisement = verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&recipient_key),
            &profile,
            NOW,
        )
        .unwrap();

        let canonical_request = request_bytes_for_advertisement(
            &requester_key,
            advertisement.profile_did(),
            advertisement.message().envelope_sha256(),
            NOW,
            NOW + 60,
        );
        let mut noncanonical_request: SignedCollaborationMessage =
            serde_json::from_slice(&canonical_request).unwrap();
        let mut reordered = serde_json::Map::new();
        reordered.insert(
            "advertisement_envelope_sha256".to_string(),
            serde_json::json!(advertisement.message().envelope_sha256()),
        );
        reordered.insert(
            "signed_profile".to_string(),
            serde_json::to_value(signed_profile_for_device(
                &requester_key,
                "Alice",
                Some("alice"),
                1,
                None,
                NOW,
            ))
            .unwrap(),
        );
        noncanonical_request.payload.payload = serde_json::Value::Object(reordered);
        let noncanonical = serde_json::to_vec(&noncanonical_request).unwrap();
        assert!(
            verify_collaboration_contact_request(&noncanonical, &profile, &advertisement, NOW)
                .unwrap_err()
                .to_string()
                .contains("not canonical JSON")
        );

        let unknown_field = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            serde_json::json!({
                "advertisement_envelope_sha256":
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "signed_profile": signed_profile_for_device(
                    &requester_key,
                    "Alice",
                    Some("alice"),
                    1,
                    None,
                    NOW,
                ),
                "extra": true,
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_contact_request(
            &unknown_field,
            &profile,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("invalid collaboration contact request payload"));

        let mut invalid_profile_name = serde_json::to_value(signed_profile_for_device(
            &requester_key,
            "Alice",
            Some("alice"),
            1,
            None,
            NOW,
        ))
        .unwrap();
        invalid_profile_name["payload"]["display_name"] = serde_json::json!(" Alice ");
        let invalid_signed_profile_name = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement
                    .message()
                    .envelope_sha256()
                    .to_string(),
                signed_profile: serde_json::from_value(invalid_profile_name).unwrap(),
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_contact_request(
            &invalid_signed_profile_name,
            &profile,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("invalid collaboration contact request profile"));

        let unauthorized_profile =
            crate::collaboration_profile_authority::signed_profile_document_for_test(
                &profile_signing_key_for_device(&requester_key),
                "Alice",
                Some("alice"),
                1,
                None,
                NOW,
                vec![recipient_did.clone()],
            )
            .unwrap()
            .signed_envelope()
            .clone();
        let unauthorized_signed_profile = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement
                    .message()
                    .envelope_sha256()
                    .to_string(),
                signed_profile: unauthorized_profile,
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_contact_request(
            &unauthorized_signed_profile,
            &profile,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("sender endpoint is not authorized"));

        let requester_did = device_did(&requester_key);
        let unauthorized_signer_profile =
            crate::collaboration_profile_authority::signed_profile_document_with_authority_for_test(
                &profile_signing_key_for_device(&requester_key),
                "Alice",
                Some("alice"),
                1,
                None,
                NOW,
                crate::collaboration_profile_authority::ProfileAuthorityForTest {
                    endpoint_dids: vec![requester_did],
                    signer_dids: vec![recipient_did.clone()],
                },
            )
            .unwrap()
            .signed_envelope()
            .clone();
        let unauthorized_signer_request = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement
                    .message()
                    .envelope_sha256()
                    .to_string(),
                signed_profile: unauthorized_signer_profile,
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_contact_request(
            &unauthorized_signer_request,
            &profile,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("signer is not authorized"));
    }

    #[test]
    fn rejects_legacy_unsigned_advertisement_and_request_payload_shapes() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let (recipient_key, _) = generate_keypair();

        let legacy_advertisement = signed_message(
            &recipient_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            serde_json::json!({
                "display_name": "Legacy Bob",
                "handle": "legacy-bob",
            }),
            NOW,
            NOW + 60,
        );
        assert!(
            verify_collaboration_discovery_advertisement(&legacy_advertisement, &profile, NOW)
                .unwrap_err()
                .to_string()
                .contains("invalid collaboration discovery advertisement payload")
        );

        let advertisement = verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&recipient_key),
            &profile,
            NOW,
        )
        .unwrap();
        let legacy_request = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            serde_json::json!({
                "advertisement_envelope_sha256": advertisement.message().envelope_sha256(),
                "display_name": "Legacy Alice",
                "handle": "legacy-alice",
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_contact_request(
            &legacy_request,
            &profile,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("invalid collaboration contact request payload"));
    }

    #[test]
    fn rejects_oversized_nested_signed_profile_payload() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (device_key, _) = generate_keypair();

        let mut oversized_profile = serde_json::to_value(signed_profile_for_device(
            &device_key,
            "Alice",
            Some("alice"),
            1,
            None,
            NOW,
        ))
        .unwrap();
        oversized_profile["signature"] = serde_json::json!("a".repeat(20_000));
        let oversized_advertisement = signed_message(
            &device_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationDiscoveryAdvertisementPayload {
                signed_profile: serde_json::from_value(oversized_profile).unwrap(),
            }),
            NOW,
            NOW + 60,
        );

        let error =
            verify_collaboration_discovery_advertisement(&oversized_advertisement, &profile, NOW)
                .unwrap_err();
        assert!(format!("{error:#}").contains("profile document is too large"));
    }

    #[test]
    fn rejects_wrong_recipients_self_contact_wrong_network_and_overlong_ttls() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let requester_did = device_did(&requester_key);
        let (recipient_key, _) = generate_keypair();
        let advertisement = verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&recipient_key),
            &profile,
            NOW,
        )
        .unwrap();

        let wrong_advertisement_recipient = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: requester_did.clone(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationDiscoveryAdvertisementPayload {
                signed_profile: signed_profile_for_device(
                    &requester_key,
                    "Alice",
                    None,
                    1,
                    None,
                    NOW,
                ),
            }),
            NOW,
            NOW + 60,
        );
        assert!(verify_collaboration_discovery_advertisement(
            &wrong_advertisement_recipient,
            &profile,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("recipient is invalid"));

        let self_request = request_bytes_for_advertisement(
            &requester_key,
            &device_did(&profile_signing_key_for_device(&requester_key)),
            advertisement.message().envelope_sha256(),
            NOW,
            NOW + 60,
        );
        assert!(
            verify_collaboration_contact_request(&self_request, &profile, &advertisement, NOW)
                .unwrap_err()
                .to_string()
                .contains("cannot target the sender Profile")
        );

        let (other_signer, _) = generate_keypair();
        let other_profile = verified_profile_with_network(&other_signer, "other.network");
        assert!(verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&recipient_key),
            &other_profile,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("belongs to another network"));

        let long_advertisement = signed_message(
            &recipient_key,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationDiscoveryAdvertisementPayload {
                signed_profile: signed_profile_for_device(
                    &recipient_key,
                    "Bob",
                    None,
                    1,
                    None,
                    NOW,
                ),
            }),
            NOW,
            NOW + COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS + 1,
        );
        assert!(
            verify_collaboration_discovery_advertisement(&long_advertisement, &profile, NOW)
                .unwrap_err()
                .to_string()
                .contains("lifetime is too long")
        );

        let long_request = signed_message(
            &requester_key,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement.profile_did().to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement
                    .message()
                    .envelope_sha256()
                    .to_string(),
                signed_profile: signed_profile_for_device(
                    &requester_key,
                    "Alice",
                    None,
                    1,
                    None,
                    NOW,
                ),
            }),
            NOW,
            NOW + COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS + 1,
        );
        assert!(
            verify_collaboration_contact_request(&long_request, &profile, &advertisement, NOW)
                .unwrap_err()
                .to_string()
                .contains("lifetime is too long")
        );
    }

    #[test]
    fn rejects_substituted_stale_and_no_longer_live_advertisements() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let (recipient_key, _) = generate_keypair();
        let (other_key, _) = generate_keypair();

        let advertisement_bytes_value = advertisement_bytes(&recipient_key);
        let advertisement =
            verify_collaboration_discovery_advertisement(&advertisement_bytes_value, &profile, NOW)
                .unwrap();
        let other_advertisement = verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&other_key),
            &profile,
            NOW,
        )
        .unwrap();

        let substituted = request_bytes_for_advertisement(
            &requester_key,
            advertisement.profile_did(),
            other_advertisement.message().envelope_sha256(),
            NOW,
            NOW + 60,
        );
        assert!(
            verify_collaboration_contact_request(&substituted, &profile, &advertisement, NOW)
                .unwrap_err()
                .to_string()
                .contains("advertisement hash mismatch")
        );

        let stale = request_bytes_for_advertisement(
            &requester_key,
            advertisement.profile_did(),
            advertisement.message().envelope_sha256(),
            advertisement.message().envelope().payload.expires_at
                + MAX_COLLABORATION_CLOCK_SKEW_SECS
                + 1,
            advertisement.message().envelope().payload.expires_at
                + MAX_COLLABORATION_CLOCK_SKEW_SECS
                + 61,
        );
        let stored_advertisement =
            stored_advertisement(&advertisement_bytes_value, &profile).unwrap();
        assert!(stored_request(&stale, &profile, &stored_advertisement)
            .unwrap_err()
            .to_string()
            .contains("advertisement expired"));

        let later_live_request = request_bytes_for_advertisement(
            &requester_key,
            advertisement.profile_did(),
            advertisement.message().envelope_sha256(),
            NOW + 10,
            NOW + 70,
        );
        assert!(verify_collaboration_contact_request(
            &later_live_request,
            &profile,
            &advertisement,
            NOW + 61,
        )
        .unwrap_err()
        .to_string()
        .contains("advertisement is expired"));
    }

    #[test]
    fn rejects_bad_decision_receipts_and_future_decided_at() {
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer);
        let (requester_key, _) = generate_keypair();
        let (recipient_key, _) = generate_keypair();
        let recipient_did = device_did(&recipient_key);
        let (other_key, _) = generate_keypair();
        let other_did = device_did(&other_key);

        let advertisement = verify_collaboration_discovery_advertisement(
            &advertisement_bytes(&recipient_key),
            &profile,
            NOW,
        )
        .unwrap();
        let request = verify_collaboration_contact_request(
            &request_bytes_for_advertisement(
                &requester_key,
                advertisement.profile_did(),
                advertisement.message().envelope_sha256(),
                NOW,
                NOW + 60,
            ),
            &profile,
            &advertisement,
            NOW,
        )
        .unwrap();

        let valid = decision_receipt_bytes(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
        );
        let reparsed: SignedCollaborationContactDecisionReceipt =
            serde_json::from_slice(&valid).unwrap();
        let noncanonical = serde_json::to_string_pretty(&reparsed)
            .unwrap()
            .into_bytes();
        assert!(verify_collaboration_contact_decision_receipt(
            &noncanonical,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("not canonical JSON"));

        let wrong_signer = decision_receipt_bytes_with(
            &other_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| {
                receipt.recipient_endpoint_did = recipient_did.clone();
            },
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_signer,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("signer does not match"));

        let wrong_recipient = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| {
                receipt.recipient_endpoint_did = other_did.clone();
            },
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_recipient,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("recipient endpoint DID"));

        let wrong_request = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| {
                receipt.request_envelope_sha256 =
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string();
            },
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_request,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("request hash mismatch"));

        let wrong_network = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| receipt.network_id = "other.network".to_string(),
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_network,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("belongs to another network"));

        let wrong_conversation = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| receipt.conversation_id = "other.scope".to_string(),
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_conversation,
            &request,
            &advertisement,
            NOW + 1,
        )
        .unwrap_err()
        .to_string()
        .contains("conversation mismatch"));

        let wrong_nonce = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| {
                receipt.request_message_nonce = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()
            },
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &wrong_nonce,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("message nonce mismatch"));

        let future = decision_receipt_bytes(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 31,
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &future,
            &request,
            &advertisement,
            NOW
        )
        .unwrap_err()
        .to_string()
        .contains("decision receipt time is invalid"));

        let late = decision_receipt_bytes(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            request.message().envelope().payload.expires_at + MAX_COLLABORATION_CLOCK_SKEW_SECS + 1,
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &late,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("decision receipt time is invalid"));

        let self_receipt = decision_receipt_bytes_with(
            &recipient_key,
            &request,
            &advertisement,
            CollaborationContactDecision::Accepted,
            NOW + 1,
            |receipt| receipt.requester_endpoint_did = other_did.clone(),
        );
        assert!(verify_collaboration_contact_decision_receipt(
            &self_receipt,
            &request,
            &advertisement,
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("requester endpoint DID mismatch"));
    }
}
