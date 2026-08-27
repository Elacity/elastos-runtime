use ed25519_dalek::SigningKey;
use hpke::{kem::Kem as KemTrait, Deserializable, Serializable};
use rand09::{rngs::StdRng, RngCore as _, SeedableRng as _};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    Digest32, NodeCustodyPublicKeyV1, NodePublicKey, RecipientKeyIdentityV1,
    CONTENT_KEY_COMMITMENT_DOMAIN_V1, CUSTODY_HPKE_SUITE_ID_V1,
};

use crate::{hpke_helpers::HpkeKem, CustodyError, CONTENT_KEY_BYTES};

pub struct ContentEncryptionKeyV1(Zeroizing<[u8; CONTENT_KEY_BYTES]>);

impl ContentEncryptionKeyV1 {
    pub fn generate() -> Result<Self, CustodyError> {
        Ok(Self(random_bytes()?))
    }

    pub(crate) fn with_bytes<T>(&self, callback: impl FnOnce(&[u8; CONTENT_KEY_BYTES]) -> T) -> T {
        callback(&self.0)
    }

    pub(crate) fn from_guarded_bytes(bytes: Zeroizing<[u8; CONTENT_KEY_BYTES]>) -> Self {
        Self(bytes)
    }

    pub(crate) fn commitment(&self) -> Digest32 {
        self.with_bytes(content_key_commitment)
    }

    pub(crate) fn matches_commitment(&self, expected: Digest32) -> bool {
        self.commitment()
            .as_bytes()
            .ct_eq(expected.as_bytes())
            .into()
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(bytes: [u8; CONTENT_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for ContentEncryptionKeyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ContentEncryptionKeyV1([redacted])")
    }
}

pub struct NodeCustodySecretKeyV1(Zeroizing<[u8; 32]>);

impl NodeCustodySecretKeyV1 {
    pub fn generate() -> Result<Self, CustodyError> {
        Ok(Self(random_bytes()?))
    }

    pub fn public_key(&self) -> Result<NodeCustodyPublicKeyV1, CustodyError> {
        NodeCustodyPublicKeyV1::new(public_key_bytes_from_secret(&self.0)?).map_err(Into::into)
    }

    pub fn matches_node_entry(
        &self,
        node_public_key: NodePublicKey,
        entry_key: NodeCustodyPublicKeyV1,
        signing_key: &SigningKey,
    ) -> Result<(), CustodyError> {
        let derived_signer = NodePublicKey::new(signing_key.verifying_key().to_bytes())?;
        if derived_signer != node_public_key {
            return Err(CustodyError::BindingMismatch("node_public_key"));
        }
        if self.public_key()? != entry_key {
            return Err(CustodyError::BindingMismatch("node_custody_public_key"));
        }
        Ok(())
    }

    pub(crate) fn secret_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for NodeCustodySecretKeyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeCustodySecretKeyV1([redacted])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipientPublicKeyV1([u8; 32]);

impl RecipientPublicKeyV1 {
    pub fn new(bytes: [u8; 32]) -> Result<Self, CustodyError> {
        NodeCustodyPublicKeyV1::new(bytes)
            .map(|_| Self(bytes))
            .map_err(|_| CustodyError::BindingMismatch("recipient_public_key"))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn identity(&self) -> Result<RecipientKeyIdentityV1, CustodyError> {
        Ok(RecipientKeyIdentityV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            Digest32::new(Sha256::digest(self.0).into()),
        )?)
    }
}

pub struct RecipientSecretKeyV1(Zeroizing<[u8; 32]>);

impl RecipientSecretKeyV1 {
    pub fn generate() -> Result<Self, CustodyError> {
        Ok(Self(random_bytes()?))
    }

    pub fn public_key(&self) -> Result<RecipientPublicKeyV1, CustodyError> {
        RecipientPublicKeyV1::new(public_key_bytes_from_secret(&self.0)?)
    }

    pub fn identity(&self) -> Result<RecipientKeyIdentityV1, CustodyError> {
        self.public_key()?.identity()
    }

    pub(crate) fn secret_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for RecipientSecretKeyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RecipientSecretKeyV1([redacted])")
    }
}

fn public_key_bytes_from_secret(secret_bytes: &[u8; 32]) -> Result<[u8; 32], CustodyError> {
    let secret = <HpkeKem as KemTrait>::PrivateKey::from_bytes(secret_bytes)?;
    Ok(<HpkeKem as KemTrait>::sk_to_pk(&secret).to_bytes().into())
}

fn content_key_commitment(bytes: &[u8; CONTENT_KEY_BYTES]) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(CONTENT_KEY_COMMITMENT_DOMAIN_V1);
    hasher.update([0u8]);
    hasher.update(bytes);
    Digest32::new(hasher.finalize().into())
}

fn random_bytes<const N: usize>() -> Result<Zeroizing<[u8; N]>, CustodyError> {
    let mut bytes = Zeroizing::new([0u8; N]);
    StdRng::try_from_os_rng()
        .map_err(|_| CustodyError::RandomnessUnavailable)?
        .fill_bytes(&mut *bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_output_is_redacted() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x11; CONTENT_KEY_BYTES]);
        let node_secret = NodeCustodySecretKeyV1::from_test_bytes([0x22; 32]);
        let recipient_secret = RecipientSecretKeyV1::from_test_bytes([0x33; 32]);

        let content_debug = format!("{content_key:?}");
        let node_debug = format!("{node_secret:?}");
        let recipient_debug = format!("{recipient_secret:?}");

        assert!(content_debug.contains("[redacted]"));
        assert!(node_debug.contains("[redacted]"));
        assert!(recipient_debug.contains("[redacted]"));
        assert!(!content_debug.contains("11"));
        assert!(!node_debug.contains("22"));
        assert!(!recipient_debug.contains("33"));
    }

    #[test]
    fn recipient_public_key_rejects_invalid_x25519_bytes() {
        let low_order = {
            let mut bytes = [0u8; 32];
            bytes[0] = 1;
            bytes
        };
        let mut high_bit_alias = RecipientSecretKeyV1::from_test_bytes([0x41; 32])
            .public_key()
            .unwrap()
            .0;
        high_bit_alias[31] |= 0x80;
        let valid = RecipientSecretKeyV1::from_test_bytes([0x42; 32])
            .public_key()
            .unwrap()
            .0;

        assert!(matches!(
            RecipientPublicKeyV1::new([0; 32]),
            Err(CustodyError::BindingMismatch("recipient_public_key"))
        ));
        assert!(matches!(
            RecipientPublicKeyV1::new(low_order),
            Err(CustodyError::BindingMismatch("recipient_public_key"))
        ));
        assert!(matches!(
            RecipientPublicKeyV1::new(high_bit_alias),
            Err(CustodyError::BindingMismatch("recipient_public_key"))
        ));
        assert!(RecipientPublicKeyV1::new(valid).is_ok());
    }
}
