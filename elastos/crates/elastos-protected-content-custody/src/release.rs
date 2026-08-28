use ed25519_dalek::{Signer as _, SigningKey};
use hpke::rand_core::{CryptoRng as HpkeCryptoRng, RngCore as HpkeRngCore};
#[cfg(test)]
use rand09::{rngs::StdRng, SeedableRng as _};

use elastos_protected_content_contracts::{
    CanonicalContract, NodeContributionStatementV1, NodePublicKey, RecipientSealedContributionV1,
    RightsDecisionV1, SignedNodeContributionV1, SignedNodeRightsDecisionV1,
};

use crate::{
    node_share::NodeLocalStoredShareV1,
    replay_store::ClaimedNodeReleaseOperationV1,
    secrets::{NodeCustodySecretKeyV1, RecipientPublicKeyV1},
    share_wrap::seal_share,
    CustodyError,
};

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn produce_node_contribution(
    operation: ClaimedNodeReleaseOperationV1,
    signed_rights_decision: &SignedNodeRightsDecisionV1,
    node_share: &NodeLocalStoredShareV1,
    node_signing_key: &SigningKey,
    node_custody_secret: &NodeCustodySecretKeyV1,
    recipient_public_key: &RecipientPublicKeyV1,
    issued_at: u64,
    expires_at: u64,
    now: u64,
) -> Result<SignedNodeContributionV1, CustodyError> {
    let mut hpke_rng =
        StdRng::try_from_os_rng().map_err(|_| CustodyError::RandomnessUnavailable)?;
    produce_node_contribution_with_rng(
        &operation,
        signed_rights_decision,
        node_share,
        node_signing_key,
        node_custody_secret,
        recipient_public_key,
        issued_at,
        expires_at,
        now,
        &mut hpke_rng,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn produce_node_contribution_with_rng<R: HpkeCryptoRng + HpkeRngCore>(
    operation: &ClaimedNodeReleaseOperationV1,
    signed_rights_decision: &SignedNodeRightsDecisionV1,
    node_share: &NodeLocalStoredShareV1,
    node_signing_key: &SigningKey,
    node_custody_secret: &NodeCustodySecretKeyV1,
    recipient_public_key: &RecipientPublicKeyV1,
    issued_at: u64,
    expires_at: u64,
    now: u64,
    hpke_rng: &mut R,
) -> Result<SignedNodeContributionV1, CustodyError> {
    operation.validate_node_share(node_share, now)?;
    if operation.recipient().encryption_suite_id()
        != elastos_protected_content_contracts::CUSTODY_X_WING_AES256GCM_SUITE_ID_V1
    {
        return Err(CustodyError::BindingMismatch(
            "recipient_encryption_suite_id",
        ));
    }
    if recipient_public_key.identity()? != *operation.recipient() {
        return Err(CustodyError::BindingMismatch("recipient_key_identity"));
    }

    let node_set = node_share.node_set()?;
    let decision = operation.verify_node_rights_decision(signed_rights_decision, &node_set, now)?;
    if decision.decision() != RightsDecisionV1::Allowed {
        return Err(CustodyError::Release(
            elastos_protected_content_contracts::KeyReleaseError::RightsDenied,
        ));
    }
    operation.validate_node_contribution_active_window(issued_at, expires_at, &decision, now)?;
    let node_public_key = NodePublicKey::new(node_signing_key.verifying_key().to_bytes())?;
    if node_public_key != decision.node_public_key() {
        return Err(CustodyError::BindingMismatch("node_public_key"));
    }
    if node_public_key != operation.selected_node_public_key() {
        return Err(CustodyError::BindingMismatch("claimed_node_public_key"));
    }

    let node_entry = node_share.node()?;
    node_custody_secret.matches_node_entry(
        node_public_key,
        node_entry.custody_public_key(),
        node_signing_key,
    )?;

    let stored_share = node_share.stored_share();
    let stored_aad = node_share.stored_share_aad_bytes()?;
    let plaintext_share = crate::share_wrap::open_share(
        stored_share,
        node_custody_secret.secret_bytes(),
        elastos_protected_content_contracts::STORED_SHARE_HPKE_INFO_V1,
        &stored_aad,
    )?;
    let released_aad = node_entry.released_share_aad_bytes(
        operation.release_request_hash(),
        operation.binding(),
        decision.decision_hash(),
        operation.recipient(),
    )?;
    let released_ciphertext = seal_share(
        recipient_public_key.as_bytes(),
        elastos_protected_content_contracts::RELEASED_SHARE_HPKE_INFO_V1,
        &released_aad,
        &plaintext_share,
        hpke_rng,
    )?;
    let recipient_sealed_contribution = RecipientSealedContributionV1::new(
        operation.recipient().clone(),
        released_ciphertext.canonical_bytes()?,
    )?;
    let statement = NodeContributionStatementV1::new(
        operation.release_request_hash(),
        operation.binding().clone(),
        signed_rights_decision.clone(),
        recipient_sealed_contribution,
        issued_at,
        expires_at,
    )?;
    let signature = node_signing_key
        .sign(&statement.canonical_bytes()?)
        .to_bytes()
        .to_vec();
    let signed = SignedNodeContributionV1::new(statement, signature)?;
    operation.verify_node_contribution(&signed, &node_set, now)?;
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use rand09::{rngs::StdRng, SeedableRng as _};

    use super::*;
    use crate::share_wrap::{open_share, seal_share};
    use crate::test_support::{
        claimed_runtime_release_operation_for_envelope_and_node_seed, node_custody_secret,
        node_signing_key, provisioned_envelope, recipient_public_key, signed_node_decision,
        verified_release_request, verified_release_request_for_envelope,
    };
    use elastos_protected_content_contracts::{
        ContractError, CustodyEnvelopeV1, KeyReleaseError, RightsError, RIGHTS_CLOCK_SKEW_SECS,
    };

    fn node_share_index(envelope: &CustodyEnvelopeV1, seed: u8) -> usize {
        let node_public_key = elastos_protected_content_contracts::NodePublicKey::new(
            node_signing_key(seed).verifying_key().to_bytes(),
        )
        .unwrap();
        envelope.manifest().node_index(node_public_key).unwrap()
    }

    fn claimed_operation(
        envelope: &CustodyEnvelopeV1,
        node_seed: u8,
    ) -> ClaimedNodeReleaseOperationV1 {
        claimed_runtime_release_operation_for_envelope_and_node_seed(envelope, node_seed, 0x30)
    }

    fn local_share(envelope: &CustodyEnvelopeV1, node_seed: u8) -> NodeLocalStoredShareV1 {
        NodeLocalStoredShareV1::extract_from_envelope(
            envelope,
            elastos_protected_content_contracts::NodePublicKey::new(
                node_signing_key(node_seed).verifying_key().to_bytes(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn release_produces_a_verified_node_contribution() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let signed = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap();
        let verified = signed
            .verify(
                &request,
                &envelope.manifest().node_set().unwrap(),
                crate::test_support::NOW + 6,
            )
            .unwrap();
        assert_eq!(
            verified.node_public_key(),
            elastos_protected_content_contracts::NodePublicKey::new(
                node_signing_key(1).verifying_key().to_bytes()
            )
            .unwrap()
        );
    }

    #[test]
    fn release_rejects_wrong_signing_key() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(2),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("node_public_key")
        ));
    }

    #[test]
    fn release_rejects_claimed_operation_for_a_different_node() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 2, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(2),
            &node_custody_secret(2),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("claimed_node_public_key")
        ));
    }

    #[test]
    fn release_rejects_wrong_custody_secret() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(2),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("node_custody_public_key")
        ));
    }

    #[test]
    fn release_rejects_wrong_recipient_public_key() {
        let envelope = provisioned_envelope();
        let request = verified_release_request();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x31),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::BindingMismatch("recipient_key_identity")
        ));
    }

    #[test]
    fn release_rejects_denied_decision() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Denied),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::Release(
                elastos_protected_content_contracts::KeyReleaseError::RightsDenied
            )
        ));
    }

    #[test]
    fn release_rejects_post_binding_envelope_mutation_before_decrypt() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let mut stored_shares = envelope.stored_shares().to_vec();
        let index = node_share_index(&envelope, 1);
        stored_shares[index] = stored_shares[index].tamper_envelope();
        let tampered = CustodyEnvelopeV1::new(envelope.manifest().clone(), stored_shares).unwrap();
        let err = produce_node_contribution_with_rng(
            &claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&tampered, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
            &mut StdRng::from_seed([0x71; 32]),
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::BindingMismatch("key_envelope")));
    }

    #[test]
    fn release_reaches_hpke_failure_for_request_bound_to_invalid_ciphertext() {
        let envelope = provisioned_envelope();
        let mut stored_shares = envelope.stored_shares().to_vec();
        let index = node_share_index(&envelope, 1);
        stored_shares[index] = stored_shares[index].tamper_envelope();
        let tampered = CustodyEnvelopeV1::new(envelope.manifest().clone(), stored_shares).unwrap();
        let request = verified_release_request_for_envelope(&tampered);
        let err = produce_node_contribution_with_rng(
            &claimed_operation(&tampered, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&tampered, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
            &mut StdRng::from_seed([0x71; 32]),
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::PqHybrid(_)));
    }

    #[test]
    fn release_reaches_hpke_failure_for_request_bound_to_wrong_node_aad_stored_share() {
        let envelope = provisioned_envelope();
        let node_public_key = elastos_protected_content_contracts::NodePublicKey::new(
            node_signing_key(1).verifying_key().to_bytes(),
        )
        .unwrap();
        let stored_share = envelope.stored_share_for_node(node_public_key).unwrap();
        let correct_aad = envelope
            .manifest()
            .stored_share_aad_bytes_for_node(node_public_key)
            .unwrap();
        let wrong_aad = envelope
            .manifest()
            .stored_share_aad_bytes_for_node(
                elastos_protected_content_contracts::NodePublicKey::new(
                    node_signing_key(2).verifying_key().to_bytes(),
                )
                .unwrap(),
            )
            .unwrap();
        let plaintext = open_share(
            stored_share,
            node_custody_secret(1).secret_bytes(),
            elastos_protected_content_contracts::STORED_SHARE_HPKE_INFO_V1,
            &correct_aad,
        )
        .unwrap();

        let mut stored_shares = envelope.stored_shares().to_vec();
        let index = node_share_index(&envelope, 1);
        stored_shares[index] = seal_share(
            envelope.manifest().nodes()[index]
                .custody_public_key()
                .as_bytes(),
            elastos_protected_content_contracts::STORED_SHARE_HPKE_INFO_V1,
            &wrong_aad,
            &plaintext,
            &mut StdRng::from_seed([0x74; 32]),
        )
        .unwrap();
        let tampered = CustodyEnvelopeV1::new(envelope.manifest().clone(), stored_shares).unwrap();
        let request = verified_release_request_for_envelope(&tampered);

        let err = produce_node_contribution_with_rng(
            &claimed_operation(&tampered, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&tampered, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
            &mut StdRng::from_seed([0x71; 32]),
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::PqHybrid(_)));
    }

    #[test]
    fn release_checks_active_window_before_opening_bound_invalid_ciphertext() {
        let envelope = provisioned_envelope();
        let mut stored_shares = envelope.stored_shares().to_vec();
        let index = node_share_index(&envelope, 1);
        stored_shares[index] = stored_shares[index].tamper_envelope();
        let tampered = CustodyEnvelopeV1::new(envelope.manifest().clone(), stored_shares).unwrap();
        let request = verified_release_request_for_envelope(&tampered);
        let err = produce_node_contribution_with_rng(
            &claimed_operation(&tampered, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&tampered, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 6,
            crate::test_support::NOW + 6,
            &mut StdRng::from_seed([0x71; 32]),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::Release(KeyReleaseError::Rights(RightsError::Expired))
        ));
    }

    #[test]
    fn release_contract_rejects_noncanonical_eph_before_pq_hybrid_open() {
        let envelope = provisioned_envelope();
        let index = node_share_index(&envelope, 1);
        let share = envelope.stored_shares()[index].clone();
        let mut tampered = share.envelope().to_vec();
        tampered[1088..1120].copy_from_slice(&[
            0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0x7f,
        ]);
        let err =
            elastos_protected_content_contracts::PqHybridSealedShareV1::new(tampered).unwrap_err();
        assert_eq!(err, ContractError::InvalidField("pq_hybrid_eph_x25519"));
    }

    #[test]
    fn release_accepts_the_contract_clock_skew_boundary() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 6 + RIGHTS_CLOCK_SKEW_SECS,
            crate::test_support::NOW + 40,
            crate::test_support::NOW + 6,
        )
        .unwrap();
    }

    #[test]
    fn release_rejects_expires_at_equal_to_now() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let err = produce_node_contribution(
            claimed_operation(&envelope, 1),
            &signed_node_decision(&request, 1, RightsDecisionV1::Allowed),
            &local_share(&envelope, 1),
            &node_signing_key(1),
            &node_custody_secret(1),
            &recipient_public_key(0x30),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 6,
            crate::test_support::NOW + 6,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::Release(KeyReleaseError::Rights(RightsError::Expired))
        ));
    }
}
