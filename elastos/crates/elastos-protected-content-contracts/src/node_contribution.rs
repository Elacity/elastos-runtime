use ed25519_dalek::Signature;
use sha2::{Digest as _, Sha256};

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::rights::{validate_active, validate_time_window};
use crate::{
    CanonicalContract, Digest32, KeyReleaseError, NodePublicKey, NodeSetV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, RightsDecisionV1,
    SignedNodeRightsDecisionV1, VerifiedKeyReleaseRequestV1, VerifiedNodeRightsDecisionV1,
};

pub const MAX_NODE_CONTRIBUTION_LIFETIME_SECS: u64 = 60;
pub const MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES: usize = 16 * 1024;
const NODE_SIGNATURE_BYTES: usize = 64;

/// Provider-private bytes labeled for one custody-selected recipient.
///
/// This contract binds recipient identity, bytes, and commitment. It does not
/// implement or prove encryption; custody integration must construct and audit
/// the cryptographic sealing operation before this type is used in production.
#[derive(Clone, PartialEq, Eq)]
pub struct RecipientSealedContributionV1 {
    recipient: RecipientKeyIdentityV1,
    sealed_bytes: Vec<u8>,
}

impl std::fmt::Debug for RecipientSealedContributionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecipientSealedContributionV1")
            .field("recipient", &self.recipient)
            .field("sealed_bytes", &"[redacted]")
            .finish()
    }
}

impl RecipientSealedContributionV1 {
    pub fn new(
        recipient: RecipientKeyIdentityV1,
        sealed_bytes: Vec<u8>,
    ) -> Result<Self, ContractError> {
        if sealed_bytes.is_empty() || sealed_bytes.len() > MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES {
            return Err(ContractError::InvalidField("recipient_sealed_contribution"));
        }
        Ok(Self {
            recipient,
            sealed_bytes,
        })
    }

    pub fn recipient(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient
    }

    pub fn sealed_bytes(&self) -> &[u8] {
        &self.sealed_bytes
    }

    pub fn commitment(&self) -> Digest32 {
        Digest32::new(Sha256::digest(&self.sealed_bytes).into())
    }
}

impl CanonicalBody for RecipientSealedContributionV1 {
    const DOMAIN: &'static str = "elastos.protected-content.recipient-sealed-contribution/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.recipient.canonical_bytes()?;
        if self.sealed_bytes.is_empty()
            || self.sealed_bytes.len() > MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES
        {
            return Err(ContractError::InvalidField("recipient_sealed_contribution"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.recipient)?;
        encoder.bytes(&self.sealed_bytes)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("recipient")?,
            decoder.bytes(
                "recipient_sealed_contribution",
                MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES,
            )?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeContributionStatementV1 {
    release_request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    signed_rights_decision: SignedNodeRightsDecisionV1,
    recipient_sealed_contribution: RecipientSealedContributionV1,
    issued_at: u64,
    expires_at: u64,
}

impl NodeContributionStatementV1 {
    pub fn new(
        release_request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        signed_rights_decision: SignedNodeRightsDecisionV1,
        recipient_sealed_contribution: RecipientSealedContributionV1,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            release_request_hash,
            binding,
            signed_rights_decision,
            recipient_sealed_contribution,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn signed_rights_decision(&self) -> &SignedNodeRightsDecisionV1 {
        &self.signed_rights_decision
    }

    pub fn recipient_sealed_contribution(&self) -> &RecipientSealedContributionV1 {
        &self.recipient_sealed_contribution
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

pub fn validate_node_contribution_active_window(
    issued_at: u64,
    expires_at: u64,
    request: &VerifiedKeyReleaseRequestV1,
    decision: &VerifiedNodeRightsDecisionV1,
    now: u64,
) -> Result<(), KeyReleaseError> {
    if issued_at < request.issued_at() || expires_at > request.expires_at() {
        return Err(KeyReleaseError::BindingMismatch("node_contribution_window"));
    }
    validate_active(issued_at, expires_at, now)?;
    if issued_at < decision.issued_at() || expires_at > decision.expires_at() {
        return Err(KeyReleaseError::BindingMismatch("node_decision_window"));
    }
    Ok(())
}

impl CanonicalBody for NodeContributionStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.node-contribution/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.signed_rights_decision.canonical_bytes()?;
        self.recipient_sealed_contribution.canonical_bytes()?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_NODE_CONTRIBUTION_LIFETIME_SECS,
            "node_contribution_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.release_request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.nested(&self.signed_rights_decision)?;
        encoder.nested(&self.recipient_sealed_contribution)?;
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            decoder.nested("binding")?,
            decoder.nested("signed_rights_decision")?,
            decoder.nested("recipient_sealed_contribution")?,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedNodeContributionV1 {
    statement: NodeContributionStatementV1,
    node_signature: Vec<u8>,
}

impl SignedNodeContributionV1 {
    pub fn new(
        statement: NodeContributionStatementV1,
        node_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            node_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &NodeContributionStatementV1 {
        &self.statement
    }

    pub fn node_signature(&self) -> &[u8] {
        &self.node_signature
    }

    pub fn verify(
        &self,
        request: &VerifiedKeyReleaseRequestV1,
        node_set: &NodeSetV1,
        now: u64,
    ) -> Result<VerifiedNodeContributionV1, KeyReleaseError> {
        self.canonical_bytes()?;
        let statement = &self.statement;
        if statement.release_request_hash != request.request_hash() {
            return Err(KeyReleaseError::BindingMismatch("key_release_request_hash"));
        }
        if statement.binding != *request.binding() {
            return Err(KeyReleaseError::BindingMismatch(
                "protected_content_binding",
            ));
        }
        if statement.recipient_sealed_contribution.recipient() != request.recipient() {
            return Err(KeyReleaseError::BindingMismatch("recipient_key_identity"));
        }
        let decision = statement
            .signed_rights_decision
            .verify(request, node_set, now)?;
        if decision.decision() != RightsDecisionV1::Allowed {
            return Err(KeyReleaseError::RightsDenied);
        }
        validate_node_contribution_active_window(
            statement.issued_at,
            statement.expires_at,
            request,
            &decision,
            now,
        )?;

        let key =
            validate_ed25519_public_key(*decision.node_public_key().as_bytes(), "node_public_key")
                .map_err(|_| KeyReleaseError::InvalidNodeContributionSignature)?;
        let signature = Signature::from_slice(&self.node_signature)
            .map_err(|_| KeyReleaseError::InvalidNodeContributionSignature)?;
        key.verify_strict(&statement.canonical_bytes()?, &signature)
            .map_err(|_| KeyReleaseError::InvalidNodeContributionSignature)?;
        Ok(VerifiedNodeContributionV1 {
            release_request_hash: statement.release_request_hash,
            recipient: statement.recipient_sealed_contribution.recipient().clone(),
            node_public_key: decision.node_public_key(),
            decision_hash: decision.decision_hash(),
            contribution_hash: self.canonical_hash()?,
            contribution_commitment: statement.recipient_sealed_contribution.commitment(),
            issued_at: statement.issued_at,
            expires_at: statement.expires_at,
        })
    }
}

impl CanonicalBody for SignedNodeContributionV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-node-contribution/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.node_signature.len() != NODE_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("node_contribution_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.node_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("node_contribution")?,
            decoder.bytes("node_contribution_signature", NODE_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedNodeContributionV1 {
    release_request_hash: Digest32,
    recipient: RecipientKeyIdentityV1,
    node_public_key: NodePublicKey,
    decision_hash: Digest32,
    contribution_hash: Digest32,
    contribution_commitment: Digest32,
    issued_at: u64,
    expires_at: u64,
}

impl VerifiedNodeContributionV1 {
    pub const fn release_request_hash(&self) -> Digest32 {
        self.release_request_hash
    }

    pub fn recipient(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn decision_hash(&self) -> Digest32 {
        self.decision_hash
    }

    pub const fn contribution_hash(&self) -> Digest32 {
        self.contribution_hash
    }

    pub const fn contribution_commitment(&self) -> Digest32 {
        self.contribution_commitment
    }

    pub(crate) const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub(crate) const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_contribution_debug_output_redacts_provider_private_bytes() {
        let recipient = RecipientKeyIdentityV1::new(
            "hpke-x25519-hkdf-sha256-aes256gcm",
            Digest32::new([0xa1; 32]),
        )
        .unwrap();
        let sealed =
            RecipientSealedContributionV1::new(recipient, vec![222, 173, 190, 239]).unwrap();
        let debug = format!("{sealed:?}");

        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("222, 173, 190, 239"));
    }
}
