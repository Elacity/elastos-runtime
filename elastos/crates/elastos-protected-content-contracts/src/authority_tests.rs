use ed25519_dalek::{Signer as _, SigningKey};
use k256::ecdsa::SigningKey as WalletSigningKey;

use elastos_auth::ethereum_signed_message_hash;

use crate::test_support::{
    binding_for_wallet, custody_epoch_identity, digest, node_key, node_public_key, node_set,
    wallet, TestReplayClaims, NOW,
};
use crate::*;

fn signed_rights(seed: u8) -> WalletSignedRightsRequestV1 {
    let wallet = wallet(seed);
    let request = RightsRequestV1::new(
        binding_for_wallet(wallet),
        RightsActionV1::View,
        recipient(0xa0),
        NOW,
        NOW + 180,
        ReplayNonce16::new([0x55; 16]),
    )
    .unwrap();
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let (signature, recovery_id) = key
        .sign_prehash_recoverable(&ethereum_signed_message_hash(
            &request.canonical_bytes().unwrap(),
        ))
        .unwrap();
    let mut signature_bytes = signature.to_bytes().to_vec();
    signature_bytes.push(recovery_id.to_byte());
    WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
}

fn recipient(seed: u8) -> RecipientKeyIdentityV1 {
    RecipientKeyIdentityV1::new("x25519-hkdf-sha256-aes256gcm/v1", digest(seed)).unwrap()
}

fn verified_rights() -> VerifiedRightsRequestV1 {
    let signed = signed_rights(7);
    let context = RightsVerificationContextV1::new(
        signed.request().binding().clone(),
        signed.request().action(),
        signed.request().recipient().clone(),
        NOW + 1,
    );
    signed
        .verify(&context, &mut TestReplayClaims::default())
        .unwrap()
}

fn release_request(rights: &VerifiedRightsRequestV1) -> KeyReleaseRequestV1 {
    KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        NOW + 2,
        NOW + 60,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap()
}

fn verified_release() -> VerifiedKeyReleaseRequestV1 {
    let rights = verified_rights();
    release_request(&rights)
        .verify(&rights, NOW + 3, &mut TestReplayClaims::default())
        .unwrap()
}

fn signed_node_decision(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeRightsDecisionV1 {
    let statement = NodeRightsDecisionStatementV1::new(
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
    let signature = node_key(node_seed)
        .sign(&statement.canonical_bytes().unwrap())
        .to_bytes()
        .to_vec();
    SignedNodeRightsDecisionV1::new(statement, signature).unwrap()
}

fn signed_contribution_with_decision(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
    signing_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeContributionV1 {
    signed_contribution_with_sealed_bytes(
        request,
        node_seed,
        signing_seed,
        decision,
        vec![0xc0 + node_seed; 48],
    )
}

fn signed_contribution_with_sealed_bytes(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
    signing_seed: u8,
    decision: RightsDecisionV1,
    sealed_bytes: Vec<u8>,
) -> SignedNodeContributionV1 {
    let decision = signed_node_decision(request, node_seed, decision);
    let sealed =
        RecipientSealedContributionV1::new(request.recipient().clone(), sealed_bytes).unwrap();
    let statement = NodeContributionStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        decision,
        sealed,
        NOW + 5,
        NOW + 45,
    )
    .unwrap();
    let signature = node_key(signing_seed)
        .sign(&statement.canonical_bytes().unwrap())
        .to_bytes()
        .to_vec();
    SignedNodeContributionV1::new(statement, signature).unwrap()
}

fn verified_contribution(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
) -> VerifiedNodeContributionV1 {
    signed_contribution_with_decision(request, node_seed, node_seed, RightsDecisionV1::Allowed)
        .verify(request, &node_set(), NOW + 6)
        .unwrap()
}

fn verified_contribution_with_sealed_bytes(
    request: &VerifiedKeyReleaseRequestV1,
    node_seed: u8,
    sealed_bytes: Vec<u8>,
) -> VerifiedNodeContributionV1 {
    signed_contribution_with_sealed_bytes(
        request,
        node_seed,
        node_seed,
        RightsDecisionV1::Allowed,
        sealed_bytes,
    )
    .verify(request, &node_set(), NOW + 6)
    .unwrap()
}

fn terminal_receipt(
    request: &VerifiedKeyReleaseRequestV1,
    contributions: &[VerifiedNodeContributionV1],
    issuer_seed: u8,
) -> SignedTerminalReceiptV1 {
    let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
    let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
    let statement = TerminalReceiptStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        issuer,
        KeyReleaseOutcomeV1::Released,
        contributions
            .iter()
            .map(NodeContributionRefV1::from)
            .collect(),
        NOW + 7,
        NOW + 40,
    )
    .unwrap();
    let signature = issuer_key
        .sign(&statement.canonical_bytes().unwrap())
        .to_bytes()
        .to_vec();
    SignedTerminalReceiptV1::new(statement, signature).unwrap()
}

#[test]
fn release_authority_does_not_accept_a_preliminary_receipt() {
    let rights = verified_rights();
    let request = release_request(&rights);
    // The release API accepts only the verified Wallet request and mandatory
    // replay claimer. There is no preliminary-receipt/allow parameter to forge.
    request
        .verify(&rights, NOW + 3, &mut TestReplayClaims::default())
        .unwrap();
}

#[test]
fn release_replay_is_nonce_keyed_and_storage_failure_fails_closed() {
    let rights = verified_rights();
    let first = release_request(&rights);
    let changed = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        NOW + 3,
        NOW + 55,
        first.replay_nonce(),
    )
    .unwrap();
    let mut replay = TestReplayClaims::default();
    first.verify(&rights, NOW + 4, &mut replay).unwrap();
    assert_eq!(
        changed.verify(&rights, NOW + 4, &mut replay),
        Err(KeyReleaseError::Replay(ReplayClaimError::AlreadyClaimed))
    );

    struct Unavailable;
    impl AtomicReplayClaimer for Unavailable {
        fn claim(&mut self, _: ReplayClaimKeyV1, _: u64, _: u64) -> Result<(), ReplayClaimError> {
            Err(ReplayClaimError::Unavailable)
        }
    }
    assert_eq!(
        first.verify(&rights, NOW + 4, &mut Unavailable),
        Err(KeyReleaseError::Replay(ReplayClaimError::Unavailable))
    );
}

#[test]
fn release_stays_inside_wallet_authority_window() {
    let rights = verified_rights();
    let beyond = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        NOW + 2,
        rights.expires_at() + 1,
        ReplayNonce16::new([0x88; 16]),
    )
    .unwrap_err();
    assert_eq!(
        beyond,
        ContractError::InvalidField("key_release_request_lifetime")
    );

    let rights = verified_rights();
    let late_start = rights.expires_at() - 30;
    let beyond = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        late_start,
        rights.expires_at() + 1,
        ReplayNonce16::new([0x88; 16]),
    )
    .unwrap();
    assert_eq!(
        beyond.verify(&rights, late_start, &mut TestReplayClaims::default()),
        Err(KeyReleaseError::BindingMismatch("rights_request_window"))
    );
}

#[test]
fn wrong_content_policy_threshold_and_session_are_authority_mismatches() {
    let rights = verified_rights();
    let original = rights.binding();
    let variants = [
        ProtectedContentBindingV1::new(
            EncryptedContentIdentityV1::new(digest(0xee), 4096).unwrap(),
            KeyEnvelopeIdentityV1::new(
                EncryptedContentIdentityV1::new(digest(0xee), 4096).unwrap(),
                original.key_envelope().envelope_sha256(),
                original.key_envelope().envelope_bytes(),
                original.key_envelope().node_set_id(),
                original.key_envelope().threshold(),
                original.key_envelope().custody_epoch(),
            )
            .unwrap(),
            original.rights_policy().clone(),
            original.profile(),
            original.wallet(),
            original.runtime_session_binding(),
        )
        .unwrap(),
        ProtectedContentBindingV1::new(
            original.encrypted_content().clone(),
            original.key_envelope().clone(),
            RightsPolicyIdentityV1::new(digest(0xee), 384).unwrap(),
            original.profile(),
            original.wallet(),
            original.runtime_session_binding(),
        )
        .unwrap(),
        ProtectedContentBindingV1::new(
            original.encrypted_content().clone(),
            KeyEnvelopeIdentityV1::new(
                original.encrypted_content().clone(),
                original.key_envelope().envelope_sha256(),
                original.key_envelope().envelope_bytes(),
                digest(0xee),
                ThresholdV1::new(3, 3).unwrap(),
                custody_epoch_identity(),
            )
            .unwrap(),
            original.rights_policy().clone(),
            original.profile(),
            original.wallet(),
            original.runtime_session_binding(),
        )
        .unwrap(),
        ProtectedContentBindingV1::new(
            original.encrypted_content().clone(),
            original.key_envelope().clone(),
            original.rights_policy().clone(),
            original.profile(),
            original.wallet(),
            RuntimeSessionBindingV1::new(digest(0xee)).unwrap(),
        )
        .unwrap(),
    ];
    for variant in variants {
        let request = KeyReleaseRequestV1::new(
            variant,
            rights.request_hash(),
            rights.action(),
            rights.recipient().clone(),
            NOW + 2,
            NOW + 60,
            ReplayNonce16::new([0x91; 16]),
        )
        .unwrap();
        assert_eq!(
            request.verify(&rights, NOW + 3, &mut TestReplayClaims::default()),
            Err(KeyReleaseError::BindingMismatch(
                "protected_content_binding"
            ))
        );
    }
}

#[test]
fn node_set_and_threshold_are_checked_before_contribution() {
    let request = verified_release();
    let decision = signed_node_decision(&request, 1, RightsDecisionV1::Allowed);
    let wrong_set = NodeSetV1::new(
        ThresholdV1::new(2, 3).unwrap(),
        vec![node_public_key(1), node_public_key(2), node_public_key(4)],
    )
    .unwrap();
    assert_eq!(
        decision.verify(&request, &wrong_set, NOW + 6),
        Err(KeyReleaseError::BindingMismatch("node_set_id"))
    );

    let wrong_threshold = NodeSetV1::new(
        ThresholdV1::new(3, 3).unwrap(),
        vec![node_public_key(1), node_public_key(2), node_public_key(3)],
    )
    .unwrap();
    assert_eq!(
        decision.verify(&request, &wrong_threshold, NOW + 6),
        Err(KeyReleaseError::BindingMismatch("node_set_id"))
    );
}

#[test]
fn wrong_rights_and_release_request_hashes_fail_closed() {
    let rights = verified_rights();
    let wrong_rights_hash = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        digest(0xfe),
        rights.action(),
        rights.recipient().clone(),
        NOW + 2,
        NOW + 60,
        ReplayNonce16::new([0x93; 16]),
    )
    .unwrap();
    assert_eq!(
        wrong_rights_hash.verify(&rights, NOW + 3, &mut TestReplayClaims::default()),
        Err(KeyReleaseError::BindingMismatch("rights_request_hash"))
    );

    let request = verified_release();
    let statement = NodeRightsDecisionStatementV1::new(
        digest(0xff),
        request.rights_request_hash(),
        request.binding().clone(),
        request.action(),
        node_public_key(1),
        RightsDecisionV1::Allowed,
        digest(0x81),
        NOW + 4,
        NOW + 50,
    )
    .unwrap();
    let decision = SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_key(1)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        decision.verify(&request, &node_set(), NOW + 6),
        Err(KeyReleaseError::BindingMismatch("key_release_request_hash"))
    );
}

#[test]
fn contribution_requires_same_node_signed_allowed_rights_evidence() {
    let request = verified_release();
    let denied = signed_contribution_with_decision(&request, 1, 1, RightsDecisionV1::Denied);
    assert_eq!(
        denied.verify(&request, &node_set(), NOW + 6),
        Err(KeyReleaseError::RightsDenied)
    );

    let different_signer =
        signed_contribution_with_decision(&request, 1, 2, RightsDecisionV1::Allowed);
    assert_eq!(
        different_signer.verify(&request, &node_set(), NOW + 6),
        Err(KeyReleaseError::InvalidNodeContributionSignature)
    );

    verified_contribution(&request, 1);
}

#[test]
fn node_decision_and_contribution_cannot_escape_release_window() {
    let request = verified_release();
    let statement = NodeRightsDecisionStatementV1::new(
        request.request_hash(),
        request.rights_request_hash(),
        request.binding().clone(),
        request.action(),
        node_public_key(1),
        RightsDecisionV1::Allowed,
        digest(0x81),
        NOW + 4,
        request.expires_at() + 1,
    )
    .unwrap();
    let signed = SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_key(1)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        signed.verify(&request, &node_set(), NOW + 6),
        Err(KeyReleaseError::BindingMismatch("node_decision_window"))
    );
}

#[test]
fn terminal_receipt_is_authenticated_by_runtime_selected_issuer() {
    let request = verified_release();
    let contributions = vec![
        verified_contribution(&request, 1),
        verified_contribution(&request, 2),
    ];
    let receipt = terminal_receipt(&request, &contributions, 21);
    let expected =
        TerminalReceiptIssuerKey::new(SigningKey::from_bytes(&[21; 32]).verifying_key().to_bytes())
            .unwrap();
    receipt
        .verify(&request, &contributions, expected, NOW + 8)
        .unwrap();

    let wrong =
        TerminalReceiptIssuerKey::new(SigningKey::from_bytes(&[22; 32]).verifying_key().to_bytes())
            .unwrap();
    assert_eq!(
        receipt.verify(&request, &contributions, wrong, NOW + 8),
        Err(KeyReleaseError::UnexpectedTerminalIssuer)
    );
}

#[test]
fn terminal_receipt_cannot_escape_release_window() {
    let request = verified_release();
    let contributions = vec![
        verified_contribution(&request, 1),
        verified_contribution(&request, 2),
    ];
    let issuer_key = SigningKey::from_bytes(&[21; 32]);
    let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
    let statement = TerminalReceiptStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        issuer,
        KeyReleaseOutcomeV1::Released,
        contributions
            .iter()
            .map(NodeContributionRefV1::from)
            .collect(),
        request.expires_at() - 20,
        request.expires_at() + 1,
    )
    .unwrap();
    let receipt = SignedTerminalReceiptV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        receipt.verify(&request, &contributions, issuer, request.expires_at() - 10),
        Err(KeyReleaseError::BindingMismatch("terminal_receipt_window"))
    );
}

#[test]
fn terminal_receipt_cannot_extend_a_node_contribution_window() {
    let request = verified_release();
    let contributions = vec![
        verified_contribution(&request, 1),
        verified_contribution(&request, 2),
    ];
    let issuer_key = SigningKey::from_bytes(&[21; 32]);
    let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
    let statement = TerminalReceiptStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        issuer,
        KeyReleaseOutcomeV1::Released,
        contributions
            .iter()
            .map(NodeContributionRefV1::from)
            .collect(),
        NOW + 7,
        NOW + 46,
    )
    .unwrap();
    let receipt = SignedTerminalReceiptV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();

    assert_eq!(
        receipt.verify(&request, &contributions, issuer, NOW + 8),
        Err(KeyReleaseError::BindingMismatch("node_contribution_window"))
    );
}

#[test]
fn terminal_receipt_carries_only_authenticated_hash_references() {
    let request = verified_release();
    let contributions = vec![
        verified_contribution(&request, 1),
        verified_contribution(&request, 2),
    ];
    let receipt = terminal_receipt(&request, &contributions, 21);
    assert_eq!(receipt.statement().contribution_refs().len(), 2);
    assert!(receipt
        .statement()
        .contribution_refs()
        .iter()
        .all(|reference| reference.contribution_hash() != Digest32::new([0; 32])));
}

#[test]
fn terminal_receipt_preserves_exact_reference_verification_when_extra_contributions_are_passed() {
    let request = verified_release();
    let contributions = vec![
        verified_contribution(&request, 1),
        verified_contribution(&request, 2),
    ];
    let extra = verified_contribution(&request, 3);
    let receipt = terminal_receipt(&request, &contributions, 21);
    let issuer = receipt.statement().issuer();

    assert_eq!(
        receipt.verify(
            &request,
            &[contributions[0].clone(), contributions[1].clone(), extra],
            issuer,
            NOW + 8,
        ),
        Err(KeyReleaseError::BindingMismatch("node_contribution_refs"))
    );
}

#[test]
fn duplicate_contribution_commitment_cannot_count_toward_threshold() {
    let request = verified_release();
    let shared_bytes = vec![0xab; 48];
    let contributions = [
        verified_contribution_with_sealed_bytes(&request, 1, shared_bytes.clone()),
        verified_contribution_with_sealed_bytes(&request, 2, shared_bytes),
    ];
    assert_ne!(
        contributions[0].decision_hash(),
        contributions[1].decision_hash()
    );
    assert_ne!(
        contributions[0].contribution_hash(),
        contributions[1].contribution_hash()
    );
    assert_eq!(
        contributions[0].contribution_commitment(),
        contributions[1].contribution_commitment()
    );

    let issuer_key = SigningKey::from_bytes(&[21; 32]);
    let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
    let err = TerminalReceiptStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        issuer,
        KeyReleaseOutcomeV1::Released,
        contributions
            .iter()
            .map(NodeContributionRefV1::from)
            .collect(),
        NOW + 7,
        NOW + 40,
    )
    .unwrap_err();

    assert_eq!(err, ContractError::InvalidField("node_contribution_refs"));
}

#[test]
fn release_and_contribution_reject_a_different_recipient() {
    let rights = verified_rights();
    let wrong_release = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        recipient(0xa1),
        NOW + 2,
        NOW + 60,
        ReplayNonce16::new([0x67; 16]),
    )
    .unwrap();
    assert_eq!(
        wrong_release.verify(&rights, NOW + 3, &mut TestReplayClaims::default()),
        Err(KeyReleaseError::BindingMismatch("recipient_key_identity"))
    );

    let request = verified_release();
    let decision = signed_node_decision(&request, 1, RightsDecisionV1::Allowed);
    let sealed = RecipientSealedContributionV1::new(recipient(0xa1), vec![0xc1; 48]).unwrap();
    let statement = NodeContributionStatementV1::new(
        request.request_hash(),
        request.binding().clone(),
        decision,
        sealed,
        NOW + 5,
        NOW + 45,
    )
    .unwrap();
    let signed = SignedNodeContributionV1::new(
        statement.clone(),
        node_key(1)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        signed.verify(&request, &node_set(), NOW + 6),
        Err(KeyReleaseError::BindingMismatch("recipient_key_identity"))
    );
}

#[test]
fn terminal_receipt_rejects_contributions_from_another_release_request() {
    let rights = verified_rights();
    let request_a = release_request(&rights)
        .verify(&rights, NOW + 3, &mut TestReplayClaims::default())
        .unwrap();
    let request_b = KeyReleaseRequestV1::new(
        rights.binding().clone(),
        rights.request_hash(),
        rights.action(),
        rights.recipient().clone(),
        NOW + 2,
        NOW + 60,
        ReplayNonce16::new([0x67; 16]),
    )
    .unwrap()
    .verify(&rights, NOW + 3, &mut TestReplayClaims::default())
    .unwrap();
    assert_ne!(request_a.request_hash(), request_b.request_hash());

    let contributions = vec![
        verified_contribution(&request_a, 1),
        verified_contribution(&request_a, 2),
    ];
    let receipt = terminal_receipt(&request_b, &contributions, 21);
    let issuer =
        TerminalReceiptIssuerKey::new(SigningKey::from_bytes(&[21; 32]).verifying_key().to_bytes())
            .unwrap();
    assert_eq!(
        receipt.verify(&request_b, &contributions, issuer, NOW + 8),
        Err(KeyReleaseError::BindingMismatch("key_release_request_hash"))
    );
}

#[test]
fn canonical_signature_golden_vectors() {
    let rights = signed_rights(7);
    let verified_rights = verified_rights();
    let release = release_request(&verified_rights);
    let verified_release = verified_release();
    let contribution =
        signed_contribution_with_decision(&verified_release, 1, 1, RightsDecisionV1::Allowed);
    let decision = contribution.statement().signed_rights_decision();
    let verified_contributions = vec![
        verified_contribution(&verified_release, 1),
        verified_contribution(&verified_release, 2),
    ];
    let terminal = terminal_receipt(&verified_release, &verified_contributions, 21);

    assert_eq!(
        hex::encode(
            rights
                .request()
                .binding()
                .profile()
                .canonical_bytes()
                .unwrap()
        ),
        "656c6173746f732e70726f7465637465642d636f6e74656e742e70726f66696c652d6964656e746974792f7631009109db55f79797a396462fb895c2adcea7e8683c2f3056c07a5475155537b73e"
    );
    assert_eq!(
        hex::encode(
            rights
                .request()
                .binding()
                .rights_policy()
                .canonical_bytes()
                .unwrap()
        ),
        "656c6173746f732e70726f7465637465642d636f6e74656e742e7269676874732d706f6c6963792d6964656e746974792f763100444444444444444444444444444444444444444444444444444444444444444400000180"
    );
    assert_eq!(
        hex::encode(node_set().canonical_bytes().unwrap()),
        "656c6173746f732e70726f7465637465642d636f6e74656e742e6e6f64652d7365742f7631000203038139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3948a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ced4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"
    );
    assert_eq!(
        hex::encode(rights.wallet_signature()),
        "cda3a5d61aca1bb33e1d4df4bcca941800dab7f7739018a0c190a6d1f1faaa2e53f098e4bd6b1bc53ebc4205162cd7a283763602c48d287582adb944a86e50bb01"
    );
    assert_eq!(
        hex::encode(decision.node_signature()),
        "40a27f3b03e1f72754b9a26cc8fc62d980922982d82edfb127c6f45123b7d9d1aeb8b68ce2dac78ff12328eef24b2dd807039c176ad1b442558767b353518105"
    );
    assert_eq!(
        hex::encode(contribution.node_signature()),
        "23c25cfe287f73dbebb1fd7ee44c790f9966419041ed38fe9a4c3858cdc0a304f85c4e3054ae586e6bfa7c0d77105c4507209e578df802231716460ad7254109"
    );
    assert_eq!(
        hex::encode(terminal.issuer_signature()),
        "a3ac53c9c1f7f949a959371b31351b089ba6f84ecbd827c6550a306a5d4226991246fe52a060ff866a82a565623d523287833bbf1c3ae9dd49184440986a4306"
    );

    assert_eq!(
        [
            hex::encode(rights.canonical_hash().unwrap().as_bytes()),
            hex::encode(release.canonical_hash().unwrap().as_bytes()),
            hex::encode(contribution.canonical_hash().unwrap().as_bytes()),
            hex::encode(terminal.canonical_hash().unwrap().as_bytes()),
        ],
        [
            "61660286259d645be3551a50320dab0317901b61623e72b84ecb22830df2ef9d",
            "6d55a220fb555f50f96500f0a6cf28ec933b15fefc9ac8925220a3520e3958c8",
            "1a2900de201ab561469ea632ac6e06938b2ee4a2e7c007922a1543fca9c321f0",
            "6be094d54f410df6423d8e2872ba8f54f1d1ad6d68d05277e5acc52a421766c8",
        ]
        .map(str::to_string)
    );
}
