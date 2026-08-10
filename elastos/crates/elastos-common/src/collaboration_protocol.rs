//! Canonical wire types for Runtime-mediated collaboration.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const COLLABORATION_MESSAGE_SCHEMA_V1: &str = "elastos.collaboration.message/v1";
pub const COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1: &str =
    "elastos.collaboration.acceptance-receipt/v1";
pub const COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1: &str = "elastos.collaboration.message.v1";
pub const COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1: &str =
    "elastos.collaboration.acceptance-receipt.v1";

pub const MAX_COLLABORATION_ENVELOPE_BYTES: usize = 96 * 1024;
pub const MAX_COLLABORATION_ACCEPTANCE_RECEIPT_BYTES: usize = 8 * 1024;
pub const MAX_COLLABORATION_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_COLLABORATION_ID_BYTES: usize = 128;
pub const MAX_COLLABORATION_SERVICE_BYTES: usize = 128;
pub const MAX_COLLABORATION_PAYLOAD_TYPE_BYTES: usize = 128;
pub const MAX_COLLABORATION_MESSAGE_LIFETIME_SECS: u64 = 24 * 60 * 60;
pub const MAX_COLLABORATION_CLOCK_SKEW_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationRecipientKind {
    Profile,
    Conversation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationRecipient {
    pub kind: CollaborationRecipientKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationMessage {
    pub schema: String,
    pub network_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub nonce: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub sender_profile_did: String,
    pub sender_service: String,
    pub recipient: CollaborationRecipient,
    pub payload_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCollaborationMessage {
    pub payload: CollaborationMessage,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationAcceptanceReceipt {
    pub schema: String,
    pub network_id: String,
    pub message_envelope_sha256: String,
    pub conversation_id: String,
    pub sender_profile_did: String,
    pub message_id: String,
    pub message_nonce: String,
    pub recipient_endpoint_did: String,
    pub accepted_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCollaborationAcceptanceReceipt {
    pub payload: CollaborationAcceptanceReceipt,
    pub signature: String,
    pub signer_did: String,
}

pub fn canonical_collaboration_message_bytes(
    message: &CollaborationMessage,
) -> serde_json::Result<Vec<u8>> {
    canonical_json_bytes(message)
}

pub fn canonical_signed_collaboration_message_bytes(
    message: &SignedCollaborationMessage,
) -> serde_json::Result<Vec<u8>> {
    canonical_json_bytes(message)
}

pub fn canonical_collaboration_acceptance_receipt_bytes(
    receipt: &CollaborationAcceptanceReceipt,
) -> serde_json::Result<Vec<u8>> {
    canonical_json_bytes(receipt)
}

pub fn canonical_signed_collaboration_acceptance_receipt_bytes(
    receipt: &SignedCollaborationAcceptanceReceipt,
) -> serde_json::Result<Vec<u8>> {
    canonical_json_bytes(receipt)
}

pub fn collaboration_message_envelope_sha256(canonical_envelope: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(canonical_envelope)))
}

fn canonical_json_bytes<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::to_value(value)?)
}
