//! Holder-only recipient possession and decrypt-session CEK wrap.
//!
//! Profile signature authorizes one wrap public key. It is not possession.
//! The decrypt boundary must open a PQ-hybrid challenge sealed to that exact
//! key before reconstruction. Reconstructed CEK bytes leave this crate only
//! inside a PQ-hybrid decrypt-session wrap.

use hpke::rand_core::{CryptoRng, RngCore};
use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    CanonicalContract, Digest32, PqHybridSealedShareV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, RuntimeSessionBindingV1,
    PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
};

use crate::{
    secrets::{ContentEncryptionKeyV1, RecipientPublicKeyV1, RecipientSecretKeyV1},
    share_wrap::{open_share, seal_share},
    CustodyError, CONTENT_KEY_BYTES,
};

const DECRYPT_SESSION_SEED_LABEL: &[u8] =
    b"elastos.protected-content.decrypt-session-seed/pq-hybrid/v1";
const POSSESSION_TRANSCRIPT_DOMAIN: &[u8] =
    b"elastos.protected-content.recipient-possession-transcript/v1";
pub(crate) const POSSESSION_CHALLENGE_INFO_V1: &[u8] =
    b"elastos.protected-content.recipient-possession/v1";
pub(crate) const DECRYPT_SESSION_CEK_INFO_V1: &[u8] =
    b"elastos.protected-content.decrypt-session-cek/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecryptSessionPublicKeyV1([u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES]);

pub struct DecryptSessionSecretKeyV1(Zeroizing<[u8; 32]>);

pub struct RecipientPossessionChallengeV1 {
    sealed: PqHybridSealedShareV1,
}

pub struct VerifiedRecipientPossessionV1 {
    secret: RecipientSecretKeyV1,
    identity: RecipientKeyIdentityV1,
    transcript_digest: Digest32,
}

pub struct DecryptSessionWrappedContentKeyV1 {
    sealed: PqHybridSealedShareV1,
}

impl DecryptSessionPublicKeyV1 {
    pub fn new(bytes: [u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES]) -> Result<Self, CustodyError> {
        RecipientPublicKeyV1::new(bytes).map(|_| Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
        &self.0
    }
}

impl DecryptSessionSecretKeyV1 {
    pub fn from_seed(seed: [u8; 32]) -> Result<Self, CustodyError> {
        let secret = Self(labeled_secret(DECRYPT_SESSION_SEED_LABEL, &seed)?);
        secret.public_key()?;
        Ok(secret)
    }

    pub fn public_key(&self) -> Result<DecryptSessionPublicKeyV1, CustodyError> {
        let public =
            RecipientSecretKeyV1::from_guarded_bytes(Zeroizing::new(*self.0))?.public_key()?;
        DecryptSessionPublicKeyV1::new(*public.as_bytes())
    }

    pub(crate) fn secret_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for DecryptSessionSecretKeyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DecryptSessionSecretKeyV1([redacted])")
    }
}

impl RecipientPossessionChallengeV1 {
    pub fn sealed_share(&self) -> &PqHybridSealedShareV1 {
        &self.sealed
    }
}

impl VerifiedRecipientPossessionV1 {
    pub fn identity(&self) -> &RecipientKeyIdentityV1 {
        &self.identity
    }

    pub fn transcript_digest(&self) -> Digest32 {
        self.transcript_digest
    }

    pub(crate) fn secret(&self) -> &RecipientSecretKeyV1 {
        &self.secret
    }
}

impl std::fmt::Debug for VerifiedRecipientPossessionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedRecipientPossessionV1([redacted])")
    }
}

impl DecryptSessionWrappedContentKeyV1 {
    pub fn sealed_share(&self) -> &PqHybridSealedShareV1 {
        &self.sealed
    }
}

pub fn mint_decrypt_session_from_seed(
    seed: [u8; 32],
) -> Result<(DecryptSessionSecretKeyV1, DecryptSessionPublicKeyV1), CustodyError> {
    let secret = DecryptSessionSecretKeyV1::from_seed(seed)?;
    let public = secret.public_key()?;
    Ok((secret, public))
}

pub fn possession_transcript_v1(
    profile: ProfileIdentityV1,
    session: RuntimeSessionBindingV1,
    recipient: &RecipientKeyIdentityV1,
    request_hash: Digest32,
) -> Result<Vec<u8>, CustodyError> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(POSSESSION_TRANSCRIPT_DOMAIN);
    transcript.extend_from_slice(profile.public_key_bytes());
    transcript.extend_from_slice(session.digest().as_bytes());
    transcript.extend_from_slice(&recipient.canonical_bytes()?);
    transcript.extend_from_slice(request_hash.as_bytes());
    Ok(transcript)
}

pub fn issue_recipient_possession_challenge<R: CryptoRng + RngCore>(
    authorized_public: &RecipientPublicKeyV1,
    transcript: &[u8],
    rng: &mut R,
) -> Result<RecipientPossessionChallengeV1, CustodyError> {
    let mut nonce = [0u8; CONTENT_KEY_BYTES];
    rng.fill_bytes(&mut nonce);
    let sealed = seal_share(
        authorized_public.as_bytes(),
        POSSESSION_CHALLENGE_INFO_V1,
        transcript,
        &nonce,
        rng,
    )?;
    Ok(RecipientPossessionChallengeV1 { sealed })
}

pub fn prove_recipient_possession<R: CryptoRng + RngCore>(
    recipient_secret: &RecipientSecretKeyV1,
    binding: &ProtectedContentBindingV1,
    authorized_recipient: &RecipientKeyIdentityV1,
    request_hash: Digest32,
    rng: &mut R,
) -> Result<VerifiedRecipientPossessionV1, CustodyError> {
    let identity = recipient_secret.identity()?;
    if identity != *authorized_recipient {
        return Err(CustodyError::BindingMismatch("recipient_key_identity"));
    }
    let public = recipient_secret.public_key()?;
    if !authorized_recipient.matches_public_key(public.as_bytes()) {
        return Err(CustodyError::BindingMismatch("recipient_public_key"));
    }
    let transcript = possession_transcript_v1(
        binding.profile(),
        binding.runtime_session_binding(),
        authorized_recipient,
        request_hash,
    )?;
    let challenge = issue_recipient_possession_challenge(&public, &transcript, rng)?;
    answer_recipient_possession_challenge(
        recipient_secret,
        &public,
        authorized_recipient,
        &challenge,
        &transcript,
    )
}

pub fn answer_recipient_possession_challenge(
    recipient_secret: &RecipientSecretKeyV1,
    authorized_public: &RecipientPublicKeyV1,
    authorized_recipient: &RecipientKeyIdentityV1,
    challenge: &RecipientPossessionChallengeV1,
    transcript: &[u8],
) -> Result<VerifiedRecipientPossessionV1, CustodyError> {
    if recipient_secret.public_key()? != *authorized_public {
        return Err(CustodyError::BindingMismatch("recipient_public_key"));
    }
    if recipient_secret.identity()? != *authorized_recipient {
        return Err(CustodyError::BindingMismatch("recipient_key_identity"));
    }
    open_share(
        &challenge.sealed,
        recipient_secret.secret_bytes(),
        POSSESSION_CHALLENGE_INFO_V1,
        transcript,
    )
    .map_err(|_| CustodyError::BindingMismatch("recipient_possession"))?;
    Ok(VerifiedRecipientPossessionV1 {
        secret: recipient_secret.duplicate(),
        identity: authorized_recipient.clone(),
        transcript_digest: Digest32::new(Sha256::digest(transcript).into()),
    })
}

pub(crate) fn require_recipient_possession(
    recipient_secret: &RecipientSecretKeyV1,
    binding: &ProtectedContentBindingV1,
    authorized_recipient: &RecipientKeyIdentityV1,
    request_hash: Digest32,
) -> Result<VerifiedRecipientPossessionV1, CustodyError> {
    let mut rng = HpkeStdRng::try_from_os_rng().map_err(|_| CustodyError::RandomnessUnavailable)?;
    prove_recipient_possession(
        recipient_secret,
        binding,
        authorized_recipient,
        request_hash,
        &mut rng,
    )
}

pub fn wrap_content_key_to_decrypt_session<R: CryptoRng + RngCore>(
    content_key: &ContentEncryptionKeyV1,
    session_public: &DecryptSessionPublicKeyV1,
    transcript: &[u8],
    rng: &mut R,
) -> Result<DecryptSessionWrappedContentKeyV1, CustodyError> {
    let sealed = content_key.with_bytes(|cek| {
        seal_share(
            session_public.as_bytes(),
            DECRYPT_SESSION_CEK_INFO_V1,
            transcript,
            cek,
            rng,
        )
    })?;
    Ok(DecryptSessionWrappedContentKeyV1 { sealed })
}

pub fn unwrap_content_key_in_decrypt_session(
    session_secret: &DecryptSessionSecretKeyV1,
    wrapped: &DecryptSessionWrappedContentKeyV1,
    transcript: &[u8],
) -> Result<ContentEncryptionKeyV1, CustodyError> {
    let opened = open_share(
        wrapped.sealed_share(),
        session_secret.secret_bytes(),
        DECRYPT_SESSION_CEK_INFO_V1,
        transcript,
    )?;
    Ok(ContentEncryptionKeyV1::from_guarded_bytes(opened))
}

fn labeled_secret(label: &[u8], seed: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, CustodyError> {
    if *seed == [0u8; 32] {
        return Err(CustodyError::BindingMismatch("decrypt_session_seed"));
    }
    let mut hasher = Sha256::new();
    hasher.update(label);
    hasher.update(seed);
    Ok(Zeroizing::new(hasher.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        provisioned_envelope, recipient_public_key, recipient_secret,
        verified_release_request_for_envelope_and_recipient_seed,
    };
    use rand09::rngs::StdRng as HpkeStdRng;

    #[test]
    fn profile_authorization_without_matching_secret_is_not_possession() {
        let request =
            verified_release_request_for_envelope_and_recipient_seed(&provisioned_envelope(), 0x30);
        let authorized_public = recipient_public_key(0x30);
        let transcript = possession_transcript_v1(
            request.binding().profile(),
            request.binding().runtime_session_binding(),
            request.recipient(),
            request.request_hash(),
        )
        .unwrap();
        let challenge = issue_recipient_possession_challenge(
            &authorized_public,
            &transcript,
            &mut HpkeStdRng::from_seed([0x61; 32]),
        )
        .unwrap();

        let err = answer_recipient_possession_challenge(
            &recipient_secret(0x31),
            &authorized_public,
            request.recipient(),
            &challenge,
            &transcript,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("recipient_public_key")
        ));

        let err = prove_recipient_possession(
            &recipient_secret(0x31),
            request.binding(),
            request.recipient(),
            request.request_hash(),
            &mut HpkeStdRng::from_seed([0x62; 32]),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("recipient_key_identity")
        ));
    }

    #[test]
    fn possession_fails_closed_on_wrong_transcript() {
        let request =
            verified_release_request_for_envelope_and_recipient_seed(&provisioned_envelope(), 0x30);
        let public = recipient_public_key(0x30);
        let transcript = possession_transcript_v1(
            request.binding().profile(),
            request.binding().runtime_session_binding(),
            request.recipient(),
            request.request_hash(),
        )
        .unwrap();
        let challenge = issue_recipient_possession_challenge(
            &public,
            &transcript,
            &mut HpkeStdRng::from_seed([0x61; 32]),
        )
        .unwrap();
        let mut wrong = transcript.clone();
        wrong.extend_from_slice(&[0xff]);
        let err = answer_recipient_possession_challenge(
            &recipient_secret(0x30),
            &public,
            request.recipient(),
            &challenge,
            &wrong,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("recipient_possession")
        ));
    }

    #[test]
    fn decrypt_session_wrap_hides_cek_and_rejects_wrong_session() {
        let content_key = crate::test_support::content_key();
        let cek = content_key.with_bytes(|bytes| *bytes);
        let (session_secret, session_public) = mint_decrypt_session_from_seed([0x51; 32]).unwrap();
        let transcript = b"elastos-decrypt-session-wrap-test/v1";
        let wrapped = wrap_content_key_to_decrypt_session(
            &content_key,
            &session_public,
            transcript,
            &mut HpkeStdRng::from_seed([0x71; 32]),
        )
        .unwrap();
        let sealed_bytes = wrapped.sealed_share().canonical_bytes().unwrap();
        assert!(!sealed_bytes.windows(cek.len()).any(|window| window == cek));

        let opened =
            unwrap_content_key_in_decrypt_session(&session_secret, &wrapped, transcript).unwrap();
        assert_eq!(opened.with_bytes(|bytes| *bytes), cek);

        let (wrong_secret, _) = mint_decrypt_session_from_seed([0x52; 32]).unwrap();
        assert!(matches!(
            unwrap_content_key_in_decrypt_session(&wrong_secret, &wrapped, transcript),
            Err(CustodyError::PqHybrid(_) | CustodyError::MalformedShare(_))
        ));
    }
}
