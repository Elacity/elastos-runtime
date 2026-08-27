use std::collections::HashMap;

use ed25519_dalek::SigningKey;
use k256::ecdsa::SigningKey as WalletSigningKey;
use sha3::{Digest as _, Keccak256};

use crate::{
    AtomicReplayClaimer, CustodyEpochIdentityV1, Digest32, EncryptedContentIdentityV1,
    KeyEnvelopeIdentityV1, NodePublicKey, NodeSetV1, ProfileIdentityV1, ProtectedContentBindingV1,
    ReplayClaimError, ReplayClaimKeyV1, RightsPolicyIdentityV1, RuntimeSessionBindingV1,
    ThresholdV1, WalletAddress,
};

pub(crate) const NOW: u64 = 2_000_000_000;

pub(crate) fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

pub(crate) fn node_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub(crate) fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_key(seed).verifying_key().to_bytes()).unwrap()
}

pub(crate) fn node_set() -> NodeSetV1 {
    NodeSetV1::new(
        ThresholdV1::new(2, 3).unwrap(),
        vec![node_public_key(1), node_public_key(2), node_public_key(3)],
    )
    .unwrap()
}

pub(crate) fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

pub(crate) fn custody_epoch_identity() -> CustodyEpochIdentityV1 {
    CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap()
}

pub(crate) fn binding_for_wallet(wallet: WalletAddress) -> ProtectedContentBindingV1 {
    let content = EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap();
    let node_set = node_set();
    let threshold = node_set.threshold();
    let envelope = KeyEnvelopeIdentityV1::new(
        content.clone(),
        digest(0x22),
        512,
        node_set.node_set_id().unwrap(),
        threshold,
        custody_epoch_identity(),
    )
    .unwrap();
    let profile_key = SigningKey::from_bytes(&[0x26; 32]);
    ProtectedContentBindingV1::new(
        content,
        envelope,
        RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
        ProfileIdentityV1::from_public_key_bytes(profile_key.verifying_key().to_bytes()).unwrap(),
        wallet,
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap()
}

#[derive(Debug, Default)]
pub(crate) struct TestReplayClaims {
    claims: HashMap<ReplayClaimKeyV1, u64>,
}

impl TestReplayClaims {
    pub(crate) fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }
}

impl AtomicReplayClaimer for TestReplayClaims {
    fn claim(
        &mut self,
        key: ReplayClaimKeyV1,
        expires_at: u64,
        now: u64,
    ) -> Result<(), ReplayClaimError> {
        self.claims.retain(|_, expiry| *expiry > now);
        if self.claims.contains_key(&key) {
            return Err(ReplayClaimError::AlreadyClaimed);
        }
        self.claims.insert(key, expires_at);
        Ok(())
    }
}
