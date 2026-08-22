use ed25519_dalek::{Signature, Verifier as _};
use serde::Serialize;
use thiserror::Error;

use crate::canonical::{validate_ascii_identifier, CanonicalBody, ContractError, Decoder, Encoder};
use crate::{
    CanonicalContract, CustodyNodeIdentityV1, Digest32, KeyEnvelopeIdentityV1, NodePublicKey,
    NodeSetV1, ShareCoordinateV1, ThresholdV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES, MAX_THRESHOLD_NODES,
};

const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CustodyEpochIssuerKeyV1([u8; 32]);

impl CustodyEpochIssuerKeyV1 {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        crate::identity::validate_ed25519_public_key(bytes, "custody_epoch_issuer")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CustodyApprovedSuitesV1 {
    recipient_encryption_suite_id: String,
    stored_share_hpke_suite_id: String,
    released_share_hpke_suite_id: String,
}

impl CustodyApprovedSuitesV1 {
    pub fn new(
        recipient_encryption_suite_id: impl Into<String>,
        stored_share_hpke_suite_id: impl Into<String>,
        released_share_hpke_suite_id: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            recipient_encryption_suite_id: recipient_encryption_suite_id.into(),
            stored_share_hpke_suite_id: stored_share_hpke_suite_id.into(),
            released_share_hpke_suite_id: released_share_hpke_suite_id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn recipient_encryption_suite_id(&self) -> &str {
        &self.recipient_encryption_suite_id
    }

    pub fn stored_share_hpke_suite_id(&self) -> &str {
        &self.stored_share_hpke_suite_id
    }

    pub fn released_share_hpke_suite_id(&self) -> &str {
        &self.released_share_hpke_suite_id
    }
}

impl CanonicalBody for CustodyApprovedSuitesV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-approved-suites/v1";

    fn validate(&self) -> Result<(), ContractError> {
        validate_ascii_identifier(
            &self.recipient_encryption_suite_id,
            "recipient_encryption_suite_id",
            MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
        )?;
        validate_ascii_identifier(
            &self.stored_share_hpke_suite_id,
            "stored_share_hpke_suite_id",
            MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
        )?;
        validate_ascii_identifier(
            &self.released_share_hpke_suite_id,
            "released_share_hpke_suite_id",
            MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
        )?;
        if self.recipient_encryption_suite_id != CUSTODY_X_WING_AES256GCM_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("recipient_encryption_suite_id"));
        }
        if self.stored_share_hpke_suite_id != CUSTODY_X_WING_AES256GCM_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("stored_share_hpke_suite_id"));
        }
        if self.released_share_hpke_suite_id != CUSTODY_X_WING_AES256GCM_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("released_share_hpke_suite_id"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.string(&self.recipient_encryption_suite_id)?;
        encoder.string(&self.stored_share_hpke_suite_id)?;
        encoder.string(&self.released_share_hpke_suite_id)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.string(
                "recipient_encryption_suite_id",
                MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
            )?,
            decoder.string(
                "stored_share_hpke_suite_id",
                MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
            )?,
            decoder.string(
                "released_share_hpke_suite_id",
                MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
            )?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct CustodyEpochIdentityV1 {
    epoch_sha256: Digest32,
    epoch_bytes: u32,
}

impl CustodyEpochIdentityV1 {
    pub fn new(epoch_sha256: Digest32, epoch_bytes: u32) -> Result<Self, ContractError> {
        let value = Self {
            epoch_sha256,
            epoch_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn epoch_sha256(&self) -> Digest32 {
        self.epoch_sha256
    }

    pub const fn epoch_bytes(&self) -> u32 {
        self.epoch_bytes
    }
}

impl CanonicalBody for CustodyEpochIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-epoch-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.epoch_bytes == 0 || self.epoch_bytes > (u16::MAX as u32) {
            return Err(ContractError::InvalidField("epoch_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.epoch_sha256.as_bytes());
        encoder.u32(self.epoch_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u32()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEpochStatementV1 {
    issuer: CustodyEpochIssuerKeyV1,
    approved_suites: CustodyApprovedSuitesV1,
    threshold: ThresholdV1,
    nodes: Vec<CustodyNodeIdentityV1>,
}

impl CustodyEpochStatementV1 {
    pub fn new(
        issuer: CustodyEpochIssuerKeyV1,
        approved_suites: CustodyApprovedSuitesV1,
        threshold: ThresholdV1,
        mut nodes: Vec<CustodyNodeIdentityV1>,
    ) -> Result<Self, ContractError> {
        nodes.sort_unstable_by_key(|node| node.node_public_key());
        for (index, node) in nodes.iter_mut().enumerate() {
            node.share_coordinate = ShareCoordinateV1::new(
                u8::try_from(index + 1)
                    .map_err(|_| ContractError::InvalidField("share_coordinate"))?,
            )?;
        }
        let value = Self {
            issuer,
            approved_suites,
            threshold,
            nodes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub fn approved_suites(&self) -> &CustodyApprovedSuitesV1 {
        &self.approved_suites
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }

    pub fn nodes(&self) -> &[CustodyNodeIdentityV1] {
        &self.nodes
    }

    pub fn node_set(&self) -> Result<NodeSetV1, ContractError> {
        NodeSetV1::new(
            self.threshold,
            self.nodes
                .iter()
                .map(|node| node.node_public_key())
                .collect(),
        )
    }
}

impl CanonicalBody for CustodyEpochStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-epoch-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        CustodyEpochIssuerKeyV1::new(*self.issuer.as_bytes())?;
        self.approved_suites.canonical_bytes()?;
        validate_custody_node_set(&self.nodes, self.threshold, "custody_epoch.nodes")
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.issuer.as_bytes());
        encoder.nested(&self.approved_suites)?;
        self.threshold.encode(encoder);
        encoder.u8(self.nodes.len() as u8);
        for node in &self.nodes {
            encoder.nested(node)?;
        }
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let issuer = CustodyEpochIssuerKeyV1::new(decoder.fixed()?)?;
        let approved_suites = decoder.nested("approved_suites")?;
        let threshold = ThresholdV1::decode(decoder)?;
        let count = usize::from(decoder.u8()?);
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            nodes.push(decoder.nested("custody_node")?);
        }
        Self::new(issuer, approved_suites, threshold, nodes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCustodyEpochV1 {
    statement: CustodyEpochStatementV1,
    issuer_signature: Vec<u8>,
}

impl SignedCustodyEpochV1 {
    pub fn new(
        statement: CustodyEpochStatementV1,
        issuer_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            issuer_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &CustodyEpochStatementV1 {
        &self.statement
    }

    pub fn epoch_identity(&self) -> Result<CustodyEpochIdentityV1, ContractError> {
        CustodyEpochIdentityV1::new(
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::InvalidField("epoch_bytes"))?,
        )
    }

    pub fn verify(&self) -> Result<VerifiedCustodyEpochV1, CustodyEpochError> {
        self.canonical_bytes()?;
        let signature = Signature::from_bytes(
            &self
                .issuer_signature
                .clone()
                .try_into()
                .map_err(|_| CustodyEpochError::InvalidIssuerSignature)?,
        );
        let issuer_key = crate::identity::validate_ed25519_public_key(
            *self.statement.issuer.as_bytes(),
            "custody_epoch_issuer",
        )
        .map_err(|_| CustodyEpochError::InvalidIssuerSignature)?;
        issuer_key
            .verify(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| CustodyEpochError::InvalidIssuerSignature)?;
        Ok(VerifiedCustodyEpochV1 {
            epoch_identity: self.epoch_identity()?,
            issuer: self.statement.issuer,
            approved_suites: self.statement.approved_suites.clone(),
            threshold: self.statement.threshold,
            nodes: self.statement.nodes.clone(),
        })
    }

    pub fn verify_against_key_envelope(
        &self,
        key_envelope: &KeyEnvelopeIdentityV1,
    ) -> Result<VerifiedCustodyEpochV1, CustodyEpochError> {
        let verified = self.verify()?;
        if verified.epoch_identity != key_envelope.custody_epoch() {
            return Err(CustodyEpochError::BindingMismatch("custody_epoch_identity"));
        }
        if verified.node_set_id()? != key_envelope.node_set_id() {
            return Err(CustodyEpochError::BindingMismatch("node_set_id"));
        }
        if verified.threshold != key_envelope.threshold() {
            return Err(CustodyEpochError::BindingMismatch("threshold"));
        }
        Ok(verified)
    }
}

impl CanonicalBody for SignedCustodyEpochV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-custody-epoch/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.issuer_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("issuer_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.issuer_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes("issuer_signature", ED25519_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CustodyEpochError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("custody epoch mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("custody epoch issuer signature is invalid")]
    InvalidIssuerSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCustodyEpochV1 {
    epoch_identity: CustodyEpochIdentityV1,
    issuer: CustodyEpochIssuerKeyV1,
    approved_suites: CustodyApprovedSuitesV1,
    threshold: ThresholdV1,
    nodes: Vec<CustodyNodeIdentityV1>,
}

impl VerifiedCustodyEpochV1 {
    pub const fn epoch_identity(&self) -> CustodyEpochIdentityV1 {
        self.epoch_identity
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub fn approved_suites(&self) -> &CustodyApprovedSuitesV1 {
        &self.approved_suites
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }

    pub fn nodes(&self) -> &[CustodyNodeIdentityV1] {
        &self.nodes
    }

    pub fn node_set_id(&self) -> Result<Digest32, ContractError> {
        NodeSetV1::new(
            self.threshold,
            self.nodes
                .iter()
                .map(|node| node.node_public_key())
                .collect(),
        )?
        .node_set_id()
    }

    pub fn node(&self, node_public_key: NodePublicKey) -> Option<&CustodyNodeIdentityV1> {
        self.nodes
            .binary_search_by_key(&node_public_key, |node| node.node_public_key())
            .ok()
            .map(|index| &self.nodes[index])
    }
}

pub(crate) fn validate_custody_node_set(
    nodes: &[CustodyNodeIdentityV1],
    threshold: ThresholdV1,
    nodes_field: &'static str,
) -> Result<(), ContractError> {
    threshold.validate()?;
    if nodes.len() != usize::from(threshold.total())
        || nodes.len() > usize::from(MAX_THRESHOLD_NODES)
    {
        return Err(ContractError::InvalidField(nodes_field));
    }
    for (index, node) in nodes.iter().enumerate() {
        node.canonical_bytes()?;
        let expected_coordinate = ShareCoordinateV1::new(
            u8::try_from(index + 1).map_err(|_| ContractError::InvalidField("share_coordinate"))?,
        )?;
        if node.share_coordinate() != expected_coordinate {
            return Err(ContractError::InvalidField("share_coordinate"));
        }
        if index > 0 && nodes[index - 1].node_public_key() >= node.node_public_key() {
            return Err(ContractError::InvalidField(nodes_field));
        }
    }
    for (index, node) in nodes.iter().enumerate() {
        if nodes[index + 1..]
            .iter()
            .any(|other| node.custody_public_key() == other.custody_public_key())
        {
            return Err(ContractError::InvalidField("node_custody_public_key"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};
    use hex::encode;

    use super::*;
    use crate::test_support::node_custody_public_key;

    fn node_public_key(seed: u8) -> NodePublicKey {
        let key = SigningKey::from_bytes(&[seed; 32]);
        NodePublicKey::new(key.verifying_key().to_bytes()).unwrap()
    }

    fn custody_public_key(seed: u8) -> crate::NodeCustodyPublicKeyV1 {
        node_custody_public_key(seed)
    }

    fn node(seed: u8, coordinate: u8) -> CustodyNodeIdentityV1 {
        CustodyNodeIdentityV1::new(
            node_public_key(seed),
            custody_public_key(seed),
            ShareCoordinateV1::new(coordinate).unwrap(),
        )
        .unwrap()
    }

    fn signed_epoch() -> SignedCustodyEpochV1 {
        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        let statement = CustodyEpochStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
            CustodyApprovedSuitesV1::new(
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            )
            .unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            vec![node(1, 1), node(2, 2), node(3, 3)],
        )
        .unwrap();
        SignedCustodyEpochV1::new(
            statement.clone(),
            issuer_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn custody_epoch_identity_is_signed_and_stable() {
        let epoch = signed_epoch();
        let verified = epoch.verify().unwrap();
        assert_eq!(verified.threshold(), ThresholdV1::new(2, 3).unwrap());
        assert_eq!(
            encode(epoch.epoch_identity().unwrap().epoch_sha256().as_bytes()),
            "7823e0c872453b579a0c71c99bf94cdbc15fa0dcb1aff70d68dd7ab6c7d2692f"
        );
    }

    #[test]
    fn custody_epoch_rejects_bad_signature_suites_and_duplicate_nodes() {
        let mut epoch = signed_epoch().canonical_bytes().unwrap();
        *epoch.last_mut().unwrap() ^= 1;
        assert_eq!(
            SignedCustodyEpochV1::from_canonical_bytes(&epoch)
                .unwrap()
                .verify(),
            Err(CustodyEpochError::InvalidIssuerSignature)
        );

        assert_eq!(
            CustodyApprovedSuitesV1::new(
                "wrong",
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1
            ),
            Err(ContractError::InvalidField("recipient_encryption_suite_id"))
        );

        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        assert_eq!(
            CustodyEpochStatementV1::new(
                CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
                CustodyApprovedSuitesV1::new(
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                )
                .unwrap(),
                ThresholdV1::new(2, 3).unwrap(),
                vec![node(2, 2), node(1, 1), node(1, 3)],
            ),
            Err(ContractError::InvalidField("custody_epoch.nodes"))
        );
    }
}
