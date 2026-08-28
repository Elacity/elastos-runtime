use ed25519_dalek::Signature;
use serde::Serialize;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::rights::{validate_active, validate_time_window};
use crate::{
    CanonicalContract, Digest32, KeyReleaseError, NodePublicKey, NodeSetV1,
    ProtectedContentBindingV1, RightsActionV1, RightsDecisionV1, VerifiedKeyReleaseRequestV1,
};

pub const MAX_NODE_DECISION_LIFETIME_SECS: u64 = 60;
const NODE_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeRightsDecisionStatementV1 {
    release_request_hash: Digest32,
    rights_request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    node_public_key: NodePublicKey,
    decision: RightsDecisionV1,
    evidence_hash: Digest32,
    issued_at: u64,
    expires_at: u64,
}

impl NodeRightsDecisionStatementV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        release_request_hash: Digest32,
        rights_request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        action: RightsActionV1,
        node_public_key: NodePublicKey,
        decision: RightsDecisionV1,
        evidence_hash: Digest32,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            release_request_hash,
            rights_request_hash,
            binding,
            action,
            node_public_key,
            decision,
            evidence_hash,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn release_request_hash(&self) -> Digest32 {
        self.release_request_hash
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn decision(&self) -> RightsDecisionV1 {
        self.decision
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl CanonicalBody for NodeRightsDecisionStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.node-rights-decision/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        NodePublicKey::new(*self.node_public_key.as_bytes())?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_NODE_DECISION_LIFETIME_SECS,
            "node_rights_decision_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.release_request_hash.as_bytes());
        encoder.fixed(self.rights_request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.u8(self.action as u8);
        encoder.fixed(self.node_public_key.as_bytes());
        encoder.u8(self.decision as u8);
        encoder.fixed(self.evidence_hash.as_bytes());
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            Digest32::new(decoder.fixed()?),
            decoder.nested("binding")?,
            RightsActionV1::decode(decoder.u8()?)?,
            NodePublicKey::new(decoder.fixed()?)?,
            RightsDecisionV1::decode(decoder.u8()?)?,
            Digest32::new(decoder.fixed()?),
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedNodeRightsDecisionV1 {
    statement: NodeRightsDecisionStatementV1,
    node_signature: Vec<u8>,
}

impl SignedNodeRightsDecisionV1 {
    pub fn new(
        statement: NodeRightsDecisionStatementV1,
        node_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            node_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &NodeRightsDecisionStatementV1 {
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
    ) -> Result<VerifiedNodeRightsDecisionV1, KeyReleaseError> {
        self.canonical_bytes()?;
        let statement = &self.statement;
        if node_set.node_set_id()? != request.binding().key_envelope().node_set_id() {
            return Err(KeyReleaseError::BindingMismatch("node_set_id"));
        }
        if node_set.threshold() != request.binding().key_envelope().threshold() {
            return Err(KeyReleaseError::BindingMismatch("threshold"));
        }
        if !node_set.contains(statement.node_public_key) {
            return Err(KeyReleaseError::UnknownNode);
        }
        if statement.release_request_hash != request.request_hash() {
            return Err(KeyReleaseError::BindingMismatch("key_release_request_hash"));
        }
        if statement.rights_request_hash != request.rights_request_hash() {
            return Err(KeyReleaseError::BindingMismatch("rights_request_hash"));
        }
        if statement.binding != *request.binding() {
            return Err(KeyReleaseError::BindingMismatch(
                "protected_content_binding",
            ));
        }
        if statement.action != request.action() {
            return Err(KeyReleaseError::BindingMismatch("rights_action"));
        }
        if statement.issued_at < request.issued_at() || statement.expires_at > request.expires_at()
        {
            return Err(KeyReleaseError::BindingMismatch("node_decision_window"));
        }
        validate_active(statement.issued_at, statement.expires_at, now)?;
        let key =
            validate_ed25519_public_key(*statement.node_public_key.as_bytes(), "node_public_key")
                .map_err(|_| KeyReleaseError::InvalidNodeDecisionSignature)?;
        let signature = Signature::from_slice(&self.node_signature)
            .map_err(|_| KeyReleaseError::InvalidNodeDecisionSignature)?;
        key.verify_strict(&statement.canonical_bytes()?, &signature)
            .map_err(|_| KeyReleaseError::InvalidNodeDecisionSignature)?;
        Ok(VerifiedNodeRightsDecisionV1 {
            decision_hash: self.canonical_hash()?,
            node_public_key: statement.node_public_key,
            decision: statement.decision,
            issued_at: statement.issued_at,
            expires_at: statement.expires_at,
        })
    }
}

impl CanonicalBody for SignedNodeRightsDecisionV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-node-rights-decision/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.node_signature.len() != NODE_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("node_decision_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.node_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("node_rights_decision")?,
            decoder.bytes("node_decision_signature", NODE_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedNodeRightsDecisionV1 {
    decision_hash: Digest32,
    node_public_key: NodePublicKey,
    decision: RightsDecisionV1,
    issued_at: u64,
    expires_at: u64,
}

impl VerifiedNodeRightsDecisionV1 {
    pub const fn decision_hash(&self) -> Digest32 {
        self.decision_hash
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn decision(&self) -> RightsDecisionV1 {
        self.decision
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}
