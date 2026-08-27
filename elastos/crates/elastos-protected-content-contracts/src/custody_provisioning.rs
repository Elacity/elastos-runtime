use ed25519_dalek::{Signature, Verifier as _};
use serde::Serialize;
use thiserror::Error;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::rights::{validate_time_window, RIGHTS_CLOCK_SKEW_SECS};
use crate::{
    CanonicalContract, CustodyEnvelopeManifestV1, Digest32, HpkeCiphertextV1,
    KeyEnvelopeIdentityV1, NodePublicKey, NodeSetV1, RuntimeOperationIssuerKeyV1, ThresholdV1,
};

pub const MAX_RUNTIME_CUSTODY_PROVISIONING_LIFETIME_SECS: u64 = 60;
const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RuntimeCustodyProvisioningIdV1(Digest32);

impl RuntimeCustodyProvisioningIdV1 {
    pub fn new(value: Digest32) -> Result<Self, ContractError> {
        if value == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField(
                "runtime_custody_provisioning_id",
            ));
        }
        Ok(Self(value))
    }

    pub const fn digest(&self) -> Digest32 {
        self.0
    }
}

impl CanonicalBody for RuntimeCustodyProvisioningIdV1 {
    const DOMAIN: &'static str = "elastos.protected-content.runtime-custody-provisioning-id/v1";

    fn validate(&self) -> Result<(), ContractError> {
        Self::new(self.0)?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.0.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CustodyNodeProvisioningRecordIdentityV1 {
    record_sha256: Digest32,
    record_bytes: u32,
}

impl CustodyNodeProvisioningRecordIdentityV1 {
    pub fn new(record_sha256: Digest32, record_bytes: u32) -> Result<Self, ContractError> {
        let value = Self {
            record_sha256,
            record_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn record_sha256(&self) -> Digest32 {
        self.record_sha256
    }

    pub const fn record_bytes(&self) -> u32 {
        self.record_bytes
    }
}

impl CanonicalBody for CustodyNodeProvisioningRecordIdentityV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.custody-node-provisioning-record-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.record_sha256 == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField(
                "custody_node_provisioning_record_sha256",
            ));
        }
        if self.record_bytes == 0 {
            return Err(ContractError::InvalidField(
                "custody_node_provisioning_record_bytes",
            ));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.record_sha256.as_bytes());
        encoder.u32(self.record_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u32()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyNodeProvisioningRecordV1 {
    key_envelope_identity: KeyEnvelopeIdentityV1,
    manifest: CustodyEnvelopeManifestV1,
    selected_node_public_key: NodePublicKey,
    sealed_share: HpkeCiphertextV1,
}

impl CustodyNodeProvisioningRecordV1 {
    pub fn new(
        key_envelope_identity: KeyEnvelopeIdentityV1,
        manifest: CustodyEnvelopeManifestV1,
        selected_node_public_key: NodePublicKey,
        sealed_share: HpkeCiphertextV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            key_envelope_identity,
            manifest,
            selected_node_public_key,
            sealed_share,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn key_envelope_identity(&self) -> &KeyEnvelopeIdentityV1 {
        &self.key_envelope_identity
    }

    pub fn manifest(&self) -> &CustodyEnvelopeManifestV1 {
        &self.manifest
    }

    pub const fn selected_node_public_key(&self) -> NodePublicKey {
        self.selected_node_public_key
    }

    pub fn sealed_share(&self) -> &HpkeCiphertextV1 {
        &self.sealed_share
    }

    pub fn node_set(&self) -> Result<NodeSetV1, ContractError> {
        self.manifest.node_set()
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.manifest.threshold()
    }

    pub fn record_identity(
        &self,
    ) -> Result<CustodyNodeProvisioningRecordIdentityV1, ContractError> {
        CustodyNodeProvisioningRecordIdentityV1::new(
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::FieldTooLong("custody_node_provisioning_record"))?,
        )
    }
}

impl CanonicalBody for CustodyNodeProvisioningRecordV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-node-provisioning-record/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.key_envelope_identity.canonical_bytes()?;
        self.manifest.canonical_bytes()?;
        NodePublicKey::new(*self.selected_node_public_key.as_bytes())?;
        self.sealed_share.canonical_bytes()?;
        if self.manifest.node(self.selected_node_public_key).is_none() {
            return Err(ContractError::InvalidField("selected_node_public_key"));
        }
        let node_set = self.manifest.node_set()?;
        if node_set.node_set_id()? != self.key_envelope_identity.node_set_id() {
            return Err(ContractError::InvalidField("key_envelope.node_set_id"));
        }
        if node_set.threshold() != self.key_envelope_identity.threshold() {
            return Err(ContractError::InvalidField("key_envelope.threshold"));
        }
        if self.manifest.encrypted_content() != self.key_envelope_identity.encrypted_content() {
            return Err(ContractError::InvalidField(
                "key_envelope.encrypted_content",
            ));
        }
        if self.manifest.threshold() != self.key_envelope_identity.threshold() {
            return Err(ContractError::InvalidField("key_envelope.threshold"));
        }
        if self.manifest.custody_pool() != self.key_envelope_identity.custody_pool() {
            return Err(ContractError::InvalidField("key_envelope.custody_pool"));
        }
        if self.manifest.custody_epoch() != self.key_envelope_identity.custody_epoch() {
            return Err(ContractError::InvalidField("key_envelope.custody_epoch"));
        }
        if self.manifest.custody_committee_authorization()
            != self.key_envelope_identity.custody_committee_authorization()
        {
            return Err(ContractError::InvalidField(
                "key_envelope.custody_committee_authorization",
            ));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.key_envelope_identity)?;
        encoder.nested(&self.manifest)?;
        encoder.fixed(self.selected_node_public_key.as_bytes());
        encoder.nested(&self.sealed_share)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("key_envelope_identity")?,
            decoder.nested("custody_manifest")?,
            NodePublicKey::new(decoder.fixed()?)?,
            decoder.nested("sealed_share")?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCustodyProvisioningStatementV1 {
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
    provisioning_id: RuntimeCustodyProvisioningIdV1,
    issued_at: u64,
    expires_at: u64,
}

impl RuntimeCustodyProvisioningStatementV1 {
    pub fn new(
        runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        record_identity: CustodyNodeProvisioningRecordIdentityV1,
        provisioning_id: RuntimeCustodyProvisioningIdV1,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            runtime_operation_issuer,
            record_identity,
            provisioning_id,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub const fn record_identity(&self) -> CustodyNodeProvisioningRecordIdentityV1 {
        self.record_identity
    }

    pub const fn provisioning_id(&self) -> RuntimeCustodyProvisioningIdV1 {
        self.provisioning_id
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl CanonicalBody for RuntimeCustodyProvisioningStatementV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.runtime-custody-provisioning-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        RuntimeOperationIssuerKeyV1::new(*self.runtime_operation_issuer.as_bytes())?;
        self.record_identity.canonical_bytes()?;
        self.provisioning_id.canonical_bytes()?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RUNTIME_CUSTODY_PROVISIONING_LIFETIME_SECS,
            "runtime_custody_provisioning_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.runtime_operation_issuer.as_bytes());
        encoder.nested(&self.record_identity)?;
        encoder.nested(&self.provisioning_id)?;
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            RuntimeOperationIssuerKeyV1::new(decoder.fixed()?)?,
            decoder.nested("record_identity")?,
            decoder.nested("provisioning_id")?,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeCustodyProvisioningError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("runtime custody provisioning mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("runtime custody provisioning signature is invalid")]
    InvalidRuntimeSignature,
    #[error("runtime custody provisioning is not yet valid")]
    NotYetValid,
    #[error("runtime custody provisioning expired")]
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRuntimeCustodyProvisioningV1 {
    statement: RuntimeCustodyProvisioningStatementV1,
    runtime_signature: Vec<u8>,
}

impl SignedRuntimeCustodyProvisioningV1 {
    pub fn new(
        statement: RuntimeCustodyProvisioningStatementV1,
        runtime_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            runtime_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &RuntimeCustodyProvisioningStatementV1 {
        &self.statement
    }

    pub fn verify_for_record(
        &self,
        record: &CustodyNodeProvisioningRecordV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now: u64,
    ) -> Result<AuthenticatedRuntimeCustodyProvisioningV1, RuntimeCustodyProvisioningError> {
        self.canonical_bytes()?;
        map_active(self.statement.issued_at, self.statement.expires_at, now)?;
        let record_identity = record.record_identity()?;
        if self.statement.record_identity != record_identity {
            return Err(RuntimeCustodyProvisioningError::BindingMismatch(
                "custody_node_provisioning_record_identity",
            ));
        }
        if self.statement.runtime_operation_issuer != expected_runtime_issuer {
            return Err(RuntimeCustodyProvisioningError::BindingMismatch(
                "runtime_operation_issuer",
            ));
        }
        let signature = Signature::from_bytes(
            &self
                .runtime_signature
                .clone()
                .try_into()
                .map_err(|_| RuntimeCustodyProvisioningError::InvalidRuntimeSignature)?,
        );
        let runtime_key = crate::identity::validate_ed25519_public_key(
            *self.statement.runtime_operation_issuer.as_bytes(),
            "runtime_operation_issuer",
        )
        .map_err(|_| RuntimeCustodyProvisioningError::InvalidRuntimeSignature)?;
        runtime_key
            .verify(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| RuntimeCustodyProvisioningError::InvalidRuntimeSignature)?;
        Ok(AuthenticatedRuntimeCustodyProvisioningV1 {
            statement: self.statement.clone(),
            operation_hash: self.statement.canonical_hash()?,
            record_identity,
        })
    }
}

impl CanonicalBody for SignedRuntimeCustodyProvisioningV1 {
    const DOMAIN: &'static str = "elastos.protected-content.runtime-custody-provisioning/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.runtime_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("runtime_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.runtime_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes("runtime_signature", ED25519_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedRuntimeCustodyProvisioningV1 {
    statement: RuntimeCustodyProvisioningStatementV1,
    operation_hash: Digest32,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
}

impl AuthenticatedRuntimeCustodyProvisioningV1 {
    pub fn statement(&self) -> &RuntimeCustodyProvisioningStatementV1 {
        &self.statement
    }

    pub const fn operation_hash(&self) -> Digest32 {
        self.operation_hash
    }

    pub const fn record_identity(&self) -> CustodyNodeProvisioningRecordIdentityV1 {
        self.record_identity
    }

    pub const fn provisioning_id(&self) -> RuntimeCustodyProvisioningIdV1 {
        self.statement.provisioning_id
    }
}

fn map_active(
    issued_at: u64,
    expires_at: u64,
    now: u64,
) -> Result<(), RuntimeCustodyProvisioningError> {
    if now + RIGHTS_CLOCK_SKEW_SECS < issued_at {
        return Err(RuntimeCustodyProvisioningError::NotYetValid);
    }
    if now >= expires_at {
        return Err(RuntimeCustodyProvisioningError::Expired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signer as _;

    use super::*;
    use crate::{
        CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1, CustodyNodeIdentityV1,
        CustodyPoolIdentityV1, EncryptedContentIdentityV1, NodeCustodyPublicKeyV1,
        ShareCoordinateV1, HPKE_ENCAPPED_KEY_BYTES, HPKE_SEALED_SHARE_BYTES,
    };

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn runtime_key() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[0x71; 32])
    }

    fn node_key(seed: u8) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        NodePublicKey::new(node_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        NodeCustodyPublicKeyV1::new([seed; 32]).unwrap()
    }

    fn encrypted_content(seed: u8) -> EncryptedContentIdentityV1 {
        EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap()
    }

    fn custody_pool(seed: u8) -> CustodyPoolIdentityV1 {
        CustodyPoolIdentityV1::new(digest(seed), 512).unwrap()
    }

    fn custody_epoch(seed: u8) -> CustodyEpochIdentityV1 {
        CustodyEpochIdentityV1::new(digest(seed), 512).unwrap()
    }

    fn committee_authorization(seed: u8) -> CustodyCommitteeAuthorizationIdentityV1 {
        CustodyCommitteeAuthorizationIdentityV1::new(digest(seed), 512).unwrap()
    }

    fn nodes() -> Vec<CustodyNodeIdentityV1> {
        vec![
            CustodyNodeIdentityV1::new(
                node_public_key(1),
                custody_public_key(3),
                ShareCoordinateV1::new(1).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(2),
                custody_public_key(4),
                ShareCoordinateV1::new(2).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(3),
                custody_public_key(5),
                ShareCoordinateV1::new(3).unwrap(),
            )
            .unwrap(),
        ]
    }

    fn manifest_with(
        content: EncryptedContentIdentityV1,
        pool: CustodyPoolIdentityV1,
        epoch: CustodyEpochIdentityV1,
        authorization: CustodyCommitteeAuthorizationIdentityV1,
        threshold: ThresholdV1,
        node_entries: Vec<CustodyNodeIdentityV1>,
    ) -> CustodyEnvelopeManifestV1 {
        CustodyEnvelopeManifestV1::new(
            content,
            pool,
            epoch,
            authorization,
            threshold,
            digest(0x44),
            node_entries,
        )
        .unwrap()
    }

    fn manifest() -> CustodyEnvelopeManifestV1 {
        manifest_with(
            encrypted_content(0x11),
            custody_pool(0x35),
            custody_epoch(0x33),
            committee_authorization(0x36),
            ThresholdV1::new(2, 3).unwrap(),
            nodes(),
        )
    }

    fn key_envelope_for(manifest: &CustodyEnvelopeManifestV1) -> KeyEnvelopeIdentityV1 {
        KeyEnvelopeIdentityV1::new(
            manifest.encrypted_content().clone(),
            digest(0x22),
            512,
            manifest.node_set().unwrap().node_set_id().unwrap(),
            manifest.threshold(),
            manifest.custody_pool(),
            manifest.custody_epoch(),
            manifest.custody_committee_authorization(),
        )
        .unwrap()
    }

    fn sealed_share(seed: u8) -> HpkeCiphertextV1 {
        let mut encapped = [seed; HPKE_ENCAPPED_KEY_BYTES];
        encapped[0] = 9;
        HpkeCiphertextV1::new(encapped, [seed ^ 0x55; HPKE_SEALED_SHARE_BYTES]).unwrap()
    }

    fn record() -> CustodyNodeProvisioningRecordV1 {
        let manifest = manifest();
        CustodyNodeProvisioningRecordV1::new(
            key_envelope_for(&manifest),
            manifest,
            node_public_key(1),
            sealed_share(0x61),
        )
        .unwrap()
    }

    fn provisioning_id() -> RuntimeCustodyProvisioningIdV1 {
        RuntimeCustodyProvisioningIdV1::new(digest(0x81)).unwrap()
    }

    fn signed_for(
        record: &CustodyNodeProvisioningRecordV1,
        issued_at: u64,
        expires_at: u64,
    ) -> SignedRuntimeCustodyProvisioningV1 {
        let runtime = runtime_key();
        let statement = RuntimeCustodyProvisioningStatementV1::new(
            RuntimeOperationIssuerKeyV1::new(runtime.verifying_key().to_bytes()).unwrap(),
            record.record_identity().unwrap(),
            provisioning_id(),
            issued_at,
            expires_at,
        )
        .unwrap();
        SignedRuntimeCustodyProvisioningV1::new(
            statement.clone(),
            runtime
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn custody_node_provisioning_record_round_trips_and_has_golden_identity() {
        let record = record();
        let canonical = record.canonical_bytes().unwrap();
        let decoded = CustodyNodeProvisioningRecordV1::from_canonical_bytes(&canonical).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(
            hex::encode(record.record_identity().unwrap().record_sha256().as_bytes()),
            "31f8d2a51bb3e2f1dbaa2e12c51d97eb1086a056ae5e12aa4244dc909a54e64e"
        );
        assert_eq!(record.record_identity().unwrap().record_bytes(), 1533);
        assert_eq!(decoded.selected_node_public_key(), node_public_key(1));
        assert_eq!(
            decoded.node_set().unwrap().threshold(),
            ThresholdV1::new(2, 3).unwrap()
        );
    }

    #[test]
    fn runtime_custody_provisioning_round_trips_verifies_and_has_golden_signature() {
        let record = record();
        let signed = signed_for(&record, 2_000_000_000, 2_000_000_060);
        let canonical = signed.canonical_bytes().unwrap();
        let decoded = SignedRuntimeCustodyProvisioningV1::from_canonical_bytes(&canonical).unwrap();
        let authenticated = decoded
            .verify_for_record(
                &record,
                signed.statement().runtime_operation_issuer(),
                2_000_000_010,
            )
            .unwrap();
        assert_eq!(
            authenticated.record_identity(),
            record.record_identity().unwrap()
        );
        assert_eq!(authenticated.provisioning_id(), provisioning_id());
        assert_eq!(
            hex::encode(signed.runtime_signature),
            "2d458f1ec4c5d1a533680ff87a7459f79ee1e5db1f824a8ab8264c0f4d705da928a7b781d6f34a72e381854e1f6e086ec98f9160b53ea44af9296ab15ff13a0d"
        );
        assert_eq!(
            hex::encode(authenticated.operation_hash().as_bytes()),
            "74951bb7515c63704a1514b2f17175d37a56ce5192c90d87225d60cfe597948a"
        );
    }

    #[test]
    fn custody_node_provisioning_record_rejects_authority_substitution() {
        let base_manifest = manifest();
        let cases = [
            (
                "content",
                manifest_with(
                    encrypted_content(0x12),
                    base_manifest.custody_pool(),
                    base_manifest.custody_epoch(),
                    base_manifest.custody_committee_authorization(),
                    base_manifest.threshold(),
                    nodes(),
                ),
            ),
            (
                "pool",
                manifest_with(
                    base_manifest.encrypted_content().clone(),
                    custody_pool(0x36),
                    base_manifest.custody_epoch(),
                    base_manifest.custody_committee_authorization(),
                    base_manifest.threshold(),
                    nodes(),
                ),
            ),
            (
                "epoch",
                manifest_with(
                    base_manifest.encrypted_content().clone(),
                    base_manifest.custody_pool(),
                    custody_epoch(0x34),
                    base_manifest.custody_committee_authorization(),
                    base_manifest.threshold(),
                    nodes(),
                ),
            ),
            (
                "committee_authorization",
                manifest_with(
                    base_manifest.encrypted_content().clone(),
                    base_manifest.custody_pool(),
                    base_manifest.custody_epoch(),
                    committee_authorization(0x37),
                    base_manifest.threshold(),
                    nodes(),
                ),
            ),
            (
                "threshold",
                manifest_with(
                    base_manifest.encrypted_content().clone(),
                    base_manifest.custody_pool(),
                    base_manifest.custody_epoch(),
                    base_manifest.custody_committee_authorization(),
                    ThresholdV1::new(2, 2).unwrap(),
                    nodes()[..2].to_vec(),
                ),
            ),
        ];

        let key_envelope = key_envelope_for(&base_manifest);
        for (label, substituted_manifest) in cases {
            assert!(
                CustodyNodeProvisioningRecordV1::new(
                    key_envelope.clone(),
                    substituted_manifest,
                    node_public_key(1),
                    sealed_share(0x61),
                )
                .is_err(),
                "{label}"
            );
        }

        let mut node_entries = nodes();
        node_entries[0] = CustodyNodeIdentityV1::new(
            node_public_key(4),
            custody_public_key(6),
            ShareCoordinateV1::new(1).unwrap(),
        )
        .unwrap();
        let substituted_node_set = manifest_with(
            base_manifest.encrypted_content().clone(),
            base_manifest.custody_pool(),
            base_manifest.custody_epoch(),
            base_manifest.custody_committee_authorization(),
            base_manifest.threshold(),
            node_entries,
        );
        assert!(CustodyNodeProvisioningRecordV1::new(
            key_envelope,
            substituted_node_set,
            node_public_key(1),
            sealed_share(0x61),
        )
        .is_err());
    }

    #[test]
    fn custody_node_provisioning_record_rejects_unknown_selected_node_and_tamper() {
        let manifest = manifest();
        let key_envelope = key_envelope_for(&manifest);
        assert!(CustodyNodeProvisioningRecordV1::new(
            key_envelope,
            manifest,
            node_public_key(4),
            sealed_share(0x61),
        )
        .is_err());

        let mut canonical = record().canonical_bytes().unwrap();
        canonical.push(0);
        assert!(CustodyNodeProvisioningRecordV1::from_canonical_bytes(&canonical).is_err());
    }

    #[test]
    fn signed_runtime_custody_provisioning_rejects_mismatched_record_and_signature() {
        let record = record();
        let signed = signed_for(&record, 2_000_000_000, 2_000_000_060);

        let other_manifest = manifest_with(
            encrypted_content(0x11),
            custody_pool(0x39),
            custody_epoch(0x33),
            committee_authorization(0x36),
            ThresholdV1::new(2, 3).unwrap(),
            nodes(),
        );
        let other = CustodyNodeProvisioningRecordV1::new(
            key_envelope_for(&other_manifest),
            other_manifest,
            node_public_key(1),
            sealed_share(0x61),
        )
        .unwrap();
        assert_eq!(
            signed.verify_for_record(
                &other,
                signed.statement().runtime_operation_issuer(),
                2_000_000_010,
            ),
            Err(RuntimeCustodyProvisioningError::BindingMismatch(
                "custody_node_provisioning_record_identity"
            ))
        );

        let mut bytes = signed.canonical_bytes().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let tampered = SignedRuntimeCustodyProvisioningV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(
            tampered.verify_for_record(
                &record,
                signed.statement().runtime_operation_issuer(),
                2_000_000_010,
            ),
            Err(RuntimeCustodyProvisioningError::InvalidRuntimeSignature)
        );

        let wrong_runtime_issuer = RuntimeOperationIssuerKeyV1::new(
            ed25519_dalek::SigningKey::from_bytes(&[0x79; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(
            signed.verify_for_record(&record, wrong_runtime_issuer, 2_000_000_010),
            Err(RuntimeCustodyProvisioningError::BindingMismatch(
                "runtime_operation_issuer"
            ))
        );
    }

    #[test]
    fn signed_runtime_custody_provisioning_rejects_expiry_and_future_time() {
        let record = record();
        assert_eq!(
            signed_for(&record, 2_000_000_000, 2_000_000_060).verify_for_record(
                &record,
                signed_for(&record, 2_000_000_000, 2_000_000_060)
                    .statement()
                    .runtime_operation_issuer(),
                2_000_000_060,
            ),
            Err(RuntimeCustodyProvisioningError::Expired)
        );
        assert_eq!(
            signed_for(&record, 2_000_000_100, 2_000_000_160).verify_for_record(
                &record,
                signed_for(&record, 2_000_000_100, 2_000_000_160)
                    .statement()
                    .runtime_operation_issuer(),
                2_000_000_000,
            ),
            Err(RuntimeCustodyProvisioningError::NotYetValid)
        );
    }

    #[test]
    fn runtime_custody_provisioning_id_rejects_zero_and_binds_replay_identity() {
        assert_eq!(
            RuntimeCustodyProvisioningIdV1::new(Digest32::new([0; 32])),
            Err(ContractError::InvalidField(
                "runtime_custody_provisioning_id"
            ))
        );
        let record = record();
        let signed = signed_for(&record, 2_000_000_000, 2_000_000_060);
        assert_eq!(signed.statement().provisioning_id(), provisioning_id());
    }
}
