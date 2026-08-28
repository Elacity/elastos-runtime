use serde::Serialize;
use thiserror::Error;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::rights::{validate_active, validate_time_window};
use crate::{
    AtomicReplayClaimer, CanonicalContract, Digest32, ProtectedContentBindingV1,
    RecipientKeyIdentityV1, ReplayClaimError, ReplayClaimKeyV1, ReplayNonce16, RightsActionV1,
    RightsError, VerifiedRightsRequestV1,
};

pub const MAX_RELEASE_REQUEST_LIFETIME_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyReleaseError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Rights(#[from] RightsError),
    #[error(transparent)]
    Replay(#[from] ReplayClaimError),
    #[error("key-release binding mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("node is not a member of the bound node set")]
    UnknownNode,
    #[error("node rights decision denied key release")]
    RightsDenied,
    #[error("node rights decision signature is invalid")]
    InvalidNodeDecisionSignature,
    #[error("node contribution signature is invalid")]
    InvalidNodeContributionSignature,
    #[error("terminal receipt signature is invalid")]
    InvalidTerminalSignature,
    #[error("terminal receipt issuer does not match Runtime selection")]
    UnexpectedTerminalIssuer,
    #[error("terminal receipt has insufficient unique contributions")]
    InsufficientContributions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReleaseAuthorityScopeV1 {
    binding: ProtectedContentBindingV1,
    rights_request_hash: Digest32,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
}

impl CanonicalBody for ReleaseAuthorityScopeV1 {
    const DOMAIN: &'static str = "elastos.protected-content.release-authority-scope/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.recipient.canonical_bytes().map(|_| ())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.fixed(self.rights_request_hash.as_bytes());
        encoder.u8(self.action as u8);
        encoder.nested(&self.recipient)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Ok(Self {
            binding: decoder.nested("binding")?,
            rights_request_hash: Digest32::new(decoder.fixed()?),
            action: RightsActionV1::decode(decoder.u8()?)?,
            recipient: decoder.nested("recipient")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyReleaseRequestV1 {
    binding: ProtectedContentBindingV1,
    rights_request_hash: Digest32,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
    issued_at: u64,
    expires_at: u64,
    replay_nonce: ReplayNonce16,
}

impl KeyReleaseRequestV1 {
    pub fn new(
        binding: ProtectedContentBindingV1,
        rights_request_hash: Digest32,
        action: RightsActionV1,
        recipient: RecipientKeyIdentityV1,
        issued_at: u64,
        expires_at: u64,
        replay_nonce: ReplayNonce16,
    ) -> Result<Self, ContractError> {
        let value = Self {
            binding,
            rights_request_hash,
            action,
            recipient,
            issued_at,
            expires_at,
            replay_nonce,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn rights_request_hash(&self) -> Digest32 {
        self.rights_request_hash
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub fn recipient(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub const fn replay_nonce(&self) -> ReplayNonce16 {
        self.replay_nonce
    }

    pub fn request_hash(&self) -> Result<Digest32, ContractError> {
        self.canonical_hash()
    }

    pub fn replay_claim_key(&self) -> Result<ReplayClaimKeyV1, ContractError> {
        let scope = ReleaseAuthorityScopeV1 {
            binding: self.binding.clone(),
            rights_request_hash: self.rights_request_hash,
            action: self.action,
            recipient: self.recipient.clone(),
        };
        Ok(ReplayClaimKeyV1::new(
            scope.canonical_hash()?,
            self.replay_nonce,
        ))
    }

    pub fn verify(
        &self,
        rights: &VerifiedRightsRequestV1,
        now: u64,
        replay: &mut impl AtomicReplayClaimer,
    ) -> Result<VerifiedKeyReleaseRequestV1, KeyReleaseError> {
        let verified = self.verify_unclaimed(rights, now)?;
        replay.claim(self.replay_claim_key()?, self.expires_at, now)?;
        Ok(verified)
    }

    pub(crate) fn verify_unclaimed(
        &self,
        rights: &VerifiedRightsRequestV1,
        now: u64,
    ) -> Result<VerifiedKeyReleaseRequestV1, KeyReleaseError> {
        self.canonical_bytes()?;
        if self.binding != *rights.binding() {
            return Err(KeyReleaseError::BindingMismatch(
                "protected_content_binding",
            ));
        }
        if self.rights_request_hash != rights.request_hash() {
            return Err(KeyReleaseError::BindingMismatch("rights_request_hash"));
        }
        if self.action != rights.action() {
            return Err(KeyReleaseError::BindingMismatch("rights_action"));
        }
        if self.recipient != *rights.recipient() {
            return Err(KeyReleaseError::BindingMismatch("recipient_key_identity"));
        }
        if self.issued_at < rights.issued_at() || self.expires_at > rights.expires_at() {
            return Err(KeyReleaseError::BindingMismatch("rights_request_window"));
        }
        validate_active(self.issued_at, self.expires_at, now)?;
        Ok(VerifiedKeyReleaseRequestV1 {
            request_hash: self.request_hash()?,
            binding: self.binding.clone(),
            rights_request_hash: self.rights_request_hash,
            action: self.action,
            recipient: self.recipient.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        })
    }
}

impl CanonicalBody for KeyReleaseRequestV1 {
    const DOMAIN: &'static str = "elastos.protected-content.key-release-request/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.recipient.canonical_bytes()?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RELEASE_REQUEST_LIFETIME_SECS,
            "key_release_request_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.fixed(self.rights_request_hash.as_bytes());
        encoder.u8(self.action as u8);
        encoder.nested(&self.recipient)?;
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        encoder.fixed(self.replay_nonce.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("binding")?,
            Digest32::new(decoder.fixed()?),
            RightsActionV1::decode(decoder.u8()?)?,
            decoder.nested("recipient")?,
            decoder.u64()?,
            decoder.u64()?,
            ReplayNonce16::new(decoder.fixed()?),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedKeyReleaseRequestV1 {
    request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    rights_request_hash: Digest32,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
    issued_at: u64,
    expires_at: u64,
}

impl VerifiedKeyReleaseRequestV1 {
    pub const fn request_hash(&self) -> Digest32 {
        self.request_hash
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn rights_request_hash(&self) -> Digest32 {
        self.rights_request_hash
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub fn recipient(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}
