use ed25519_dalek::{Signer as _, SigningKey};
use k256::ecdsa::SigningKey as WalletSigningKey;
use sha3::{Digest as _, Keccak256};

use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyEnvelopeManifestV1, CustodyEnvelopeV1,
    CustodyEpochIssuerKeyV1, CustodyEpochStatementV1, Digest32, EncryptedContentIdentityV1,
    EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1, HpkeCiphertextV1,
    KeyReleaseOutcomeV1, KeyReleaseRequestV1, NodeContributionRefV1, NodeContributionStatementV1,
    NodeCustodyPublicKeyV1, NodePublicKey, RecipientKeyAuthorizationStatementV1,
    RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, RecipientSealedContributionV1,
    ReplayNonce16, RightsActionV1, RightsDecisionV1, RightsEvaluationEvidenceRequestV1,
    RightsObservationFinalityV1, RightsPolicyBodyV1, RightsRequestV1, RightsSubjectSourceV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    RuntimeSessionBindingV1, ShareCoordinateV1, SignedCustodyEpochV1, SignedNodeContributionV1,
    SignedNodeRightsDecisionV1, SignedRecipientKeyAuthorizationV1, SignedRuntimeReleaseOperationV1,
    SignedTerminalReceiptV1, TerminalReceiptIssuerKey, TerminalReceiptStatementV1, ThresholdV1,
    WalletAddress, WalletSignedRightsRequestV1, CUSTODY_HPKE_SUITE_ID_V1, HPKE_ENCAPPED_KEY_BYTES,
    HPKE_SEALED_SHARE_BYTES,
};

pub(crate) const NOW: u64 = 2_000_000_000;

pub(crate) fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

pub(crate) fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub(crate) fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

pub(crate) fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
    let mut bytes = [0u8; 32];
    bytes[0] = seed.max(9);
    RecipientPublicKeyBytesV1::new(bytes).unwrap()
}

pub(crate) fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
    recipient_public_key(seed)
        .key_identity(CUSTODY_HPKE_SUITE_ID_V1)
        .unwrap()
}

pub(crate) fn policy_body() -> RightsPolicyBodyV1 {
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

pub(crate) fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let nodes = vec![
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(1),
            NodeCustodyPublicKeyV1::new([0x31; 32]).unwrap(),
            ShareCoordinateV1::new(1).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(2),
            NodeCustodyPublicKeyV1::new([0x32; 32]).unwrap(),
            ShareCoordinateV1::new(2).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(3),
            NodeCustodyPublicKeyV1::new([0x33; 32]).unwrap(),
            ShareCoordinateV1::new(3).unwrap(),
        )
        .unwrap(),
    ];
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        nodes,
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

pub(crate) fn custody_envelope() -> CustodyEnvelopeV1 {
    custody_envelope_for_seed(0x11)
}

pub(crate) fn custody_envelope_for_seed(seed: u8) -> CustodyEnvelopeV1 {
    let epoch = signed_custody_epoch();
    let manifest = CustodyEnvelopeManifestV1::new(
        EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap(),
        epoch.epoch_identity().unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        digest(seed ^ 0x33),
        epoch.statement().nodes().to_vec(),
    )
    .unwrap();
    let shares = [seed ^ 0x50, seed ^ 0x51, seed ^ 0x52]
        .into_iter()
        .map(|seed| {
            let mut encapped_key = [0u8; HPKE_ENCAPPED_KEY_BYTES];
            encapped_key[0] = seed.max(9);
            let mut ciphertext = [0u8; HPKE_SEALED_SHARE_BYTES];
            ciphertext.fill(seed);
            HpkeCiphertextV1::new(encapped_key, ciphertext).unwrap()
        })
        .collect();
    CustodyEnvelopeV1::new(manifest, shares).unwrap()
}

pub(crate) fn binding_for_envelope(
    envelope: &CustodyEnvelopeV1,
) -> elastos_protected_content_contracts::ProtectedContentBindingV1 {
    let policy = policy_body();
    elastos_protected_content_contracts::ProtectedContentBindingV1::new(
        envelope.manifest().encrypted_content().clone(),
        envelope.key_envelope_identity().unwrap(),
        policy.policy_identity().unwrap(),
        elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[0x26; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap()
}

pub(crate) fn make_signed_runtime_release_operation() -> SignedRuntimeReleaseOperationV1 {
    make_signed_runtime_release_operation_for_seed(0x42)
}

pub(crate) fn make_signed_runtime_release_operation_for_seed(
    seed: u8,
) -> SignedRuntimeReleaseOperationV1 {
    make_signed_runtime_release_operation_for_envelope_and_seed(seed, &custody_envelope())
}

pub(crate) fn make_signed_runtime_release_operation_for_envelope_and_seed(
    seed: u8,
    envelope: &CustodyEnvelopeV1,
) -> SignedRuntimeReleaseOperationV1 {
    let runtime_key = SigningKey::from_bytes(&[seed; 32]);
    let binding = binding_for_envelope(envelope);
    let rights_request = {
        let request = RightsRequestV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_identity(0x30),
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
    };
    let release_request = KeyReleaseRequestV1::new(
        binding.clone(),
        rights_request.request().request_hash().unwrap(),
        RightsActionV1::View,
        rights_request.request().recipient().clone(),
        NOW + 1,
        NOW + 50,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let profile = SigningKey::from_bytes(&[0x26; 32]);
    let recipient_public_key = recipient_public_key(0x30);
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_public_key,
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
    let policy = policy_body();
    let evidence_request =
        RightsEvaluationEvidenceRequestV1::new(binding.clone(), policy.policy_identity().unwrap())
            .unwrap();
    let statement = RuntimeReleaseOperationStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        rights_request,
        release_request,
        recipient_public_key,
        authorization,
        policy,
        evidence_request,
        signed_custody_epoch(),
        RuntimeReleaseAuditIdV1::new(digest(0x91 ^ seed)).unwrap(),
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
}

pub(crate) fn make_signed_node_rights_decision(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeRightsDecisionV1 {
    let authenticated = operation.verify(NOW + 3).unwrap();
    let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.rights_request_hash(),
        authenticated.binding().clone(),
        authenticated.action(),
        node_public_key(node_seed),
        decision,
        digest(0x80 ^ node_seed),
        NOW + 4,
        NOW + 50,
    )
    .unwrap();
    SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub(crate) fn make_signed_node_contribution(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
) -> SignedNodeContributionV1 {
    let authenticated = operation.verify(NOW + 5).unwrap();
    let decision =
        make_signed_node_rights_decision(operation, node_seed, RightsDecisionV1::Allowed);
    let sealed =
        RecipientSealedContributionV1::new(authenticated.recipient().clone(), vec![node_seed; 96])
            .unwrap();
    let statement = NodeContributionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        decision,
        sealed,
        NOW + 5,
        NOW + 40,
    )
    .unwrap();
    SignedNodeContributionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub(crate) fn make_signed_terminal_receipt(
    operation: &SignedRuntimeReleaseOperationV1,
    contributions: &[SignedNodeContributionV1],
    issuer_seed: u8,
) -> SignedTerminalReceiptV1 {
    let authenticated = operation.verify(NOW + 6).unwrap();
    let node_set = signed_custody_epoch().statement().node_set().unwrap();
    let verified_contributions = contributions
        .iter()
        .map(|contribution| {
            authenticated
                .verify_node_contribution(contribution, &node_set, NOW + 6)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let refs = verified_contributions
        .iter()
        .map(NodeContributionRefV1::from)
        .collect::<Vec<_>>();
    let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
    let statement = TerminalReceiptStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        KeyReleaseOutcomeV1::Released,
        refs,
        NOW + 6,
        NOW + 40,
    )
    .unwrap();
    SignedTerminalReceiptV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}
