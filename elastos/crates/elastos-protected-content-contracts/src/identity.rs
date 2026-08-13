use ed25519_dalek::VerifyingKey;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::canonical::{validate_ascii_identifier, CanonicalBody, ContractError, Decoder, Encoder};

pub const MAX_ENCRYPTED_CONTENT_BYTES: u64 = 1 << 50;
pub const MAX_KEY_ENVELOPE_BYTES: u32 = 1 << 20;
pub const MAX_RIGHTS_POLICY_BYTES: u32 = 1 << 20;
pub const MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES: usize = 96;
pub const MAX_THRESHOLD_NODES: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Digest32([u8; 32]);

impl Digest32 {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct WalletAddress([u8; 20]);

impl WalletAddress {
    pub const fn new(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

/// Exact recipient key selected by Runtime for one protected-content release.
///
/// The suite id names the reviewed encryption and key-encoding suite. The key
/// id is the SHA-256 identity of the exact recipient public-key bytes under
/// that suite. This type identifies a recipient; it does not implement or
/// approve an encryption suite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct RecipientKeyIdentityV1 {
    encryption_suite_id: String,
    key_id: Digest32,
}

impl RecipientKeyIdentityV1 {
    pub fn new(
        encryption_suite_id: impl Into<String>,
        key_id: Digest32,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encryption_suite_id: encryption_suite_id.into(),
            key_id,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encryption_suite_id(&self) -> &str {
        &self.encryption_suite_id
    }

    pub const fn key_id(&self) -> Digest32 {
        self.key_id
    }

    pub fn matches_public_key(&self, public_key_bytes: &[u8]) -> bool {
        let digest: [u8; 32] = Sha256::digest(public_key_bytes).into();
        digest == *self.key_id.as_bytes()
    }
}

impl CanonicalBody for RecipientKeyIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.recipient-key-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        validate_ascii_identifier(
            &self.encryption_suite_id,
            "recipient_encryption_suite_id",
            MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.string(&self.encryption_suite_id)?;
        encoder.fixed(self.key_id.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.string(
                "recipient_encryption_suite_id",
                MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES,
            )?,
            Digest32::new(decoder.fixed()?),
        )
    }
}

/// Collaboration Profile authority in its unique canonical form.
///
/// Contract encodings contain only the Ed25519 public-key bytes. `did:key`
/// text is a checked input/projection through the repository's shared codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileIdentityV1([u8; 32]);

impl ProfileIdentityV1 {
    pub fn from_public_key_bytes(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_ed25519_public_key(bytes, "profile_public_key")?;
        Ok(Self(bytes))
    }

    pub fn from_did_key(did: &str) -> Result<Self, ContractError> {
        let key = elastos_identity::decode_did_key(did)
            .map_err(|_| ContractError::InvalidField("profile_did"))?;
        Self::from_public_key_bytes(key.to_bytes())
    }

    pub const fn public_key_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn did_key(&self) -> Result<String, ContractError> {
        let key = validate_ed25519_public_key(self.0, "profile_public_key")
            .map_err(|_| ContractError::InvalidField("profile_public_key"))?;
        elastos_identity::encode_did_key(&key)
            .map_err(|_| ContractError::InvalidField("profile_public_key"))
    }

    fn validate_key(&self) -> Result<(), ContractError> {
        Self::from_public_key_bytes(self.0).map(|_| ())
    }
}

pub(crate) fn validate_ed25519_public_key(
    bytes: [u8; 32],
    field: &'static str,
) -> Result<VerifyingKey, ContractError> {
    elastos_identity::validate_canonical_ed25519_verifying_key_bytes(bytes)
        .map_err(|_| ContractError::InvalidField(field))
}

impl CanonicalBody for ProfileIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.profile-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.validate_key()
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(&self.0);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::from_public_key_bytes(decoder.fixed()?)
    }
}

/// Non-secret fixed-width binding selected by Runtime for one verified session.
///
/// This is not the Runtime session id and must never contain a cookie, token,
/// credential, or other bearer authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RuntimeSessionBindingV1(Digest32);

impl RuntimeSessionBindingV1 {
    pub fn new(value: Digest32) -> Result<Self, ContractError> {
        if value == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField("runtime_session_binding"));
        }
        Ok(Self(value))
    }

    pub const fn digest(&self) -> Digest32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ThresholdV1 {
    required: u8,
    total: u8,
}

impl ThresholdV1 {
    pub fn new(required: u8, total: u8) -> Result<Self, ContractError> {
        let value = Self { required, total };
        value.validate()?;
        Ok(value)
    }

    pub const fn required(&self) -> u8 {
        self.required
    }

    pub const fn total(&self) -> u8 {
        self.total
    }

    pub(crate) fn validate(&self) -> Result<(), ContractError> {
        if self.required < 2 || self.required > self.total || self.total > MAX_THRESHOLD_NODES {
            return Err(ContractError::InvalidField("threshold"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self, encoder: &mut Encoder) {
        encoder.u8(self.required);
        encoder.u8(self.total);
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(decoder.u8()?, decoder.u8()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EncryptedContentIdentityV1 {
    ciphertext_sha256: Digest32,
    ciphertext_bytes: u64,
}

impl EncryptedContentIdentityV1 {
    pub fn new(ciphertext_sha256: Digest32, ciphertext_bytes: u64) -> Result<Self, ContractError> {
        let value = Self {
            ciphertext_sha256,
            ciphertext_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn ciphertext_sha256(&self) -> Digest32 {
        self.ciphertext_sha256
    }

    pub const fn ciphertext_bytes(&self) -> u64 {
        self.ciphertext_bytes
    }
}

impl CanonicalBody for EncryptedContentIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.encrypted-content/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.ciphertext_bytes == 0 || self.ciphertext_bytes > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(ContractError::InvalidField("ciphertext_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.ciphertext_sha256.as_bytes());
        encoder.u64(self.ciphertext_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u64()?)
    }
}

/// Content address of the immutable rule every node evaluates independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsPolicyIdentityV1 {
    policy_sha256: Digest32,
    policy_bytes: u32,
}

impl RightsPolicyIdentityV1 {
    pub fn new(policy_sha256: Digest32, policy_bytes: u32) -> Result<Self, ContractError> {
        let value = Self {
            policy_sha256,
            policy_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn policy_sha256(&self) -> Digest32 {
        self.policy_sha256
    }

    pub const fn policy_bytes(&self) -> u32 {
        self.policy_bytes
    }
}

impl CanonicalBody for RightsPolicyIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-policy-identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.policy_bytes == 0 || self.policy_bytes > MAX_RIGHTS_POLICY_BYTES {
            return Err(ContractError::InvalidField("policy_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.policy_sha256.as_bytes());
        encoder.u32(self.policy_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u32()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyEnvelopeIdentityV1 {
    encrypted_content: EncryptedContentIdentityV1,
    envelope_sha256: Digest32,
    envelope_bytes: u32,
    node_set_id: Digest32,
    threshold: ThresholdV1,
}

impl KeyEnvelopeIdentityV1 {
    pub fn new(
        encrypted_content: EncryptedContentIdentityV1,
        envelope_sha256: Digest32,
        envelope_bytes: u32,
        node_set_id: Digest32,
        threshold: ThresholdV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encrypted_content,
            envelope_sha256,
            envelope_bytes,
            node_set_id,
            threshold,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn envelope_sha256(&self) -> Digest32 {
        self.envelope_sha256
    }

    pub const fn envelope_bytes(&self) -> u32 {
        self.envelope_bytes
    }

    pub const fn node_set_id(&self) -> Digest32 {
        self.node_set_id
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }
}

impl CanonicalBody for KeyEnvelopeIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.key-envelope/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content.validate()?;
        self.threshold.validate()?;
        if self.envelope_bytes == 0 || self.envelope_bytes > MAX_KEY_ENVELOPE_BYTES {
            return Err(ContractError::InvalidField("envelope_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.encrypted_content)?;
        encoder.fixed(self.envelope_sha256.as_bytes());
        encoder.u32(self.envelope_bytes);
        encoder.fixed(self.node_set_id.as_bytes());
        self.threshold.encode(encoder);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("encrypted_content")?,
            Digest32::new(decoder.fixed()?),
            decoder.u32()?,
            Digest32::new(decoder.fixed()?),
            ThresholdV1::decode(decoder)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectedContentBindingV1 {
    encrypted_content: EncryptedContentIdentityV1,
    key_envelope: KeyEnvelopeIdentityV1,
    rights_policy: RightsPolicyIdentityV1,
    profile: ProfileIdentityV1,
    wallet: WalletAddress,
    runtime_session_binding: RuntimeSessionBindingV1,
}

impl ProtectedContentBindingV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        encrypted_content: EncryptedContentIdentityV1,
        key_envelope: KeyEnvelopeIdentityV1,
        rights_policy: RightsPolicyIdentityV1,
        profile: ProfileIdentityV1,
        wallet: WalletAddress,
        runtime_session_binding: RuntimeSessionBindingV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encrypted_content,
            key_envelope,
            rights_policy,
            profile,
            wallet,
            runtime_session_binding,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub fn key_envelope(&self) -> &KeyEnvelopeIdentityV1 {
        &self.key_envelope
    }

    pub fn rights_policy(&self) -> &RightsPolicyIdentityV1 {
        &self.rights_policy
    }

    pub const fn profile(&self) -> ProfileIdentityV1 {
        self.profile
    }

    pub const fn wallet(&self) -> WalletAddress {
        self.wallet
    }

    pub const fn runtime_session_binding(&self) -> RuntimeSessionBindingV1 {
        self.runtime_session_binding
    }
}

impl CanonicalBody for ProtectedContentBindingV1 {
    const DOMAIN: &'static str = "elastos.protected-content.binding/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content.validate()?;
        self.key_envelope.validate()?;
        self.rights_policy.validate()?;
        self.profile.validate_key()?;
        RuntimeSessionBindingV1::new(self.runtime_session_binding.digest())?;
        if self.key_envelope.encrypted_content() != &self.encrypted_content {
            return Err(ContractError::InvalidField(
                "key_envelope.encrypted_content",
            ));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.encrypted_content)?;
        encoder.nested(&self.key_envelope)?;
        encoder.nested(&self.rights_policy)?;
        encoder.nested(&self.profile)?;
        encoder.fixed(self.wallet.as_bytes());
        encoder.fixed(self.runtime_session_binding.digest().as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("encrypted_content")?,
            decoder.nested("key_envelope")?,
            decoder.nested("rights_policy")?,
            decoder.nested("profile")?,
            WalletAddress::new(decoder.fixed()?),
            RuntimeSessionBindingV1::new(Digest32::new(decoder.fixed()?))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::CanonicalContract;

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    #[test]
    fn profile_identity_is_key_bytes_with_checked_did_projection() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let profile =
            ProfileIdentityV1::from_public_key_bytes(key.verifying_key().to_bytes()).unwrap();
        assert_eq!(
            ProfileIdentityV1::from_did_key(&profile.did_key().unwrap()).unwrap(),
            profile
        );
        assert_eq!(
            ProfileIdentityV1::from_did_key("did:example:alice"),
            Err(ContractError::InvalidField("profile_did"))
        );
        assert_eq!(
            ProfileIdentityV1::from_public_key_bytes([
                0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00,
            ]),
            Err(ContractError::InvalidField("profile_public_key"))
        );
        assert_eq!(
            ProfileIdentityV1::from_public_key_bytes([
                0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0x7f,
            ]),
            Err(ContractError::InvalidField("profile_public_key"))
        );
        assert!(profile
            .canonical_bytes()
            .unwrap()
            .ends_with(profile.public_key_bytes()));
        assert_eq!(
            ProfileIdentityV1::from_public_key_bytes([
                236, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127,
            ]),
            Err(ContractError::InvalidField("profile_public_key"))
        );
    }

    #[test]
    fn recipient_key_identity_requires_an_exact_suite_and_key() {
        let public_key = [0xa0; 32];
        let recipient = RecipientKeyIdentityV1::new(
            "x25519-hkdf-sha256-aes256gcm/v1",
            Digest32::new(Sha256::digest(public_key).into()),
        )
        .unwrap();
        let bytes = recipient.canonical_bytes().unwrap();
        assert_eq!(
            RecipientKeyIdentityV1::from_canonical_bytes(&bytes).unwrap(),
            recipient
        );
        assert!(RecipientKeyIdentityV1::new("", digest(0xa0)).is_err());
        assert!(RecipientKeyIdentityV1::new("x25519\ninvalid", digest(0xa0)).is_err());
        assert!(recipient.matches_public_key(&public_key));
        assert!(!recipient.matches_public_key(&[0xa1; 32]));
        assert_ne!(
            RecipientKeyIdentityV1::new("x25519-hkdf-sha256-aes256gcm/v2", digest(0xa0))
                .unwrap()
                .canonical_hash()
                .unwrap(),
            recipient.canonical_hash().unwrap()
        );
    }

    #[test]
    fn runtime_session_binding_is_fixed_width_and_nonzero() {
        let binding = RuntimeSessionBindingV1::new(digest(0x66)).unwrap();
        assert_eq!(binding.digest(), digest(0x66));
        assert_eq!(
            RuntimeSessionBindingV1::new(digest(0)),
            Err(ContractError::InvalidField("runtime_session_binding"))
        );
    }

    #[test]
    fn policy_identity_is_strictly_content_addressed() {
        let policy = RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap();
        let bytes = policy.canonical_bytes().unwrap();
        assert_eq!(
            RightsPolicyIdentityV1::from_canonical_bytes(&bytes).unwrap(),
            policy
        );
        assert!(RightsPolicyIdentityV1::new(digest(0x44), 0).is_err());
        assert!(RightsPolicyIdentityV1::new(digest(0x44), MAX_RIGHTS_POLICY_BYTES + 1).is_err());
    }
}
