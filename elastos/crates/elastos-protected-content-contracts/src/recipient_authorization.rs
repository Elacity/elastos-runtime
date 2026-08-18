use ed25519_dalek::{Signature, Verifier as _};
use serde::Serialize;
use sha2::Digest as _;
use thiserror::Error;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::custody_envelope::validate_canonical_x25519_public_key;
use crate::rights::{validate_active, validate_time_window, RightsActionV1, RightsError};
use crate::{CanonicalContract, Digest32, ProtectedContentBindingV1, RecipientKeyIdentityV1};

pub const MAX_RECIPIENT_KEY_AUTHORIZATION_LIFETIME_SECS: u64 = 5 * 60;
const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RecipientPublicKeyBytesV1([u8; 32]);

impl RecipientPublicKeyBytesV1 {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        validate_canonical_x25519_public_key(bytes, "recipient_public_key")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn key_identity(
        &self,
        encryption_suite_id: &str,
    ) -> Result<RecipientKeyIdentityV1, ContractError> {
        RecipientKeyIdentityV1::new(
            encryption_suite_id,
            Digest32::new(sha2::Sha256::digest(self.0).into()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RuntimeOperationIssuerKeyV1([u8; 32]);

impl RuntimeOperationIssuerKeyV1 {
    pub fn new(bytes: [u8; 32]) -> Result<Self, ContractError> {
        crate::identity::validate_ed25519_public_key(bytes, "runtime_operation_issuer")?;
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecipientAuthorizationError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("recipient-key authorization mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("recipient-key authorization is not yet valid")]
    NotYetValid,
    #[error("recipient-key authorization expired")]
    Expired,
    #[error("recipient-key authorization signature is invalid")]
    InvalidProfileSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecipientKeyAuthorizationStatementV1 {
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    recipient_public_key: RecipientPublicKeyBytesV1,
    recipient_identity: RecipientKeyIdentityV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    issued_at: u64,
    expires_at: u64,
}

impl RecipientKeyAuthorizationStatementV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding: ProtectedContentBindingV1,
        action: RightsActionV1,
        recipient_public_key: RecipientPublicKeyBytesV1,
        recipient_identity: RecipientKeyIdentityV1,
        runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            binding,
            action,
            recipient_public_key,
            recipient_identity,
            runtime_operation_issuer,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub const fn recipient_public_key(&self) -> RecipientPublicKeyBytesV1 {
        self.recipient_public_key
    }

    pub fn recipient_identity(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient_identity
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl CanonicalBody for RecipientKeyAuthorizationStatementV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.recipient-key-authorization-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.recipient_identity.canonical_bytes()?;
        RecipientPublicKeyBytesV1::new(*self.recipient_public_key.as_bytes())?;
        RuntimeOperationIssuerKeyV1::new(*self.runtime_operation_issuer.as_bytes())?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RECIPIENT_KEY_AUTHORIZATION_LIFETIME_SECS,
            "recipient_key_authorization_lifetime",
        )?;
        if !self
            .recipient_identity
            .matches_public_key(self.recipient_public_key.as_bytes())
        {
            return Err(ContractError::InvalidField("recipient_key_identity"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.u8(self.action as u8);
        encoder.fixed(self.recipient_public_key.as_bytes());
        encoder.nested(&self.recipient_identity)?;
        encoder.fixed(self.runtime_operation_issuer.as_bytes());
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("binding")?,
            RightsActionV1::decode(decoder.u8()?)?,
            RecipientPublicKeyBytesV1::new(decoder.fixed()?)?,
            decoder.nested("recipient_key_identity")?,
            RuntimeOperationIssuerKeyV1::new(decoder.fixed()?)?,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedRecipientKeyAuthorizationV1 {
    statement: RecipientKeyAuthorizationStatementV1,
    profile_signature: Vec<u8>,
}

impl SignedRecipientKeyAuthorizationV1 {
    pub fn new(
        statement: RecipientKeyAuthorizationStatementV1,
        profile_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            profile_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &RecipientKeyAuthorizationStatementV1 {
        &self.statement
    }

    pub fn verify(
        &self,
        context: &RecipientKeyAuthorizationContextV1,
    ) -> Result<VerifiedRecipientKeyAuthorizationV1, RecipientAuthorizationError> {
        self.canonical_bytes()?;
        if self.statement.binding != context.expected_binding {
            return Err(RecipientAuthorizationError::BindingMismatch(
                "protected_content_binding",
            ));
        }
        if self.statement.action != context.expected_action {
            return Err(RecipientAuthorizationError::BindingMismatch(
                "rights_action",
            ));
        }
        if self.statement.recipient_public_key != context.expected_recipient_public_key {
            return Err(RecipientAuthorizationError::BindingMismatch(
                "recipient_public_key",
            ));
        }
        if self.statement.runtime_operation_issuer != context.expected_runtime_operation_issuer {
            return Err(RecipientAuthorizationError::BindingMismatch(
                "runtime_operation_issuer",
            ));
        }
        if !self
            .statement
            .recipient_identity
            .matches_public_key(self.statement.recipient_public_key.as_bytes())
        {
            return Err(RecipientAuthorizationError::BindingMismatch(
                "recipient_key_identity",
            ));
        }
        map_active(
            self.statement.issued_at,
            self.statement.expires_at,
            context.now,
        )?;
        let signature = Signature::from_bytes(
            &self
                .profile_signature
                .clone()
                .try_into()
                .map_err(|_| RecipientAuthorizationError::InvalidProfileSignature)?,
        );
        let profile_key = crate::identity::validate_ed25519_public_key(
            *self.statement.binding.profile().public_key_bytes(),
            "profile_public_key",
        )
        .map_err(|_| RecipientAuthorizationError::InvalidProfileSignature)?;
        profile_key
            .verify(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| RecipientAuthorizationError::InvalidProfileSignature)?;
        Ok(VerifiedRecipientKeyAuthorizationV1 {
            statement_hash: self.statement.canonical_hash()?,
            binding: self.statement.binding.clone(),
            action: self.statement.action,
            recipient_public_key: self.statement.recipient_public_key,
            recipient_identity: self.statement.recipient_identity.clone(),
            runtime_operation_issuer: self.statement.runtime_operation_issuer,
            issued_at: self.statement.issued_at,
            expires_at: self.statement.expires_at,
        })
    }
}

impl CanonicalBody for SignedRecipientKeyAuthorizationV1 {
    const DOMAIN: &'static str = "elastos.protected-content.recipient-key-authorization/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.profile_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("profile_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.profile_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes("profile_signature", ED25519_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone)]
pub struct RecipientKeyAuthorizationContextV1 {
    expected_binding: ProtectedContentBindingV1,
    expected_action: RightsActionV1,
    expected_recipient_public_key: RecipientPublicKeyBytesV1,
    expected_runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    now: u64,
}

impl RecipientKeyAuthorizationContextV1 {
    pub fn new(
        expected_binding: ProtectedContentBindingV1,
        expected_action: RightsActionV1,
        expected_recipient_public_key: RecipientPublicKeyBytesV1,
        expected_runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        now: u64,
    ) -> Self {
        Self {
            expected_binding,
            expected_action,
            expected_recipient_public_key,
            expected_runtime_operation_issuer,
            now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRecipientKeyAuthorizationV1 {
    statement_hash: Digest32,
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    recipient_public_key: RecipientPublicKeyBytesV1,
    recipient_identity: RecipientKeyIdentityV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    issued_at: u64,
    expires_at: u64,
}

impl VerifiedRecipientKeyAuthorizationV1 {
    pub const fn statement_hash(&self) -> Digest32 {
        self.statement_hash
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub const fn recipient_public_key(&self) -> RecipientPublicKeyBytesV1 {
        self.recipient_public_key
    }

    pub fn recipient_identity(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient_identity
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

fn map_active(
    issued_at: u64,
    expires_at: u64,
    now: u64,
) -> Result<(), RecipientAuthorizationError> {
    match validate_active(issued_at, expires_at, now) {
        Ok(()) => Ok(()),
        Err(RightsError::NotYetValid) => Err(RecipientAuthorizationError::NotYetValid),
        Err(RightsError::Expired) => Err(RecipientAuthorizationError::Expired),
        Err(RightsError::Contract(error)) => Err(RecipientAuthorizationError::Contract(error)),
        Err(_) => Err(RecipientAuthorizationError::InvalidProfileSignature),
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};
    use hex::encode;

    use super::*;
    use crate::test_support::{binding_for_wallet, digest, wallet, NOW};

    fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
        let key = SigningKey::from_bytes(&[seed; 32]);
        RuntimeOperationIssuerKeyV1::new(key.verifying_key().to_bytes()).unwrap()
    }

    fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
        let mut bytes = [0u8; 32];
        bytes[0] = seed.max(9);
        RecipientPublicKeyBytesV1::new(bytes).unwrap()
    }

    fn signed_authorization(seed: u8) -> SignedRecipientKeyAuthorizationV1 {
        let binding = binding_for_wallet(wallet(7));
        let profile = SigningKey::from_bytes(&[0x26; 32]);
        let recipient_public_key = recipient_public_key(seed);
        let statement = RecipientKeyAuthorizationStatementV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_public_key,
            recipient_public_key
                .key_identity("hpke-rfc9180-base-x25519-hkdf-sha256-aes256gcm/v1")
                .unwrap(),
            runtime_issuer(0x42),
            NOW,
            NOW + 120,
        )
        .unwrap();
        let signature = profile
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec();
        SignedRecipientKeyAuthorizationV1::new(statement, signature).unwrap()
    }

    #[test]
    fn recipient_authorization_binds_profile_recipient_and_runtime_issuer() {
        let signed = signed_authorization(0x11);
        let context = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            signed.statement().runtime_operation_issuer(),
            NOW + 1,
        );
        let verified = signed.verify(&context).unwrap();
        assert_eq!(
            encode(verified.statement_hash().as_bytes()),
            "741d092465bb9b16a16e62b7ad521ae54d56b14e5b25c6af6855a3f810b8dabd"
        );
    }

    #[test]
    fn recipient_authorization_rejects_wrong_identity_issuer_and_window() {
        let signed = signed_authorization(0x11);
        let wrong_recipient = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            recipient_public_key(0x22),
            signed.statement().runtime_operation_issuer(),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_recipient),
            Err(RecipientAuthorizationError::BindingMismatch(
                "recipient_public_key"
            ))
        );

        let wrong_issuer = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            runtime_issuer(0x77),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_issuer),
            Err(RecipientAuthorizationError::BindingMismatch(
                "runtime_operation_issuer"
            ))
        );

        let future = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            signed.statement().runtime_operation_issuer(),
            NOW - 10,
        );
        assert_eq!(
            signed.verify(&future),
            Err(RecipientAuthorizationError::NotYetValid)
        );

        let expired = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            signed.statement().runtime_operation_issuer(),
            signed.statement().expires_at(),
        );
        assert_eq!(
            signed.verify(&expired),
            Err(RecipientAuthorizationError::Expired)
        );
    }

    #[test]
    fn recipient_authorization_rejects_wrong_profile_action_and_identity_bytes() {
        let signed = signed_authorization(0x11);
        let mut wrong_profile_binding = signed.statement().binding().clone();
        wrong_profile_binding = ProtectedContentBindingV1::new(
            wrong_profile_binding.encrypted_content().clone(),
            wrong_profile_binding.key_envelope().clone(),
            wrong_profile_binding.rights_policy().clone(),
            crate::ProfileIdentityV1::from_public_key_bytes(
                SigningKey::from_bytes(&[0x61; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            wrong_profile_binding.wallet(),
            wrong_profile_binding.runtime_session_binding(),
        )
        .unwrap();
        let wrong_profile = RecipientKeyAuthorizationContextV1::new(
            wrong_profile_binding,
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            signed.statement().runtime_operation_issuer(),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_profile),
            Err(RecipientAuthorizationError::BindingMismatch(
                "protected_content_binding"
            ))
        );

        let wrong_action = RecipientKeyAuthorizationContextV1::new(
            signed.statement().binding().clone(),
            RightsActionV1::Download,
            signed.statement().recipient_public_key(),
            signed.statement().runtime_operation_issuer(),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_action),
            Err(RecipientAuthorizationError::BindingMismatch(
                "rights_action"
            ))
        );

        let statement = RecipientKeyAuthorizationStatementV1::new(
            signed.statement().binding().clone(),
            signed.statement().action(),
            signed.statement().recipient_public_key(),
            RecipientKeyIdentityV1::new(
                "hpke-rfc9180-base-x25519-hkdf-sha256-aes256gcm/v1",
                digest(0xee),
            )
            .unwrap(),
            signed.statement().runtime_operation_issuer(),
            NOW,
            NOW + 120,
        )
        .unwrap_err();
        assert_eq!(
            statement,
            ContractError::InvalidField("recipient_key_identity")
        );
    }
}
