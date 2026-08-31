use ed25519_dalek::Signature;
use serde::Serialize;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::rights::{validate_active, validate_time_window};
use crate::{
    CanonicalContract, Digest32, ProtectedContentBindingV1, RightsActionV1, RightsDecisionV1,
    RightsError, VerifiedRightsRequestV1,
};

pub const MAX_RIGHTS_RECEIPT_LIFETIME_SECS: u64 = 5 * 60;
const ISSUER_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RightsReceiptIssuerKey([u8; 32]);

impl RightsReceiptIssuerKey {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_ed25519_public_key(bytes, "rights_receipt_issuer")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Signed preliminary evidence for Runtime audit. Release verification never
/// consumes this receipt; nodes sign their own independent decisions instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsReceiptStatementV1 {
    request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    issuer: RightsReceiptIssuerKey,
    decision: RightsDecisionV1,
    evidence_hash: Digest32,
    issued_at: u64,
    expires_at: u64,
}

impl RightsReceiptStatementV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        action: RightsActionV1,
        issuer: RightsReceiptIssuerKey,
        decision: RightsDecisionV1,
        evidence_hash: Digest32,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            request_hash,
            binding,
            action,
            issuer,
            decision,
            evidence_hash,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn issuer(&self) -> RightsReceiptIssuerKey {
        self.issuer
    }

    pub const fn decision(&self) -> RightsDecisionV1 {
        self.decision
    }
}

impl CanonicalBody for RightsReceiptStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-receipt-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        RightsReceiptIssuerKey::new(self.issuer.0)?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RIGHTS_RECEIPT_LIFETIME_SECS,
            "rights_receipt_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.u8(self.action as u8);
        encoder.fixed(self.issuer.as_bytes());
        encoder.u8(self.decision as u8);
        encoder.fixed(self.evidence_hash.as_bytes());
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            decoder.nested("binding")?,
            RightsActionV1::decode(decoder.u8()?)?,
            RightsReceiptIssuerKey::new(decoder.fixed()?)?,
            RightsDecisionV1::decode(decoder.u8()?)?,
            Digest32::new(decoder.fixed()?),
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedRightsReceiptV1 {
    statement: RightsReceiptStatementV1,
    issuer_signature: Vec<u8>,
}

impl SignedRightsReceiptV1 {
    pub fn new(
        statement: RightsReceiptStatementV1,
        issuer_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            issuer_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &RightsReceiptStatementV1 {
        &self.statement
    }

    pub fn verify_audit(
        &self,
        request: &VerifiedRightsRequestV1,
        expected_issuer: RightsReceiptIssuerKey,
        now: u64,
    ) -> Result<(), RightsError> {
        self.canonical_bytes()?;
        let statement = &self.statement;
        if statement.issuer != expected_issuer {
            return Err(RightsError::UnexpectedReceiptIssuer);
        }
        if statement.request_hash != request.request_hash() {
            return Err(RightsError::BindingMismatch("rights_request_hash"));
        }
        if statement.binding != *request.binding() {
            return Err(RightsError::BindingMismatch("protected_content_binding"));
        }
        if statement.action != request.action() {
            return Err(RightsError::BindingMismatch("rights_action"));
        }
        if statement.issued_at < request.issued_at() || statement.expires_at > request.expires_at()
        {
            return Err(RightsError::BindingMismatch("rights_receipt_window"));
        }
        validate_active(statement.issued_at, statement.expires_at, now)?;
        let key =
            validate_ed25519_public_key(*statement.issuer.as_bytes(), "rights_receipt_issuer")
                .map_err(|_| RightsError::InvalidReceiptSignature)?;
        let signature = Signature::from_slice(&self.issuer_signature)
            .map_err(|_| RightsError::InvalidReceiptSignature)?;
        key.verify_strict(&statement.canonical_bytes()?, &signature)
            .map_err(|_| RightsError::InvalidReceiptSignature)
    }
}

impl CanonicalBody for SignedRightsReceiptV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-rights-receipt/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.issuer_signature.len() != ISSUER_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("rights_receipt_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.issuer_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("rights_receipt_statement")?,
            decoder.bytes("rights_receipt_signature", ISSUER_SIGNATURE_BYTES)?,
        )
    }
}
