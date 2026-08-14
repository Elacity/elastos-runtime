use curve25519_dalek::{constants::X25519_LOW_ORDER_POINTS, montgomery::MontgomeryPoint};

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::{
    CanonicalContract, Digest32, EncryptedContentIdentityV1, KeyEnvelopeIdentityV1, NodePublicKey,
    NodeSetV1, ProtectedContentBindingV1, RecipientKeyIdentityV1, ThresholdV1, MAX_THRESHOLD_NODES,
};

pub const CUSTODY_HPKE_SUITE_ID_V1: &str = "hpke-rfc9180-base-x25519-hkdf-sha256-aes256gcm/v1";
pub const STORED_SHARE_HPKE_INFO_V1: &[u8] = b"elastos.protected-content.stored-share/v1";
pub const RELEASED_SHARE_HPKE_INFO_V1: &[u8] = b"elastos.protected-content.released-share/v1";
pub const HPKE_ENCAPPED_KEY_BYTES: usize = 32;
pub const HPKE_SEALED_SHARE_BYTES: usize = 48;

const X25519_MODULUS: [u8; 32] = [
    0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeCustodyPublicKeyV1([u8; 32]);

impl NodeCustodyPublicKeyV1 {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_canonical_x25519_public_key(bytes, "node_custody_public_key")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn matches_public_key_bytes(&self, bytes: &[u8; 32]) -> bool {
        &self.0 == bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShareCoordinateV1(u8);

impl ShareCoordinateV1 {
    pub fn new(value: u8) -> Result<Self, ContractError> {
        if value == 0 {
            return Err(ContractError::InvalidField("share_coordinate"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CustodyNodeIdentityV1 {
    node_public_key: NodePublicKey,
    custody_public_key: NodeCustodyPublicKeyV1,
    share_coordinate: ShareCoordinateV1,
}

impl CustodyNodeIdentityV1 {
    pub fn new(
        node_public_key: NodePublicKey,
        custody_public_key: NodeCustodyPublicKeyV1,
        share_coordinate: ShareCoordinateV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            node_public_key,
            custody_public_key,
            share_coordinate,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn custody_public_key(&self) -> NodeCustodyPublicKeyV1 {
        self.custody_public_key
    }

    pub const fn share_coordinate(&self) -> ShareCoordinateV1 {
        self.share_coordinate
    }

    pub fn stored_share_aad_bytes(
        &self,
        manifest_hash: Digest32,
    ) -> Result<Vec<u8>, ContractError> {
        StoredShareAadV1::new(manifest_hash, self.clone())?.canonical_bytes()
    }

    pub fn released_share_aad_bytes(
        &self,
        release_request_hash: Digest32,
        binding: &ProtectedContentBindingV1,
        decision_hash: Digest32,
        recipient: &RecipientKeyIdentityV1,
    ) -> Result<Vec<u8>, ContractError> {
        ReleasedShareAadV1::new(
            release_request_hash,
            binding.clone(),
            decision_hash,
            self.clone(),
            recipient.clone(),
        )?
        .canonical_bytes()
    }
}

impl CanonicalBody for CustodyNodeIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-node-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        NodePublicKey::new(*self.node_public_key.as_bytes())?;
        NodeCustodyPublicKeyV1::new(*self.custody_public_key.as_bytes())?;
        ShareCoordinateV1::new(self.share_coordinate.get())?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.node_public_key.as_bytes());
        encoder.fixed(self.custody_public_key.as_bytes());
        encoder.u8(self.share_coordinate.get());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            NodePublicKey::new(decoder.fixed()?)?,
            NodeCustodyPublicKeyV1::new(decoder.fixed()?)?,
            ShareCoordinateV1::new(decoder.u8()?)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEnvelopeManifestV1 {
    encrypted_content: EncryptedContentIdentityV1,
    threshold: ThresholdV1,
    nodes: Vec<CustodyNodeIdentityV1>,
}

impl CustodyEnvelopeManifestV1 {
    pub fn new(
        encrypted_content: EncryptedContentIdentityV1,
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
            encrypted_content,
            threshold,
            nodes,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }

    pub fn nodes(&self) -> &[CustodyNodeIdentityV1] {
        &self.nodes
    }

    pub fn manifest_hash(&self) -> Result<Digest32, ContractError> {
        self.canonical_hash()
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

    pub fn node_index(&self, node_public_key: NodePublicKey) -> Option<usize> {
        self.nodes
            .binary_search_by_key(&node_public_key, |node| node.node_public_key())
            .ok()
    }

    pub fn node(&self, node_public_key: NodePublicKey) -> Option<&CustodyNodeIdentityV1> {
        self.node_index(node_public_key)
            .map(|index| &self.nodes[index])
    }

    pub fn stored_share_aad_bytes_for_node(
        &self,
        node_public_key: NodePublicKey,
    ) -> Result<Vec<u8>, ContractError> {
        let node = self
            .node(node_public_key)
            .ok_or(ContractError::InvalidField("custody_node"))?;
        node.stored_share_aad_bytes(self.manifest_hash()?)
    }
}

impl CanonicalBody for CustodyEnvelopeManifestV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-envelope-manifest/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content.validate()?;
        self.threshold.validate()?;
        if self.nodes.len() != usize::from(self.threshold.total())
            || self.nodes.len() > usize::from(MAX_THRESHOLD_NODES)
            || self
                .nodes
                .windows(2)
                .any(|window| window[0].node_public_key() >= window[1].node_public_key())
        {
            return Err(ContractError::InvalidField("custody_manifest.nodes"));
        }
        for (index, node) in self.nodes.iter().enumerate() {
            node.canonical_bytes()?;
            let expected_coordinate = ShareCoordinateV1::new(
                u8::try_from(index + 1)
                    .map_err(|_| ContractError::InvalidField("share_coordinate"))?,
            )?;
            if node.share_coordinate() != expected_coordinate {
                return Err(ContractError::InvalidField("share_coordinate"));
            }
        }
        if has_duplicate_custody_keys(&self.nodes) {
            return Err(ContractError::InvalidField("node_custody_public_key"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.encrypted_content)?;
        self.threshold.encode(encoder);
        encoder.u8(self.nodes.len() as u8);
        for node in &self.nodes {
            encoder.nested(node)?;
        }
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let encrypted_content = decoder.nested("encrypted_content")?;
        let threshold = ThresholdV1::decode(decoder)?;
        let count = usize::from(decoder.u8()?);
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            nodes.push(decoder.nested("custody_node")?);
        }
        Self::new(encrypted_content, threshold, nodes)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HpkeCiphertextV1 {
    encapped_key: [u8; HPKE_ENCAPPED_KEY_BYTES],
    ciphertext: [u8; HPKE_SEALED_SHARE_BYTES],
}

impl std::fmt::Debug for HpkeCiphertextV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HpkeCiphertextV1")
            .field("encapped_key", &"[redacted]")
            .field("ciphertext", &"[redacted]")
            .finish()
    }
}

impl HpkeCiphertextV1 {
    pub fn new(
        encapped_key: [u8; HPKE_ENCAPPED_KEY_BYTES],
        ciphertext: [u8; HPKE_SEALED_SHARE_BYTES],
    ) -> Result<Self, ContractError> {
        let value = Self {
            encapped_key,
            ciphertext,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn encapped_key(&self) -> &[u8; HPKE_ENCAPPED_KEY_BYTES] {
        &self.encapped_key
    }

    pub const fn ciphertext(&self) -> &[u8; HPKE_SEALED_SHARE_BYTES] {
        &self.ciphertext
    }
}

impl CanonicalBody for HpkeCiphertextV1 {
    const DOMAIN: &'static str = "elastos.protected-content.hpke-ciphertext/v1";

    fn validate(&self) -> Result<(), ContractError> {
        validate_canonical_x25519_public_key(self.encapped_key, "hpke_encapped_key")?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(&self.encapped_key);
        encoder.fixed(&self.ciphertext);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(decoder.fixed()?, decoder.fixed()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEnvelopeV1 {
    manifest: CustodyEnvelopeManifestV1,
    stored_shares: Vec<HpkeCiphertextV1>,
}

impl CustodyEnvelopeV1 {
    pub fn new(
        manifest: CustodyEnvelopeManifestV1,
        stored_shares: Vec<HpkeCiphertextV1>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            manifest,
            stored_shares,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn manifest(&self) -> &CustodyEnvelopeManifestV1 {
        &self.manifest
    }

    pub fn stored_shares(&self) -> &[HpkeCiphertextV1] {
        &self.stored_shares
    }

    pub fn stored_share_for_node(
        &self,
        node_public_key: NodePublicKey,
    ) -> Option<&HpkeCiphertextV1> {
        self.manifest
            .node_index(node_public_key)
            .map(|index| &self.stored_shares[index])
    }

    pub fn key_envelope_identity(&self) -> Result<KeyEnvelopeIdentityV1, ContractError> {
        KeyEnvelopeIdentityV1::new(
            self.manifest.encrypted_content.clone(),
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::FieldTooLong("custody_envelope"))?,
            self.manifest.node_set()?.node_set_id()?,
            self.manifest.threshold,
        )
    }

    pub fn matches_key_envelope_identity(
        &self,
        key_envelope: &KeyEnvelopeIdentityV1,
    ) -> Result<bool, ContractError> {
        Ok(self.key_envelope_identity()? == *key_envelope)
    }
}

impl CanonicalBody for CustodyEnvelopeV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-envelope/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.manifest.canonical_bytes()?;
        if self.stored_shares.len() != self.manifest.nodes.len() {
            return Err(ContractError::InvalidField("stored_shares"));
        }
        for stored_share in &self.stored_shares {
            stored_share.canonical_bytes()?;
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.manifest)?;
        encoder.u8(self.stored_shares.len() as u8);
        for stored_share in &self.stored_shares {
            encoder.nested(stored_share)?;
        }
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let manifest = decoder.nested("custody_manifest")?;
        let count = usize::from(decoder.u8()?);
        let mut stored_shares = Vec::with_capacity(count);
        for _ in 0..count {
            stored_shares.push(decoder.nested("stored_share")?);
        }
        Self::new(manifest, stored_shares)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredShareAadV1 {
    manifest_hash: Digest32,
    node: CustodyNodeIdentityV1,
}

impl StoredShareAadV1 {
    fn new(manifest_hash: Digest32, node: CustodyNodeIdentityV1) -> Result<Self, ContractError> {
        let value = Self {
            manifest_hash,
            node,
        };
        value.validate()?;
        Ok(value)
    }
}

impl CanonicalBody for StoredShareAadV1 {
    const DOMAIN: &'static str = "elastos.protected-content.stored-share-aad/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.node.canonical_bytes()?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.manifest_hash.as_bytes());
        encoder.nested(&self.node)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            decoder.nested("custody_node")?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleasedShareAadV1 {
    release_request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    decision_hash: Digest32,
    node: CustodyNodeIdentityV1,
    recipient: RecipientKeyIdentityV1,
}

impl ReleasedShareAadV1 {
    fn new(
        release_request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        decision_hash: Digest32,
        node: CustodyNodeIdentityV1,
        recipient: RecipientKeyIdentityV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            release_request_hash,
            binding,
            decision_hash,
            node,
            recipient,
        };
        value.validate()?;
        Ok(value)
    }
}

impl CanonicalBody for ReleasedShareAadV1 {
    const DOMAIN: &'static str = "elastos.protected-content.released-share-aad/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.node.canonical_bytes()?;
        self.recipient.canonical_bytes()?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.release_request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.fixed(self.decision_hash.as_bytes());
        encoder.nested(&self.node)?;
        encoder.nested(&self.recipient)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            decoder.nested("binding")?,
            Digest32::new(decoder.fixed()?),
            decoder.nested("custody_node")?,
            decoder.nested("recipient")?,
        )
    }
}

fn has_duplicate_custody_keys(nodes: &[CustodyNodeIdentityV1]) -> bool {
    for (index, node) in nodes.iter().enumerate() {
        if nodes[index + 1..]
            .iter()
            .any(|other| node.custody_public_key() == other.custody_public_key())
        {
            return true;
        }
    }
    false
}

fn validate_canonical_x25519_public_key(
    bytes: [u8; 32],
    field: &'static str,
) -> Result<(), ContractError> {
    if bytes[31] & 0x80 != 0 || le_bytes_ge(bytes, X25519_MODULUS) {
        return Err(ContractError::InvalidField(field));
    }
    let point = MontgomeryPoint(bytes);
    if X25519_LOW_ORDER_POINTS.iter().any(|low_order| point == *low_order) {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

fn le_bytes_ge(lhs: [u8; 32], rhs: [u8; 32]) -> bool {
    for index in (0..32).rev() {
        if lhs[index] != rhs[index] {
            return lhs[index] > rhs[index];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use curve25519_dalek::{
        constants::X25519_LOW_ORDER_POINTS,
        montgomery::MontgomeryPoint,
    };
    use ed25519_dalek::SigningKey;
    use hex::encode;

    use super::*;

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        let key = SigningKey::from_bytes(&[seed; 32]);
        NodePublicKey::new(key.verifying_key().to_bytes()).unwrap()
    }

    fn custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        NodeCustodyPublicKeyV1::new(valid_x25519_public_key_bytes(seed)).unwrap()
    }

    fn node(seed: u8, coordinate: u8) -> CustodyNodeIdentityV1 {
        CustodyNodeIdentityV1::new(
            node_public_key(seed),
            custody_public_key(seed),
            ShareCoordinateV1::new(coordinate).unwrap(),
        )
        .unwrap()
    }

    fn encrypted_content(byte: u8) -> EncryptedContentIdentityV1 {
        EncryptedContentIdentityV1::new(digest(byte), 4096).unwrap()
    }

    fn manifest() -> CustodyEnvelopeManifestV1 {
        CustodyEnvelopeManifestV1::new(
            encrypted_content(0x11),
            ThresholdV1::new(2, 3).unwrap(),
            vec![node(3, 3), node(1, 1), node(2, 2)],
        )
        .unwrap()
    }

    fn stored_share(seed: u8) -> HpkeCiphertextV1 {
        let encapped_key = valid_x25519_public_key_bytes(seed.wrapping_add(0x40));
        let mut ciphertext = [0u8; HPKE_SEALED_SHARE_BYTES];
        ciphertext.fill(seed.wrapping_add(0x40));
        HpkeCiphertextV1::new(encapped_key, ciphertext).unwrap()
    }

    fn valid_x25519_public_key_bytes(seed: u8) -> [u8; 32] {
        MontgomeryPoint::mul_base_clamped([seed; 32]).to_bytes()
    }

    fn recipient(seed: u8) -> RecipientKeyIdentityV1 {
        RecipientKeyIdentityV1::new(CUSTODY_HPKE_SUITE_ID_V1, digest(seed)).unwrap()
    }

    #[test]
    fn custody_manifest_sorts_nodes_and_assigns_deterministic_coordinates() {
        let manifest = manifest();
        let mut expected = vec![node_public_key(1), node_public_key(2), node_public_key(3)];
        expected.sort_unstable();
        assert_eq!(
            manifest
                .nodes()
                .iter()
                .map(|node| node.node_public_key())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            manifest
                .nodes()
                .iter()
                .map(|node| node.share_coordinate().get())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn custody_envelope_round_trip_and_identity_vector_are_stable() {
        let envelope = CustodyEnvelopeV1::new(
            manifest(),
            vec![stored_share(1), stored_share(2), stored_share(3)],
        )
        .unwrap();
        let canonical = envelope.canonical_bytes().unwrap();
        let decoded = CustodyEnvelopeV1::from_canonical_bytes(&canonical).unwrap();
        assert_eq!(decoded, envelope);

        assert_eq!(
            encode(envelope.canonical_hash().unwrap().as_bytes()),
            "059072cd112a899827770607fe96268a8338d700db49fda12661364429d8a590"
        );

        let key_envelope = envelope.key_envelope_identity().unwrap();
        assert_eq!(
            key_envelope.encrypted_content(),
            envelope.manifest().encrypted_content()
        );
        assert_eq!(
            key_envelope.node_set_id(),
            envelope
                .manifest()
                .node_set()
                .unwrap()
                .node_set_id()
                .unwrap()
        );
    }

    #[test]
    fn custody_manifest_rejects_duplicate_keys_bad_coordinates_and_bad_counts() {
        assert_eq!(
            ShareCoordinateV1::new(0),
            Err(ContractError::InvalidField("share_coordinate"))
        );

        let threshold = ThresholdV1::new(2, 3).unwrap();
        assert_eq!(
            CustodyEnvelopeManifestV1::new(
                encrypted_content(0x11),
                threshold,
                vec![
                    node(1, 1),
                    CustodyNodeIdentityV1::new(
                        node_public_key(2),
                        custody_public_key(1),
                        ShareCoordinateV1::new(2).unwrap()
                    )
                    .unwrap(),
                    node(3, 3),
                ],
            ),
            Err(ContractError::InvalidField("node_custody_public_key"))
        );

        assert_eq!(
            CustodyEnvelopeManifestV1::new(
                encrypted_content(0x11),
                threshold,
                vec![node(1, 1), node(2, 2)],
            ),
            Err(ContractError::InvalidField("custody_manifest.nodes"))
        );
    }

    #[test]
    fn custody_public_key_rejects_low_order_noncanonical_and_high_bit_alias_bytes() {
        for low_order in X25519_LOW_ORDER_POINTS.iter() {
            assert_eq!(
                NodeCustodyPublicKeyV1::new(low_order.to_bytes()),
                Err(ContractError::InvalidField("node_custody_public_key"))
            );
        }
        assert_eq!(
            NodeCustodyPublicKeyV1::new(X25519_MODULUS),
            Err(ContractError::InvalidField("node_custody_public_key"))
        );
        let mut p_plus_one = X25519_MODULUS;
        p_plus_one[0] = p_plus_one[0].wrapping_add(1);
        assert_eq!(
            NodeCustodyPublicKeyV1::new(p_plus_one),
            Err(ContractError::InvalidField("node_custody_public_key"))
        );
        let mut high_bit_alias = valid_x25519_public_key_bytes(0x51);
        high_bit_alias[31] |= 0x80;
        assert_eq!(
            NodeCustodyPublicKeyV1::new(high_bit_alias),
            Err(ContractError::InvalidField("node_custody_public_key"))
        );
    }

    #[test]
    fn custody_public_key_accepts_generated_valid_bytes() {
        assert!(NodeCustodyPublicKeyV1::new(valid_x25519_public_key_bytes(0x41)).is_ok());
    }

    #[test]
    fn custody_envelope_rejects_wrong_share_count_and_noncanonical_lengths() {
        let manifest = manifest();
        assert_eq!(
            CustodyEnvelopeV1::new(manifest.clone(), vec![stored_share(1), stored_share(2)]),
            Err(ContractError::InvalidField("stored_shares"))
        );

        let canonical = stored_share(1).canonical_bytes().unwrap();
        assert_eq!(
            HpkeCiphertextV1::from_canonical_bytes(&canonical[..canonical.len() - 1]),
            Err(ContractError::UnexpectedEnd)
        );
    }

    #[test]
    fn hpke_ciphertext_rejects_invalid_encapped_keys_at_constructor_and_decode() {
        let ciphertext = [0x55; HPKE_SEALED_SHARE_BYTES];
        assert_eq!(
            HpkeCiphertextV1::new([0; HPKE_ENCAPPED_KEY_BYTES], ciphertext),
            Err(ContractError::InvalidField("hpke_encapped_key"))
        );
        assert_eq!(
            HpkeCiphertextV1::new(X25519_MODULUS, ciphertext),
            Err(ContractError::InvalidField("hpke_encapped_key"))
        );
        let mut high_bit_alias = valid_x25519_public_key_bytes(0x61);
        high_bit_alias[31] |= 0x80;
        assert_eq!(
            HpkeCiphertextV1::new(high_bit_alias, ciphertext),
            Err(ContractError::InvalidField("hpke_encapped_key"))
        );

        let valid = HpkeCiphertextV1::new(valid_x25519_public_key_bytes(0x62), ciphertext).unwrap();
        let mut canonical = valid.canonical_bytes().unwrap();
        let offset = <HpkeCiphertextV1 as CanonicalBody>::DOMAIN.len() + 1;
        canonical[offset..offset + HPKE_ENCAPPED_KEY_BYTES]
            .copy_from_slice(X25519_LOW_ORDER_POINTS[2].as_bytes());
        assert_eq!(
            HpkeCiphertextV1::from_canonical_bytes(&canonical),
            Err(ContractError::InvalidField("hpke_encapped_key"))
        );
    }

    #[test]
    fn envelope_identity_changes_when_bound_fields_mutate() {
        let envelope = CustodyEnvelopeV1::new(
            manifest(),
            vec![stored_share(1), stored_share(2), stored_share(3)],
        )
        .unwrap();
        let changed_content = CustodyEnvelopeV1::new(
            CustodyEnvelopeManifestV1::new(
                encrypted_content(0x12),
                ThresholdV1::new(2, 3).unwrap(),
                vec![node(1, 1), node(2, 2), node(3, 3)],
            )
            .unwrap(),
            vec![stored_share(1), stored_share(2), stored_share(3)],
        )
        .unwrap();
        let changed_threshold = CustodyEnvelopeV1::new(
            CustodyEnvelopeManifestV1::new(
                encrypted_content(0x11),
                ThresholdV1::new(3, 3).unwrap(),
                vec![node(1, 1), node(2, 2), node(3, 3)],
            )
            .unwrap(),
            vec![stored_share(1), stored_share(2), stored_share(3)],
        )
        .unwrap();
        let changed_stored_share = CustodyEnvelopeV1::new(
            manifest(),
            vec![stored_share(1), stored_share(2), stored_share(4)],
        )
        .unwrap();
        let changed_custody_key = CustodyEnvelopeV1::new(
            CustodyEnvelopeManifestV1::new(
                encrypted_content(0x11),
                ThresholdV1::new(2, 3).unwrap(),
                vec![
                    node(1, 1),
                    CustodyNodeIdentityV1::new(
                        node_public_key(2),
                        custody_public_key(9),
                        ShareCoordinateV1::new(2).unwrap(),
                    )
                    .unwrap(),
                    node(3, 3),
                ],
            )
            .unwrap(),
            vec![stored_share(1), stored_share(2), stored_share(3)],
        )
        .unwrap();

        assert_ne!(
            envelope.key_envelope_identity().unwrap().envelope_sha256(),
            changed_content
                .key_envelope_identity()
                .unwrap()
                .envelope_sha256()
        );
        assert_ne!(
            envelope.key_envelope_identity().unwrap().envelope_sha256(),
            changed_threshold
                .key_envelope_identity()
                .unwrap()
                .envelope_sha256()
        );
        assert_ne!(
            envelope.key_envelope_identity().unwrap().envelope_sha256(),
            changed_stored_share
                .key_envelope_identity()
                .unwrap()
                .envelope_sha256()
        );
        assert_ne!(
            envelope.key_envelope_identity().unwrap().envelope_sha256(),
            changed_custody_key
                .key_envelope_identity()
                .unwrap()
                .envelope_sha256()
        );
    }

    #[test]
    fn stored_and_released_share_aad_are_mutation_sensitive() {
        let manifest = manifest();
        let node = manifest.node(node_public_key(1)).unwrap();
        let stored_aad = node
            .stored_share_aad_bytes(manifest.manifest_hash().unwrap())
            .unwrap();
        let changed_stored_aad = node
            .stored_share_aad_bytes(Digest32::new([0x77; 32]))
            .unwrap();
        assert_ne!(stored_aad, changed_stored_aad);

        let binding = {
            let envelope = CustodyEnvelopeV1::new(
                manifest.clone(),
                vec![stored_share(1), stored_share(2), stored_share(3)],
            )
            .unwrap();
            let profile_key = SigningKey::from_bytes(&[0x26; 32]);
            ProtectedContentBindingV1::new(
                manifest.encrypted_content().clone(),
                envelope.key_envelope_identity().unwrap(),
                crate::RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
                crate::ProfileIdentityV1::from_public_key_bytes(
                    profile_key.verifying_key().to_bytes(),
                )
                .unwrap(),
                crate::WalletAddress::new([0x55; 20]),
                crate::RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
            )
            .unwrap()
        };
        let released_aad = node
            .released_share_aad_bytes(digest(0x10), &binding, digest(0x20), &recipient(0x30))
            .unwrap();
        let changed_released_aad = node
            .released_share_aad_bytes(digest(0x11), &binding, digest(0x20), &recipient(0x30))
            .unwrap();
        assert_ne!(released_aad, changed_released_aad);
    }
}
