use ed25519_dalek::{Signer as _, SigningKey};
use k256::ecdsa::SigningKey as WalletSigningKey;
use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};
use rand10::{rngs::StdRng as ShamirStdRng, SeedableRng as _};
use sha2::Digest as _;
use sha3::Keccak256;

use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    AtomicReplayClaimer, AuthenticatedRuntimeReleaseOperationV1, CanonicalContract,
    CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeV1,
    CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
    CustodyPoolIdentityV1, Digest32, EncryptedContentIdentityV1, EvmContractAddressV1,
    EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyReleaseRequestV1, NodeCustodyPublicKeyV1,
    NodePublicKey, ProtectedContentBindingV1, RecipientKeyIdentityV1, RecipientPublicKeyBytesV1,
    ReplayClaimError, ReplayClaimKeyV1, ReplayNonce16, RightsActionV1, RightsDecisionV1,
    RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
    RightsRequestV1, RightsSubjectSourceV1, RightsVerificationContextV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    RuntimeSessionBindingV1, SignedCustodyEpochV1, SignedNodeRightsDecisionV1,
    SignedRecipientKeyAuthorizationV1, SignedRuntimeReleaseOperationV1, ThresholdV1,
    VerifiedKeyReleaseRequestV1, WalletAddress, WalletSignedRightsRequestV1,
    CUSTODY_HPKE_SUITE_ID_V1,
};

use crate::{
    provision::provision_custody_envelope_with_rng, replay_store::ClaimedNodeReleaseOperationV1,
    ContentEncryptionKeyV1, DurableReplayClaimStoreV1, NodeCustodySecretKeyV1,
    RecipientPublicKeyV1, RecipientSecretKeyV1,
};

pub(crate) const NOW: u64 = 2_000_000_000;

#[derive(Debug, Default)]
struct TestReplayClaims(std::collections::HashMap<ReplayClaimKeyV1, u64>);

impl AtomicReplayClaimer for TestReplayClaims {
    fn claim(
        &mut self,
        key: ReplayClaimKeyV1,
        expires_at: u64,
        now: u64,
    ) -> Result<(), ReplayClaimError> {
        self.0.retain(|_, expiry| *expiry > now);
        if self.0.contains_key(&key) {
            return Err(ReplayClaimError::AlreadyClaimed);
        }
        self.0.insert(key, expires_at);
        Ok(())
    }
}

pub(crate) fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

pub(crate) fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub(crate) fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

pub(crate) fn node_custody_secret(seed: u8) -> NodeCustodySecretKeyV1 {
    NodeCustodySecretKeyV1::from_test_bytes([seed; 32])
}

pub(crate) fn recipient_secret(seed: u8) -> RecipientSecretKeyV1 {
    RecipientSecretKeyV1::from_test_bytes([seed; 32])
}

pub(crate) fn recipient_public_key(seed: u8) -> RecipientPublicKeyV1 {
    recipient_secret(seed).public_key().unwrap()
}

fn recipient_identity_with_suite(seed: u8, suite: &str) -> RecipientKeyIdentityV1 {
    let key = recipient_public_key(seed);
    RecipientKeyIdentityV1::new(suite, digest_key(key.as_bytes())).unwrap()
}

fn digest_key(bytes: &[u8; 32]) -> Digest32 {
    Digest32::new(sha2::Sha256::digest(bytes).into())
}

pub(crate) fn content_key() -> ContentEncryptionKeyV1 {
    ContentEncryptionKeyV1::from_test_bytes([0x22; 32])
}

pub(crate) fn custody_nodes() -> Vec<(NodePublicKey, NodeCustodyPublicKeyV1)> {
    vec![
        (
            node_public_key(1),
            node_custody_secret(1).public_key().unwrap(),
        ),
        (
            node_public_key(2),
            node_custody_secret(2).public_key().unwrap(),
        ),
        (
            node_public_key(3),
            node_custody_secret(3).public_key().unwrap(),
        ),
    ]
}

pub(crate) fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        custody_nodes()
            .into_iter()
            .enumerate()
            .map(|(index, (node_public_key, custody_public_key))| {
                elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
                    node_public_key,
                    custody_public_key,
                    elastos_protected_content_contracts::ShareCoordinateV1::new(
                        u8::try_from(index + 1).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap();
    SignedCustodyEpochV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub(crate) fn custody_epoch_identity() -> CustodyEpochIdentityV1 {
    signed_custody_epoch().epoch_identity().unwrap()
}

pub(crate) fn custody_pool_identity() -> CustodyPoolIdentityV1 {
    CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap()
}

pub(crate) fn custody_committee_authorization_identity() -> CustodyCommitteeAuthorizationIdentityV1
{
    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap()
}

fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

fn binding_for_wallet_with_envelope(
    wallet: WalletAddress,
    envelope: &CustodyEnvelopeV1,
) -> ProtectedContentBindingV1 {
    let content = EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap();
    let policy_body = policy_body();
    ProtectedContentBindingV1::new(
        content.clone(),
        envelope.key_envelope_identity().unwrap(),
        policy_body.policy_identity().unwrap(),
        elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[0x26; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        wallet,
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap()
}

fn policy_body() -> RightsPolicyBodyV1 {
    RightsPolicyBodyV1::new(
        "content:alpha",
        RightsActionV1::View,
        "view",
        RightsSubjectSourceV1::WalletAddress,
        11155111,
        EvmContractAddressV1::new([0x11; 20]).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
        RightsObservationFinalityV1::new(12),
    )
    .unwrap()
}

pub(crate) fn verified_release_request() -> VerifiedKeyReleaseRequestV1 {
    verified_release_request_for_envelope_with_suite_and_recipient_seed(
        &provisioned_envelope(),
        CUSTODY_HPKE_SUITE_ID_V1,
        0x30,
    )
}

pub(crate) fn verified_release_request_for_envelope(
    envelope: &CustodyEnvelopeV1,
) -> VerifiedKeyReleaseRequestV1 {
    verified_release_request_for_envelope_with_suite_and_recipient_seed(
        envelope,
        CUSTODY_HPKE_SUITE_ID_V1,
        0x30,
    )
}

pub(crate) fn verified_release_request_for_envelope_and_recipient_seed(
    envelope: &CustodyEnvelopeV1,
    recipient_seed: u8,
) -> VerifiedKeyReleaseRequestV1 {
    verified_release_request_for_envelope_with_suite_and_recipient_seed(
        envelope,
        CUSTODY_HPKE_SUITE_ID_V1,
        recipient_seed,
    )
}

fn verified_release_request_for_envelope_with_suite_and_recipient_seed(
    envelope: &CustodyEnvelopeV1,
    suite: &str,
    recipient_seed: u8,
) -> VerifiedKeyReleaseRequestV1 {
    let rights = {
        let signed = signed_rights_request_with_suite_for_envelope(suite, envelope, recipient_seed);
        let context = RightsVerificationContextV1::new(
            signed.request().binding().clone(),
            signed.request().action(),
            signed.request().recipient().clone(),
            NOW + 1,
        );
        signed
            .verify(&context, &mut TestReplayClaims::default())
            .unwrap()
    };
    KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        NOW + 1,
        NOW + 50,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap()
    .verify(&rights, NOW + 3, &mut TestReplayClaims::default())
    .unwrap()
}

fn signed_rights_request_with_suite_for_envelope(
    suite: &str,
    envelope: &CustodyEnvelopeV1,
    recipient_seed: u8,
) -> WalletSignedRightsRequestV1 {
    let wallet = wallet(7);
    let request = RightsRequestV1::new(
        binding_for_wallet_with_envelope(wallet, envelope),
        RightsActionV1::View,
        recipient_identity_with_suite(recipient_seed, suite),
        NOW,
        NOW + 180,
        ReplayNonce16::new([0x55; 16]),
    )
    .unwrap();
    let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
    let (signature, recovery_id) = key
        .sign_prehash_recoverable(&ethereum_signed_message_hash(
            &request.canonical_bytes().unwrap(),
        ))
        .unwrap();
    let mut signature_bytes = signature.to_bytes().to_vec();
    signature_bytes.push(recovery_id.to_byte());
    WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
}

pub(crate) fn authenticated_runtime_release_operation_for_envelope_and_recipient_seed(
    envelope: &CustodyEnvelopeV1,
    recipient_seed: u8,
) -> AuthenticatedRuntimeReleaseOperationV1 {
    let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
    let recipient_public_key = recipient_public_key(recipient_seed);
    let recipient_public_key_bytes =
        RecipientPublicKeyBytesV1::new(*recipient_public_key.as_bytes()).unwrap();
    let rights_request = signed_rights_request_with_suite_for_envelope(
        CUSTODY_HPKE_SUITE_ID_V1,
        envelope,
        recipient_seed,
    );
    let release_request = KeyReleaseRequestV1::new(
        rights_request.request().binding().clone(),
        rights_request.request().request_hash().unwrap(),
        RightsActionV1::View,
        rights_request.request().recipient().clone(),
        NOW + 1,
        NOW + 50,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let profile = SigningKey::from_bytes(&[0x26; 32]);
    let authorization_statement =
        elastos_protected_content_contracts::RecipientKeyAuthorizationStatementV1::new(
            rights_request.request().binding().clone(),
            RightsActionV1::View,
            recipient_public_key_bytes,
            rights_request.request().recipient().clone(),
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            NOW,
            NOW + 90,
        )
        .unwrap();
    let authorization = SignedRecipientKeyAuthorizationV1::new(
        authorization_statement.clone(),
        profile
            .sign(&authorization_statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let policy_body = policy_body();
    let binding = rights_request.request().binding().clone();
    let statement = RuntimeReleaseOperationStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        rights_request,
        release_request,
        recipient_public_key_bytes,
        authorization,
        policy_body.clone(),
        RightsEvaluationEvidenceRequestV1::new(binding, policy_body.policy_identity().unwrap())
            .unwrap(),
        signed_custody_epoch(),
        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
        NOW + 2,
        NOW + 40,
    )
    .unwrap();
    SignedRuntimeReleaseOperationV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
    .verify(NOW + 3)
    .unwrap()
}

pub(crate) fn claimed_runtime_release_operation_for_envelope_and_node_seed(
    envelope: &CustodyEnvelopeV1,
    node_seed: u8,
    recipient_seed: u8,
) -> ClaimedNodeReleaseOperationV1 {
    let authenticated = authenticated_runtime_release_operation_for_envelope_and_recipient_seed(
        envelope,
        recipient_seed,
    );
    let temp = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut store =
        DurableReplayClaimStoreV1::new(node_public_key(node_seed), temp.path().join("replay"));
    store
        .claim_node_release_operation(authenticated, envelope, node_public_key(node_seed), NOW + 3)
        .unwrap()
}

pub(crate) fn signed_node_decision(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeRightsDecisionV1 {
    let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
        request.request_hash(),
        request.rights_request_hash(),
        request.binding().clone(),
        request.action(),
        node_public_key(node_seed),
        decision,
        digest(0x80 + node_seed),
        NOW + 4,
        NOW + 50,
    )
    .unwrap();
    let signature = node_signing_key(node_seed)
        .sign(&statement.canonical_bytes().unwrap())
        .to_bytes()
        .to_vec();
    SignedNodeRightsDecisionV1::new(statement, signature).unwrap()
}

pub(crate) fn provisioned_envelope() -> CustodyEnvelopeV1 {
    provision_custody_envelope_with_rng(
        EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
        &content_key(),
        custody_pool_identity(),
        custody_epoch_identity(),
        custody_committee_authorization_identity(),
        ThresholdV1::new(2, 3).unwrap(),
        custody_nodes(),
        &mut HpkeStdRng::from_seed([0x41; 32]),
        &mut ShamirStdRng::from_seed([0x42; 32]),
    )
    .unwrap()
}
