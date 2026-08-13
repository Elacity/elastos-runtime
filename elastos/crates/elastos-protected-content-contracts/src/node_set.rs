use serde::Serialize;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::{CanonicalContract, Digest32, ThresholdV1};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct NodePublicKey([u8; 32]);

impl NodePublicKey {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_ed25519_public_key(bytes, "node_public_key")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeSetV1 {
    threshold: ThresholdV1,
    members: Vec<NodePublicKey>,
}

impl NodeSetV1 {
    pub fn new(
        threshold: ThresholdV1,
        mut members: Vec<NodePublicKey>,
    ) -> Result<Self, ContractError> {
        members.sort_unstable();
        let value = Self { threshold, members };
        value.validate()?;
        Ok(value)
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }

    pub fn members(&self) -> &[NodePublicKey] {
        &self.members
    }

    pub fn node_set_id(&self) -> Result<Digest32, ContractError> {
        self.canonical_hash()
    }

    pub fn contains(&self, node: NodePublicKey) -> bool {
        self.members.binary_search(&node).is_ok()
    }
}

impl CanonicalBody for NodeSetV1 {
    const DOMAIN: &'static str = "elastos.protected-content.node-set/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.threshold.validate()?;
        if self.members.len() != usize::from(self.threshold.total())
            || self.members.windows(2).any(|window| window[0] >= window[1])
        {
            return Err(ContractError::InvalidField("node_set.members"));
        }
        for member in &self.members {
            NodePublicKey::new(*member.as_bytes())?;
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        self.threshold.encode(encoder);
        encoder.u8(self.members.len() as u8);
        for member in &self.members {
            encoder.fixed(member.as_bytes());
        }
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let threshold = ThresholdV1::decode(decoder)?;
        let count = usize::from(decoder.u8()?);
        if count != usize::from(threshold.total()) {
            return Err(ContractError::InvalidField("node_set.members"));
        }
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            members.push(NodePublicKey::new(decoder.fixed()?)?);
        }
        Self::new(threshold, members)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_alias() -> [u8; 32] {
        [
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ]
    }

    fn noncanonical_alias() -> [u8; 32] {
        [
            0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0x7f,
        ]
    }

    #[test]
    fn node_authority_rejects_noncanonical_and_weak_ed25519_keys() {
        assert_eq!(
            NodePublicKey::new(noncanonical_alias()),
            Err(ContractError::InvalidField("node_public_key"))
        );
        assert_eq!(
            NodePublicKey::new([
                236, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127,
            ]),
            Err(ContractError::InvalidField("node_public_key"))
        );
    }

    #[test]
    fn node_set_aliases_cannot_count_as_distinct_authorities() {
        assert_eq!(
            NodePublicKey::new(canonical_alias()),
            Err(ContractError::InvalidField("node_public_key"))
        );
        assert_eq!(
            NodePublicKey::new(noncanonical_alias()),
            Err(ContractError::InvalidField("node_public_key"))
        );
    }
}
