use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::Serialize;
use sha3::{Digest as _, Keccak256};
use thiserror::Error;

use elastos_auth::ethereum_signed_message_hash;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::{
    AtomicReplayClaimer, CanonicalContract, Digest32, ProtectedContentBindingV1,
    RecipientKeyIdentityV1, ReplayClaimError, ReplayClaimKeyV1, WalletAddress,
};

pub const MAX_RIGHTS_REQUEST_LIFETIME_SECS: u64 = 5 * 60;
pub const RIGHTS_CLOCK_SKEW_SECS: u64 = 5;
const WALLET_SIGNATURE_BYTES: usize = 65;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RightsError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Replay(#[from] ReplayClaimError),
    #[error("rights binding mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("Wallet signature does not recover the bound Wallet")]
    WalletMismatch,
    #[error("Wallet signature is invalid or noncanonical")]
    InvalidWalletSignature,
    #[error("rights authority is not yet valid")]
    NotYetValid,
    #[error("rights authority expired")]
    Expired,
    #[error("rights receipt issuer does not match Runtime selection")]
    UnexpectedReceiptIssuer,
    #[error("rights receipt signature is invalid")]
    InvalidReceiptSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum RightsActionV1 {
    View = 1,
    Stream = 2,
    Download = 3,
    Execute = 4,
}

impl RightsActionV1 {
    pub(crate) fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            1 => Ok(Self::View),
            2 => Ok(Self::Stream),
            3 => Ok(Self::Download),
            4 => Ok(Self::Execute),
            _ => Err(ContractError::InvalidField("rights_action")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum RightsDecisionV1 {
    Denied = 0,
    Allowed = 1,
}

impl RightsDecisionV1 {
    pub(crate) fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            0 => Ok(Self::Denied),
            1 => Ok(Self::Allowed),
            _ => Err(ContractError::InvalidField("rights_decision")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplayNonce16([u8; 16]);

impl ReplayNonce16 {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RightsAuthorityScopeV1 {
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
}

impl CanonicalBody for RightsAuthorityScopeV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-authority-scope/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.recipient.canonical_bytes().map(|_| ())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.u8(self.action as u8);
        encoder.nested(&self.recipient)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Ok(Self {
            binding: decoder.nested("binding")?,
            action: RightsActionV1::decode(decoder.u8()?)?,
            recipient: decoder.nested("recipient")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsRequestV1 {
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
    issued_at: u64,
    expires_at: u64,
    replay_nonce: ReplayNonce16,
}

impl RightsRequestV1 {
    pub fn new(
        binding: ProtectedContentBindingV1,
        action: RightsActionV1,
        recipient: RecipientKeyIdentityV1,
        issued_at: u64,
        expires_at: u64,
        replay_nonce: ReplayNonce16,
    ) -> Result<Self, ContractError> {
        let value = Self {
            binding,
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
        let scope = RightsAuthorityScopeV1 {
            binding: self.binding.clone(),
            action: self.action,
            recipient: self.recipient.clone(),
        };
        Ok(ReplayClaimKeyV1::new(
            scope.canonical_hash()?,
            self.replay_nonce,
        ))
    }
}

impl CanonicalBody for RightsRequestV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-request/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.recipient.canonical_bytes()?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RIGHTS_REQUEST_LIFETIME_SECS,
            "rights_request_lifetime",
        )
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.u8(self.action as u8);
        encoder.nested(&self.recipient)?;
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        encoder.fixed(self.replay_nonce.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let binding = decoder.nested("binding")?;
        let action = RightsActionV1::decode(decoder.u8()?)?;
        Self::new(
            binding,
            action,
            decoder.nested("recipient")?,
            decoder.u64()?,
            decoder.u64()?,
            ReplayNonce16::new(decoder.fixed()?),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WalletSignedRightsRequestV1 {
    request: RightsRequestV1,
    wallet_signature: Vec<u8>,
}

impl WalletSignedRightsRequestV1 {
    pub fn new(request: RightsRequestV1, wallet_signature: Vec<u8>) -> Result<Self, ContractError> {
        let value = Self {
            request,
            wallet_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn request(&self) -> &RightsRequestV1 {
        &self.request
    }

    pub fn wallet_signature(&self) -> &[u8] {
        &self.wallet_signature
    }

    pub fn verify(
        &self,
        context: &RightsVerificationContextV1,
        replay: &mut impl AtomicReplayClaimer,
    ) -> Result<VerifiedRightsRequestV1, RightsError> {
        let verified = self.verify_unclaimed(context)?;
        replay.claim(
            self.request.replay_claim_key()?,
            self.request.expires_at,
            context.now,
        )?;
        Ok(verified)
    }

    pub(crate) fn verify_unclaimed(
        &self,
        context: &RightsVerificationContextV1,
    ) -> Result<VerifiedRightsRequestV1, RightsError> {
        self.canonical_bytes()?;
        let request = &self.request;
        if request.binding != context.expected_binding {
            return Err(RightsError::BindingMismatch("protected_content_binding"));
        }
        if request.action != context.expected_action {
            return Err(RightsError::BindingMismatch("rights_action"));
        }
        if request.recipient != context.expected_recipient {
            return Err(RightsError::BindingMismatch("recipient_key_identity"));
        }
        validate_active(request.issued_at, request.expires_at, context.now)?;

        let recovered = recover_wallet(&request.canonical_bytes()?, &self.wallet_signature)?;
        if recovered != request.binding.wallet() {
            return Err(RightsError::WalletMismatch);
        }

        Ok(VerifiedRightsRequestV1 {
            request_hash: request.request_hash()?,
            binding: request.binding.clone(),
            action: request.action,
            recipient: request.recipient.clone(),
            issued_at: request.issued_at,
            expires_at: request.expires_at,
        })
    }
}

impl CanonicalBody for WalletSignedRightsRequestV1 {
    const DOMAIN: &'static str = "elastos.protected-content.wallet-signed-rights-request/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.request.canonical_bytes()?;
        if self.wallet_signature.len() != WALLET_SIGNATURE_BYTES
            || !matches!(self.wallet_signature[64], 0 | 1)
        {
            return Err(ContractError::InvalidField("wallet_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.request)?;
        encoder.bytes(&self.wallet_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("request")?,
            decoder.bytes("wallet_signature", WALLET_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone)]
pub struct RightsVerificationContextV1 {
    expected_binding: ProtectedContentBindingV1,
    expected_action: RightsActionV1,
    expected_recipient: RecipientKeyIdentityV1,
    now: u64,
}

impl RightsVerificationContextV1 {
    pub fn new(
        expected_binding: ProtectedContentBindingV1,
        expected_action: RightsActionV1,
        expected_recipient: RecipientKeyIdentityV1,
        now: u64,
    ) -> Self {
        Self {
            expected_binding,
            expected_action,
            expected_recipient,
            now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRightsRequestV1 {
    request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
    issued_at: u64,
    expires_at: u64,
}

impl VerifiedRightsRequestV1 {
    pub const fn request_hash(&self) -> Digest32 {
        self.request_hash
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
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

pub(crate) fn validate_time_window(
    issued_at: u64,
    expires_at: u64,
    maximum_lifetime: u64,
    field: &'static str,
) -> Result<(), ContractError> {
    if issued_at == 0 || expires_at <= issued_at || expires_at - issued_at > maximum_lifetime {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

pub(crate) fn validate_active(
    issued_at: u64,
    expires_at: u64,
    now: u64,
) -> Result<(), RightsError> {
    if now.saturating_add(RIGHTS_CLOCK_SKEW_SECS) < issued_at {
        return Err(RightsError::NotYetValid);
    }
    if now >= expires_at {
        return Err(RightsError::Expired);
    }
    Ok(())
}

fn recover_wallet(
    canonical_request: &[u8],
    signature_bytes: &[u8],
) -> Result<WalletAddress, RightsError> {
    if signature_bytes.len() != WALLET_SIGNATURE_BYTES {
        return Err(RightsError::InvalidWalletSignature);
    }
    let signature = Signature::from_slice(&signature_bytes[..64])
        .map_err(|_| RightsError::InvalidWalletSignature)?;
    if signature.normalize_s().is_some() {
        return Err(RightsError::InvalidWalletSignature);
    }
    let recovery_id =
        RecoveryId::from_byte(signature_bytes[64]).ok_or(RightsError::InvalidWalletSignature)?;
    let verifying_key = VerifyingKey::recover_from_prehash(
        &ethereum_signed_message_hash(canonical_request),
        &signature,
        recovery_id,
    )
    .map_err(|_| RightsError::InvalidWalletSignature)?;
    let encoded = verifying_key.to_encoded_point(false);
    let public_key = encoded.as_bytes();
    if public_key.len() != 65 || public_key[0] != 4 {
        return Err(RightsError::InvalidWalletSignature);
    }
    let digest = Keccak256::digest(&public_key[1..]);
    Ok(WalletAddress::new(
        digest[12..]
            .try_into()
            .map_err(|_| RightsError::InvalidWalletSignature)?,
    ))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey as ReceiptSigningKey};
    use k256::ecdsa::SigningKey;

    use elastos_auth::ethereum_signed_message_hash;

    use super::*;
    use crate::test_support::{binding_for_wallet, digest, wallet, TestReplayClaims, NOW};
    use crate::{RightsReceiptIssuerKey, RightsReceiptStatementV1, SignedRightsReceiptV1};

    fn wallet_key(seed: u8) -> SigningKey {
        SigningKey::from_slice(&[seed; 32]).unwrap()
    }

    fn request(seed: u8, expires_at: u64, nonce: u8) -> RightsRequestV1 {
        let wallet = wallet(seed);
        RightsRequestV1::new(
            binding_for_wallet(wallet),
            RightsActionV1::View,
            recipient(0xa0),
            NOW,
            expires_at,
            ReplayNonce16::new([nonce; 16]),
        )
        .unwrap()
    }

    fn signature(request: &RightsRequestV1, seed: u8) -> Vec<u8> {
        let (signature, recovery_id) = wallet_key(seed)
            .sign_prehash_recoverable(&ethereum_signed_message_hash(
                &request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut bytes = signature.to_bytes().to_vec();
        bytes.push(recovery_id.to_byte());
        bytes
    }

    fn signed(seed: u8) -> WalletSignedRightsRequestV1 {
        let request = request(seed, NOW + 120, 0x55);
        WalletSignedRightsRequestV1::new(request.clone(), signature(&request, seed)).unwrap()
    }

    fn context(request: &RightsRequestV1, now: u64) -> RightsVerificationContextV1 {
        RightsVerificationContextV1::new(
            request.binding().clone(),
            request.action(),
            request.recipient().clone(),
            now,
        )
    }

    fn recipient(seed: u8) -> RecipientKeyIdentityV1 {
        RecipientKeyIdentityV1::new("x25519-hkdf-sha256-aes256gcm/v1", digest(seed)).unwrap()
    }

    #[test]
    fn wallet_provider_zero_one_signature_is_the_only_canonical_form() {
        let signed = signed(7);
        assert!(matches!(signed.wallet_signature()[64], 0 | 1));
        let decoded =
            WalletSignedRightsRequestV1::from_canonical_bytes(&signed.canonical_bytes().unwrap())
                .unwrap();
        let mut replay = TestReplayClaims::default();
        decoded
            .verify(&context(decoded.request(), NOW + 1), &mut replay)
            .unwrap();

        let mut noncanonical_signature = signed.wallet_signature().to_vec();
        noncanonical_signature[64] += 27;
        assert_eq!(
            WalletSignedRightsRequestV1::new(signed.request().clone(), noncanonical_signature),
            Err(ContractError::InvalidField("wallet_signature"))
        );
        let mut noncanonical_wire = signed.canonical_bytes().unwrap();
        *noncanonical_wire.last_mut().unwrap() += 27;
        assert_eq!(
            WalletSignedRightsRequestV1::from_canonical_bytes(&noncanonical_wire),
            Err(ContractError::InvalidField("wallet_signature"))
        );
    }

    #[test]
    fn replay_claim_is_required_and_nonce_keyed_across_field_mutation() {
        let first = signed(7);
        let changed = request(7, NOW + 180, 0x55);
        let changed =
            WalletSignedRightsRequestV1::new(changed.clone(), signature(&changed, 7)).unwrap();
        let mut replay = TestReplayClaims::default();
        first
            .verify(&context(first.request(), NOW + 1), &mut replay)
            .unwrap();
        assert_eq!(
            changed.verify(&context(changed.request(), NOW + 1), &mut replay),
            Err(RightsError::Replay(ReplayClaimError::AlreadyClaimed))
        );
    }

    #[test]
    fn post_signature_field_mutation_fails_closed() {
        let signed = signed(7);
        let changed_request = RightsRequestV1::new(
            signed.request().binding().clone(),
            RightsActionV1::Stream,
            signed.request().recipient().clone(),
            signed.request().issued_at(),
            signed.request().expires_at(),
            signed.request().replay_nonce(),
        )
        .unwrap();
        let changed = WalletSignedRightsRequestV1::new(
            changed_request.clone(),
            signed.wallet_signature().to_vec(),
        )
        .unwrap();
        let mut replay = TestReplayClaims::default();
        assert_eq!(
            changed.verify(&context(&changed_request, NOW + 1), &mut replay),
            Err(RightsError::WalletMismatch)
        );
        assert!(replay.is_empty());
    }

    #[test]
    fn attacker_cannot_sign_a_victim_wallet_that_owns_content() {
        let victim_request = request(9, NOW + 120, 0x66);
        let attacker_signed =
            WalletSignedRightsRequestV1::new(victim_request.clone(), signature(&victim_request, 7))
                .unwrap();
        let mut replay = TestReplayClaims::default();
        assert_eq!(
            attacker_signed.verify(&context(&victim_request, NOW + 1), &mut replay),
            Err(RightsError::WalletMismatch)
        );
        assert!(replay.is_empty());
    }

    #[test]
    fn recipient_is_wallet_signed_and_runtime_selected() {
        let signed = signed(7);
        let changed_request = RightsRequestV1::new(
            signed.request().binding().clone(),
            RightsActionV1::View,
            recipient(0xa1),
            signed.request().issued_at(),
            signed.request().expires_at(),
            signed.request().replay_nonce(),
        )
        .unwrap();
        let changed = WalletSignedRightsRequestV1::new(
            changed_request.clone(),
            signed.wallet_signature().to_vec(),
        )
        .unwrap();
        let mut replay = TestReplayClaims::default();
        assert_eq!(
            changed.verify(&context(&changed_request, NOW + 1), &mut replay),
            Err(RightsError::WalletMismatch)
        );
        assert!(replay.is_empty());

        let mut replay = TestReplayClaims::default();
        let wrong_runtime_recipient = RightsVerificationContextV1::new(
            signed.request().binding().clone(),
            signed.request().action(),
            recipient(0xa1),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_runtime_recipient, &mut replay),
            Err(RightsError::BindingMismatch("recipient_key_identity"))
        );
        assert!(replay.is_empty());
    }

    #[test]
    fn forged_allow_receipt_and_lifetime_escape_fail() {
        let signed = signed(7);
        let mut replay = TestReplayClaims::default();
        let verified = signed
            .verify(&context(signed.request(), NOW + 1), &mut replay)
            .unwrap();
        let issuer_key = ReceiptSigningKey::from_bytes(&[11; 32]);
        let issuer = RightsReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
        let statement = RightsReceiptStatementV1::new(
            verified.request_hash(),
            verified.binding().clone(),
            verified.action(),
            issuer,
            RightsDecisionV1::Allowed,
            digest(0x90),
            NOW + 2,
            NOW + 60,
        )
        .unwrap();
        let forged = SignedRightsReceiptV1::new(statement.clone(), vec![0x44; 64]).unwrap();
        assert_eq!(
            forged.verify_audit(&verified, issuer, NOW + 3),
            Err(RightsError::InvalidReceiptSignature)
        );

        let valid = SignedRightsReceiptV1::new(
            statement.clone(),
            issuer_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        valid.verify_audit(&verified, issuer, NOW + 3).unwrap();

        let beyond_parent = RightsReceiptStatementV1::new(
            verified.request_hash(),
            verified.binding().clone(),
            verified.action(),
            issuer,
            RightsDecisionV1::Allowed,
            digest(0x90),
            NOW + 2,
            verified.expires_at() + 1,
        )
        .unwrap();
        let beyond_parent = SignedRightsReceiptV1::new(
            beyond_parent.clone(),
            issuer_key
                .sign(&beyond_parent.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            beyond_parent.verify_audit(&verified, issuer, NOW + 3),
            Err(RightsError::BindingMismatch("rights_receipt_window"))
        );
    }

    #[test]
    fn wrong_profile_binding_fails_before_replay_claim() {
        let signed = signed(7);
        let wrong_profile = ProtectedContentBindingV1::new(
            signed.request().binding().encrypted_content().clone(),
            signed.request().binding().key_envelope().clone(),
            signed.request().binding().rights_policy().clone(),
            crate::ProfileIdentityV1::from_public_key_bytes(
                ReceiptSigningKey::from_bytes(&[12; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            signed.request().binding().wallet(),
            signed.request().binding().runtime_session_binding(),
        )
        .unwrap();
        let mut replay = TestReplayClaims::default();
        let wrong_context = RightsVerificationContextV1::new(
            wrong_profile,
            signed.request().action(),
            signed.request().recipient().clone(),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_context, &mut replay),
            Err(RightsError::BindingMismatch("protected_content_binding"))
        );
        assert!(replay.is_empty());
    }

    #[test]
    fn wrong_policy_and_expiry_fail_before_replay_claim() {
        let signed = signed(7);
        let mut wrong = binding_for_wallet(wallet(7));
        // Rebuild through public constructors so the immutable policy digest is
        // the only changed authority field.
        wrong = ProtectedContentBindingV1::new(
            wrong.encrypted_content().clone(),
            wrong.key_envelope().clone(),
            crate::RightsPolicyIdentityV1::new(digest(0xee), 384).unwrap(),
            wrong.profile(),
            wrong.wallet(),
            wrong.runtime_session_binding(),
        )
        .unwrap();
        let mut replay = TestReplayClaims::default();
        let wrong_context = RightsVerificationContextV1::new(
            wrong,
            RightsActionV1::View,
            signed.request().recipient().clone(),
            NOW + 1,
        );
        assert_eq!(
            signed.verify(&wrong_context, &mut replay),
            Err(RightsError::BindingMismatch("protected_content_binding"))
        );
        assert!(replay.is_empty());

        let mut replay = TestReplayClaims::default();
        assert_eq!(
            signed.verify(&context(signed.request(), NOW + 120), &mut replay),
            Err(RightsError::Expired)
        );
        assert!(replay.is_empty());
    }
}
