use std::collections::HashSet;

use ed25519_dalek::Signature;
use serde::Serialize;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::rights::{validate_active, validate_time_window};
use crate::{
    CanonicalContract, Digest32, KeyReleaseError, NodePublicKey, ProtectedContentBindingV1,
    VerifiedKeyReleaseRequestV1, VerifiedNodeContributionV1,
};

pub const MAX_TERMINAL_RECEIPT_LIFETIME_SECS: u64 = 60;
const ISSUER_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TerminalReceiptIssuerKey([u8; 32]);

impl TerminalReceiptIssuerKey {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_ed25519_public_key(bytes, "terminal_receipt_issuer")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(u8)]
pub enum KeyReleaseOutcomeV1 {
    Denied = 0,
    Released = 1,
}

impl KeyReleaseOutcomeV1 {
    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            0 => Ok(Self::Denied),
            1 => Ok(Self::Released),
            _ => Err(ContractError::InvalidField("key_release_outcome")),
        }
    }
}

/// Capsule-visible reference to provider-private node evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct NodeContributionRefV1 {
    node_public_key: NodePublicKey,
    decision_hash: Digest32,
    contribution_hash: Digest32,
    contribution_commitment: Digest32,
}

impl NodeContributionRefV1 {
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
}

impl From<&VerifiedNodeContributionV1> for NodeContributionRefV1 {
    fn from(value: &VerifiedNodeContributionV1) -> Self {
        Self {
            node_public_key: value.node_public_key(),
            decision_hash: value.decision_hash(),
            contribution_hash: value.contribution_hash(),
            contribution_commitment: value.contribution_commitment(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalReceiptStatementV1 {
    release_request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    issuer: TerminalReceiptIssuerKey,
    outcome: KeyReleaseOutcomeV1,
    contribution_refs: Vec<NodeContributionRefV1>,
    issued_at: u64,
    expires_at: u64,
}

impl TerminalReceiptStatementV1 {
    pub fn new(
        release_request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        issuer: TerminalReceiptIssuerKey,
        outcome: KeyReleaseOutcomeV1,
        mut contribution_refs: Vec<NodeContributionRefV1>,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        contribution_refs.sort_unstable();
        let value = Self {
            release_request_hash,
            binding,
            issuer,
            outcome,
            contribution_refs,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn issuer(&self) -> TerminalReceiptIssuerKey {
        self.issuer
    }

    pub const fn outcome(&self) -> KeyReleaseOutcomeV1 {
        self.outcome
    }

    pub fn contribution_refs(&self) -> &[NodeContributionRefV1] {
        &self.contribution_refs
    }
}

impl CanonicalBody for TerminalReceiptStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.terminal-receipt-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        TerminalReceiptIssuerKey::new(*self.issuer.as_bytes())?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_TERMINAL_RECEIPT_LIFETIME_SECS,
            "terminal_receipt_lifetime",
        )?;
        if self
            .contribution_refs
            .windows(2)
            .any(|window| window[0] >= window[1])
            || self.contribution_refs.len()
                > usize::from(self.binding.key_envelope().threshold().total())
        {
            return Err(ContractError::InvalidField("node_contribution_refs"));
        }
        let mut nodes = HashSet::with_capacity(self.contribution_refs.len());
        let mut decisions = HashSet::with_capacity(self.contribution_refs.len());
        let mut contributions = HashSet::with_capacity(self.contribution_refs.len());
        let mut commitments = HashSet::with_capacity(self.contribution_refs.len());
        if self.contribution_refs.iter().any(|reference| {
            !nodes.insert(reference.node_public_key)
                || !decisions.insert(reference.decision_hash)
                || !contributions.insert(reference.contribution_hash)
                || !commitments.insert(reference.contribution_commitment)
        }) {
            return Err(ContractError::InvalidField("node_contribution_refs"));
        }
        match self.outcome {
            KeyReleaseOutcomeV1::Denied if !self.contribution_refs.is_empty() => {
                Err(ContractError::InvalidField("denied_contribution_refs"))
            }
            KeyReleaseOutcomeV1::Released
                if self.contribution_refs.len()
                    != usize::from(self.binding.key_envelope().threshold().required()) =>
            {
                Err(ContractError::InvalidField("release_threshold"))
            }
            _ => Ok(()),
        }
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.release_request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.fixed(self.issuer.as_bytes());
        encoder.u8(self.outcome as u8);
        encoder.u8(self.contribution_refs.len() as u8);
        for reference in &self.contribution_refs {
            encoder.fixed(reference.node_public_key.as_bytes());
            encoder.fixed(reference.decision_hash.as_bytes());
            encoder.fixed(reference.contribution_hash.as_bytes());
            encoder.fixed(reference.contribution_commitment.as_bytes());
        }
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let release_request_hash = Digest32::new(decoder.fixed()?);
        let binding: ProtectedContentBindingV1 = decoder.nested("binding")?;
        let issuer = TerminalReceiptIssuerKey::new(decoder.fixed()?)?;
        let outcome = KeyReleaseOutcomeV1::decode(decoder.u8()?)?;
        let count = usize::from(decoder.u8()?);
        if count > usize::from(binding.key_envelope().threshold().total()) {
            return Err(ContractError::InvalidField("node_contribution_refs"));
        }
        let mut contribution_refs = Vec::with_capacity(count);
        for _ in 0..count {
            contribution_refs.push(NodeContributionRefV1 {
                node_public_key: NodePublicKey::new(decoder.fixed()?)?,
                decision_hash: Digest32::new(decoder.fixed()?),
                contribution_hash: Digest32::new(decoder.fixed()?),
                contribution_commitment: Digest32::new(decoder.fixed()?),
            });
        }
        Self::new(
            release_request_hash,
            binding,
            issuer,
            outcome,
            contribution_refs,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedTerminalReceiptV1 {
    statement: TerminalReceiptStatementV1,
    issuer_signature: Vec<u8>,
}

impl SignedTerminalReceiptV1 {
    pub fn new(
        statement: TerminalReceiptStatementV1,
        issuer_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            issuer_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &TerminalReceiptStatementV1 {
        &self.statement
    }

    pub fn issuer_signature(&self) -> &[u8] {
        &self.issuer_signature
    }

    pub fn verify(
        &self,
        request: &VerifiedKeyReleaseRequestV1,
        contributions: &[VerifiedNodeContributionV1],
        expected_issuer: TerminalReceiptIssuerKey,
        now: u64,
    ) -> Result<(), KeyReleaseError> {
        self.canonical_bytes()?;
        let statement = &self.statement;
        if statement.issuer != expected_issuer {
            return Err(KeyReleaseError::UnexpectedTerminalIssuer);
        }
        if statement.release_request_hash != request.request_hash() {
            return Err(KeyReleaseError::BindingMismatch("key_release_request_hash"));
        }
        if statement.binding != *request.binding() {
            return Err(KeyReleaseError::BindingMismatch(
                "protected_content_binding",
            ));
        }
        if statement.issued_at < request.issued_at() || statement.expires_at > request.expires_at()
        {
            return Err(KeyReleaseError::BindingMismatch("terminal_receipt_window"));
        }
        validate_active(statement.issued_at, statement.expires_at, now)?;
        if contributions
            .iter()
            .any(|contribution| contribution.release_request_hash() != request.request_hash())
        {
            return Err(KeyReleaseError::BindingMismatch("key_release_request_hash"));
        }
        if contributions
            .iter()
            .any(|contribution| contribution.recipient() != request.recipient())
        {
            return Err(KeyReleaseError::BindingMismatch("recipient_key_identity"));
        }
        if contributions.iter().any(|contribution| {
            statement.issued_at < contribution.issued_at()
                || statement.expires_at > contribution.expires_at()
        }) {
            return Err(KeyReleaseError::BindingMismatch("node_contribution_window"));
        }
        let mut expected_refs = contributions
            .iter()
            .map(NodeContributionRefV1::from)
            .collect::<Vec<_>>();
        expected_refs.sort_unstable();
        if expected_refs != statement.contribution_refs {
            return Err(KeyReleaseError::BindingMismatch("node_contribution_refs"));
        }
        if statement.outcome == KeyReleaseOutcomeV1::Released {
            let required = usize::from(request.binding().key_envelope().threshold().required());
            if expected_refs.len() < required {
                return Err(KeyReleaseError::InsufficientContributions);
            }
            if expected_refs.len() != required {
                return Err(KeyReleaseError::BindingMismatch("release_threshold"));
            }
        }
        let key =
            validate_ed25519_public_key(*statement.issuer.as_bytes(), "terminal_receipt_issuer")
                .map_err(|_| KeyReleaseError::InvalidTerminalSignature)?;
        let signature = Signature::from_slice(&self.issuer_signature)
            .map_err(|_| KeyReleaseError::InvalidTerminalSignature)?;
        key.verify_strict(&statement.canonical_bytes()?, &signature)
            .map_err(|_| KeyReleaseError::InvalidTerminalSignature)
    }
}

impl CanonicalBody for SignedTerminalReceiptV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-terminal-receipt/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.issuer_signature.len() != ISSUER_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("terminal_receipt_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.issuer_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("terminal_receipt_statement")?,
            decoder.bytes("terminal_receipt_signature", ISSUER_SIGNATURE_BYTES)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    use crate::test_support::{binding_for_wallet, digest, node_public_key, wallet};

    fn issuer() -> TerminalReceiptIssuerKey {
        TerminalReceiptIssuerKey::new(
            SigningKey::from_bytes(&[0x21; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn statement_with_refs(
        refs: Vec<NodeContributionRefV1>,
        outcome: KeyReleaseOutcomeV1,
    ) -> Result<TerminalReceiptStatementV1, ContractError> {
        TerminalReceiptStatementV1::new(
            digest(0xa0),
            binding_for_wallet(wallet(7)),
            issuer(),
            outcome,
            refs,
            2_000_000_010,
            2_000_000_040,
        )
    }

    fn reference(
        node_seed: u8,
        decision_seed: u8,
        contribution_seed: u8,
        commitment_seed: u8,
    ) -> NodeContributionRefV1 {
        NodeContributionRefV1 {
            node_public_key: node_public_key(node_seed),
            decision_hash: digest(decision_seed),
            contribution_hash: digest(contribution_seed),
            contribution_commitment: digest(commitment_seed),
        }
    }

    #[test]
    fn terminal_receipt_requires_threshold_for_released_outcome() {
        let err = statement_with_refs(
            vec![reference(1, 0x31, 0x41, 0x51)],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("release_threshold"));
    }

    #[test]
    fn terminal_receipt_rejects_more_than_required_refs_for_released_outcome() {
        let err = statement_with_refs(
            vec![
                reference(1, 0x31, 0x41, 0x51),
                reference(2, 0x32, 0x42, 0x52),
                reference(3, 0x33, 0x43, 0x53),
            ],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("release_threshold"));
    }

    #[test]
    fn terminal_receipt_rejects_duplicate_node_reference() {
        let err = statement_with_refs(
            vec![
                reference(1, 0x31, 0x41, 0x51),
                reference(1, 0x32, 0x42, 0x52),
            ],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("node_contribution_refs"));
    }

    #[test]
    fn terminal_receipt_rejects_duplicate_decision_reference() {
        let err = statement_with_refs(
            vec![
                reference(1, 0x31, 0x41, 0x51),
                reference(2, 0x31, 0x42, 0x52),
            ],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("node_contribution_refs"));
    }

    #[test]
    fn terminal_receipt_rejects_duplicate_contribution_reference() {
        let err = statement_with_refs(
            vec![
                reference(1, 0x31, 0x41, 0x51),
                reference(2, 0x32, 0x41, 0x52),
            ],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("node_contribution_refs"));
    }

    #[test]
    fn terminal_receipt_rejects_duplicate_contribution_commitment() {
        let err = statement_with_refs(
            vec![
                reference(1, 0x31, 0x41, 0x51),
                reference(2, 0x32, 0x42, 0x51),
            ],
            KeyReleaseOutcomeV1::Released,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::InvalidField("node_contribution_refs"));
    }
}
