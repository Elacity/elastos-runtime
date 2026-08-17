//! Signed receipt attesting an affordance-consent redemption (W2 step 9).
//!
//! When a single-use affordance-consent token is redeemed through
//! `validate-and-consume`, the runtime — the capability issuer and trust root —
//! signs an [`AffordanceGrantReceiptV1`] over the exact `(capsule, method,
//! arguments, resource, action)` that was approved and consumed. The holder can
//! later PROVE what was done in their name; verification needs only the runtime's
//! capability public key. This is "if there is no receipt, there is no act" made
//! concrete: the redemption fails closed unless a durable signed record exists
//! (see the blocking audit in `CapabilityManager::validate`), and this receipt is
//! the portable half the caller keeps.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::primitives::time::SecureTimestamp;

/// Schema tag for the v1 affordance-grant receipt.
pub const AFFORDANCE_RECEIPT_SCHEMA_V1: &str = "elastos.affordance.receipt.v1";

/// Domain-separation tag for the receipt signature, so a receipt signature can
/// never be confused with any other ed25519 signature this key produces.
const RECEIPT_SIG_DOMAIN: &[u8] = b"elastos.affordance.receipt.v1\0";

/// A signed attestation that a single-use affordance-consent token was redeemed
/// (W2 step 9). Signed by the runtime's capability issuer key; verifiable by
/// anyone holding the matching public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffordanceGrantReceiptV1 {
    /// Schema tag ([`AFFORDANCE_RECEIPT_SCHEMA_V1`]).
    pub schema: String,
    /// The capsule the affordance acted as / was bound to (`vm-{name}`).
    pub capsule: String,
    /// The affordance method that was redeemed.
    pub method_id: String,
    /// Canonical hash of the invocation arguments that were approved.
    pub input_hash: String,
    /// The resource the token authorised.
    pub resource: String,
    /// The action that was performed.
    pub action: String,
    /// The consumed token's id (for correlation with the grant/use chain).
    pub token_id: String,
    /// When the redemption was recorded.
    pub redeemed_at: SecureTimestamp,
    /// Issuer ed25519 public key (hex) that signed this receipt.
    pub signer: String,
    /// Ed25519 signature (base64) over the canonical receipt bytes.
    pub signature: String,
}

impl AffordanceGrantReceiptV1 {
    /// Canonical, signature-excluded digest the signature covers. Every
    /// variable-length field is length-prefixed under a domain tag so distinct
    /// field boundaries can never collide.
    fn signable_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(RECEIPT_SIG_DOMAIN);
        for field in [
            self.schema.as_str(),
            self.capsule.as_str(),
            self.method_id.as_str(),
            self.input_hash.as_str(),
            self.resource.as_str(),
            self.action.as_str(),
            self.token_id.as_str(),
            self.signer.as_str(),
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        // Bind the timestamp via its serde encoding (the same encoding the audit
        // chain serializes), length-prefixed.
        let ts = serde_json::to_vec(&self.redeemed_at).unwrap_or_default();
        hasher.update((ts.len() as u64).to_le_bytes());
        hasher.update(&ts);
        hasher.finalize().into()
    }

    /// Build and sign a receipt with the issuer signing key.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        signing_key: &SigningKey,
        signer_pubkey: [u8; 32],
        token_id: &str,
        capsule: &str,
        method_id: &str,
        input_hash: &str,
        resource: &str,
        action: &str,
    ) -> Self {
        let mut receipt = Self {
            schema: AFFORDANCE_RECEIPT_SCHEMA_V1.to_string(),
            capsule: capsule.to_string(),
            method_id: method_id.to_string(),
            input_hash: input_hash.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            token_id: token_id.to_string(),
            redeemed_at: SecureTimestamp::now(),
            signer: hex::encode(signer_pubkey),
            signature: String::new(),
        };
        let signature: Signature = signing_key.sign(&receipt.signable_digest());
        receipt.signature = BASE64.encode(signature.to_bytes());
        receipt
    }

    /// Verify the signature against a verifying key. Fails closed on any
    /// malformed signature.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> bool {
        let Ok(sig_bytes) = BASE64.decode(&self.signature) else {
            return false;
        };
        let sig_arr: [u8; 64] = match sig_bytes.try_into() {
            Ok(arr) => arr,
            Err(_) => return false,
        };
        let signature = Signature::from_bytes(&sig_arr);
        verifying_key
            .verify(&self.signable_digest(), &signature)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue_test_receipt() -> (AffordanceGrantReceiptV1, VerifyingKey) {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();
        let receipt = AffordanceGrantReceiptV1::issue(
            &signing_key,
            verifying_key.to_bytes(),
            "tok-1",
            "vm-player",
            "play",
            "abc123",
            "elastos://rights/play",
            "execute",
        );
        (receipt, verifying_key)
    }

    #[test]
    fn issued_receipt_verifies_under_the_issuer_key() {
        let (receipt, vk) = issue_test_receipt();
        assert_eq!(receipt.schema, AFFORDANCE_RECEIPT_SCHEMA_V1);
        assert_eq!(receipt.method_id, "play");
        assert_eq!(receipt.input_hash, "abc123");
        assert!(receipt.verify(&vk), "a freshly issued receipt must verify");
    }

    #[test]
    fn tampering_any_bound_field_breaks_the_signature() {
        // Each bound field is covered by the signature: changing it must fail.
        let (base, vk) = issue_test_receipt();
        for mutate in [
            |r: &mut AffordanceGrantReceiptV1| r.method_id = "delete".to_string(),
            |r: &mut AffordanceGrantReceiptV1| r.input_hash = "deadbeef".to_string(),
            |r: &mut AffordanceGrantReceiptV1| r.capsule = "vm-evil".to_string(),
            |r: &mut AffordanceGrantReceiptV1| r.resource = "elastos://rights/all".to_string(),
            |r: &mut AffordanceGrantReceiptV1| r.action = "admin".to_string(),
        ] {
            let mut tampered = base.clone();
            mutate(&mut tampered);
            assert!(!tampered.verify(&vk), "a tampered receipt must not verify");
        }
    }

    #[test]
    fn a_different_key_does_not_verify() {
        let (receipt, _vk) = issue_test_receipt();
        let other = SigningKey::generate(&mut rand::thread_rng()).verifying_key();
        assert!(
            !receipt.verify(&other),
            "a receipt must not verify under a different key"
        );
    }
}
