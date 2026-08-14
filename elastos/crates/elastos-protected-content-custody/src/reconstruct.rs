use std::collections::HashSet;

use vsss_rs::Gf256;
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    CustodyEnvelopeV1, CustodyNodeIdentityV1, KeyReleaseError, KeyReleaseOutcomeV1,
    SignedNodeContributionV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    VerifiedKeyReleaseRequestV1,
};

use crate::{
    hpke_helpers::{decode_ciphertext_bytes, open_share},
    secrets::{ContentEncryptionKeyV1, RecipientSecretKeyV1},
    CustodyError, CONTENT_KEY_BYTES,
};

pub fn reconstruct_content_key(
    request: &VerifiedKeyReleaseRequestV1,
    envelope: &CustodyEnvelopeV1,
    contributions: &[SignedNodeContributionV1],
    terminal_receipt: &SignedTerminalReceiptV1,
    expected_terminal_issuer: TerminalReceiptIssuerKey,
    recipient_secret: &RecipientSecretKeyV1,
    now: u64,
) -> Result<ContentEncryptionKeyV1, CustodyError> {
    if !envelope.matches_key_envelope_identity(request.binding().key_envelope())? {
        return Err(CustodyError::BindingMismatch("key_envelope"));
    }
    if recipient_secret.identity()? != *request.recipient() {
        return Err(CustodyError::BindingMismatch("recipient_key_identity"));
    }

    let required = usize::from(request.binding().key_envelope().threshold().required());
    if contributions.len() < required {
        return Err(CustodyError::Release(
            KeyReleaseError::InsufficientContributions,
        ));
    }
    if contributions.len() != required {
        return Err(CustodyError::BindingMismatch("release_threshold"));
    }

    let node_set = envelope.manifest().node_set()?;
    let mut seen_nodes = HashSet::with_capacity(contributions.len());
    let mut verified_contributions = Vec::with_capacity(contributions.len());
    let mut ordered_contributions = Vec::with_capacity(contributions.len());

    for contribution in contributions {
        let verified = contribution.verify(request, &node_set, now)?;
        if !seen_nodes.insert(verified.node_public_key()) {
            return Err(CustodyError::BindingMismatch("duplicate_contribution_node"));
        }
        let node_entry = envelope
            .manifest()
            .node(verified.node_public_key())
            .ok_or(CustodyError::BindingMismatch("custody_node"))?
            .clone();
        verified_contributions.push(verified.clone());
        ordered_contributions.push((
            node_entry.share_coordinate().get(),
            node_entry,
            verified,
            contribution,
        ));
    }

    terminal_receipt.verify(
        request,
        &verified_contributions,
        expected_terminal_issuer,
        now,
    )?;
    if terminal_receipt.statement().outcome() != KeyReleaseOutcomeV1::Released {
        return Err(CustodyError::BindingMismatch("terminal_receipt_outcome"));
    }

    ordered_contributions.sort_unstable_by_key(|(coordinate, _, _, _)| *coordinate);

    let mut plaintext_shares =
        Zeroizing::new(Vec::<Vec<u8>>::with_capacity(ordered_contributions.len()));
    for (_, node_entry, verified, contribution) in ordered_contributions {
        let plaintext_share = open_released_share(
            request,
            &node_entry,
            &verified,
            contribution,
            recipient_secret,
        )?;
        let share_index = plaintext_shares.len();
        plaintext_shares.push(vec![0u8; CONTENT_KEY_BYTES + 1]);
        let share = &mut plaintext_shares[share_index];
        share[0] = node_entry.share_coordinate().get();
        share[1..].copy_from_slice(&plaintext_share[..]);
    }

    let reconstructed = Zeroizing::new(Gf256::combine_bytes(&*plaintext_shares)?);
    if reconstructed.len() != CONTENT_KEY_BYTES {
        return Err(CustodyError::MalformedShare("reconstructed_content_key"));
    }

    let mut content_key = Zeroizing::new([0u8; CONTENT_KEY_BYTES]);
    content_key.copy_from_slice(&reconstructed[..]);
    let content_key = ContentEncryptionKeyV1::from_guarded_bytes(content_key);
    if !content_key.matches_commitment(envelope.manifest().content_key_commitment()) {
        return Err(CustodyError::ContentKeyCommitmentMismatch);
    }
    Ok(content_key)
}

fn open_released_share(
    request: &VerifiedKeyReleaseRequestV1,
    node_entry: &CustodyNodeIdentityV1,
    verified: &elastos_protected_content_contracts::VerifiedNodeContributionV1,
    contribution: &SignedNodeContributionV1,
    recipient_secret: &RecipientSecretKeyV1,
) -> Result<Zeroizing<[u8; CONTENT_KEY_BYTES]>, CustodyError> {
    let released_aad = node_entry.released_share_aad_bytes(
        request.request_hash(),
        request.binding(),
        verified.decision_hash(),
        request.recipient(),
    )?;
    let ciphertext = decode_ciphertext_bytes(
        contribution
            .statement()
            .recipient_sealed_contribution()
            .sealed_bytes(),
    )?;
    let plaintext_share = open_share(
        &ciphertext,
        recipient_secret.secret_bytes(),
        elastos_protected_content_contracts::RELEASED_SHARE_HPKE_INFO_V1,
        &released_aad,
    )?;
    Ok(plaintext_share)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};
    use rand09::{rngs::StdRng, SeedableRng as _};
    use rand10::SeedableRng as _;

    use elastos_protected_content_contracts::{
        CanonicalContract, EncryptedContentIdentityV1, HpkeCiphertextV1, KeyReleaseError,
        KeyReleaseOutcomeV1, NodeContributionRefV1, NodeContributionStatementV1,
        RecipientSealedContributionV1, SignedNodeContributionV1, SignedTerminalReceiptV1,
        TerminalReceiptIssuerKey, TerminalReceiptStatementV1, ThresholdV1,
        VerifiedKeyReleaseRequestV1,
    };

    use crate::{
        hpke_helpers::{open_share, seal_share},
        provision::provision_custody_envelope_with_rng,
        release::produce_node_contribution_with_rng,
        test_support::{
            content_key, custody_nodes, digest, node_custody_secret, node_signing_key,
            provisioned_envelope, recipient_public_key, recipient_secret, signed_node_decision,
            verified_release_request, verified_release_request_for_envelope,
            verified_release_request_for_envelope_and_recipient_seed, NOW,
        },
    };

    use super::*;

    fn released_contribution(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        node_seed: u8,
        recipient_seed: u8,
        hpke_seed: u8,
    ) -> SignedNodeContributionV1 {
        produce_node_contribution_with_rng(
            request,
            &signed_node_decision(
                request,
                node_seed,
                elastos_protected_content_contracts::RightsDecisionV1::Allowed,
            ),
            envelope,
            &node_signing_key(node_seed),
            &node_custody_secret(node_seed),
            &recipient_public_key(recipient_seed),
            NOW + 5,
            NOW + 45,
            NOW + 6,
            &mut StdRng::from_seed([hpke_seed; 32]),
        )
        .unwrap()
    }

    fn terminal_receipt(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        contributions: &[SignedNodeContributionV1],
        issuer_seed: u8,
        outcome: KeyReleaseOutcomeV1,
    ) -> SignedTerminalReceiptV1 {
        let verified = contributions
            .iter()
            .map(|contribution| {
                contribution
                    .verify(request, &envelope.manifest().node_set().unwrap(), NOW + 7)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
        let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
        let refs = match outcome {
            KeyReleaseOutcomeV1::Denied => Vec::new(),
            KeyReleaseOutcomeV1::Released => {
                verified.iter().map(NodeContributionRefV1::from).collect()
            }
        };
        let statement = TerminalReceiptStatementV1::new(
            request.request_hash(),
            request.binding().clone(),
            issuer,
            outcome,
            refs,
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

    fn tampered_ciphertext_contribution(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        node_seed: u8,
        recipient_seed: u8,
        hpke_seed: u8,
    ) -> SignedNodeContributionV1 {
        let valid = released_contribution(request, envelope, node_seed, recipient_seed, hpke_seed);
        let mut ciphertext = decode_ciphertext_bytes(
            valid
                .statement()
                .recipient_sealed_contribution()
                .sealed_bytes(),
        )
        .unwrap();
        let mut sealed = *ciphertext.ciphertext();
        sealed[0] ^= 0x55;
        ciphertext = HpkeCiphertextV1::new(*ciphertext.encapped_key(), sealed).unwrap();
        let sealed = RecipientSealedContributionV1::new(
            request.recipient().clone(),
            ciphertext.canonical_bytes().unwrap(),
        )
        .unwrap();
        let statement = NodeContributionStatementV1::new(
            request.request_hash(),
            request.binding().clone(),
            signed_node_decision(
                request,
                node_seed,
                elastos_protected_content_contracts::RightsDecisionV1::Allowed,
            ),
            sealed,
            NOW + 5,
            NOW + 45,
        )
        .unwrap();
        let signature = node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec();
        SignedNodeContributionV1::new(statement, signature).unwrap()
    }

    fn malicious_wrong_share_contribution(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        node_seed: u8,
        recipient_seed: u8,
        hpke_seed: u8,
    ) -> SignedNodeContributionV1 {
        let valid = released_contribution(request, envelope, node_seed, recipient_seed, hpke_seed);
        let verified = valid
            .verify(request, &envelope.manifest().node_set().unwrap(), NOW + 7)
            .unwrap();
        let node_entry = envelope
            .manifest()
            .node(verified.node_public_key())
            .unwrap()
            .clone();
        let released_aad = node_entry
            .released_share_aad_bytes(
                request.request_hash(),
                request.binding(),
                verified.decision_hash(),
                request.recipient(),
            )
            .unwrap();
        let original = decode_ciphertext_bytes(
            valid
                .statement()
                .recipient_sealed_contribution()
                .sealed_bytes(),
        )
        .unwrap();
        let mut plaintext = open_share(
            &original,
            recipient_secret(recipient_seed).secret_bytes(),
            elastos_protected_content_contracts::RELEASED_SHARE_HPKE_INFO_V1,
            &released_aad,
        )
        .unwrap();
        plaintext[0] ^= 0x01;
        let resealed = seal_share(
            recipient_public_key(recipient_seed).as_bytes(),
            elastos_protected_content_contracts::RELEASED_SHARE_HPKE_INFO_V1,
            &released_aad,
            &plaintext,
            &mut StdRng::from_seed([hpke_seed.wrapping_add(0x30); 32]),
        )
        .unwrap();
        let sealed = RecipientSealedContributionV1::new(
            request.recipient().clone(),
            resealed.canonical_bytes().unwrap(),
        )
        .unwrap();
        let statement = NodeContributionStatementV1::new(
            request.request_hash(),
            request.binding().clone(),
            signed_node_decision(
                request,
                node_seed,
                elastos_protected_content_contracts::RightsDecisionV1::Allowed,
            ),
            sealed,
            NOW + 5,
            NOW + 45,
        )
        .unwrap();
        let signature = node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec();
        SignedNodeContributionV1::new(statement, signature).unwrap()
    }

    fn alternate_envelope() -> CustodyEnvelopeV1 {
        provision_custody_envelope_with_rng(
            EncryptedContentIdentityV1::new(digest(0x99), 4096).unwrap(),
            &content_key(),
            ThresholdV1::new(2, 3).unwrap(),
            custody_nodes(),
            &mut StdRng::from_seed([0x51; 32]),
            &mut rand10::rngs::StdRng::from_seed([0x52; 32]),
        )
        .unwrap()
    }

    #[test]
    fn reconstructs_threshold_content_key() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let recovered = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap();

        assert_eq!(
            recovered.with_bytes(|bytes| *bytes),
            content_key().with_bytes(|bytes| *bytes)
        );
    }

    #[test]
    fn insufficient_shares_fail_before_open() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contribution = released_contribution(&request, &envelope, 1, 0x30, 0x71);
        let terminal =
            terminal_receipt(&request, &envelope, &[], 0x21, KeyReleaseOutcomeV1::Denied);

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &[contribution],
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::Release(KeyReleaseError::InsufficientContributions)
        ));
    }

    #[test]
    fn required_plus_one_shares_fail_before_open() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
            released_contribution(&request, &envelope, 3, 0x30, 0x73),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions[..2],
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::BindingMismatch("release_threshold")
        ));
    }

    #[test]
    fn duplicate_node_contributions_fail_before_open() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contribution = released_contribution(&request, &envelope, 1, 0x30, 0x71);
        let terminal =
            terminal_receipt(&request, &envelope, &[], 0x21, KeyReleaseOutcomeV1::Denied);

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &[contribution.clone(), contribution],
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::BindingMismatch("duplicate_contribution_node")
        ));
    }

    #[test]
    fn wrong_recipient_secret_is_rejected() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x31),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::BindingMismatch("recipient_key_identity")
        ));
    }

    #[test]
    fn wrong_request_contribution_is_rejected() {
        let envelope = provisioned_envelope();
        let request_a = verified_release_request_for_envelope(&envelope);
        let request_b = verified_release_request_for_envelope_and_recipient_seed(&envelope, 0x31);
        let contributions = vec![
            released_contribution(&request_a, &envelope, 1, 0x30, 0x71),
            released_contribution(&request_b, &envelope, 2, 0x31, 0x72),
        ];
        let terminal = terminal_receipt(
            &request_a,
            &envelope,
            &[],
            0x21,
            KeyReleaseOutcomeV1::Denied,
        );

        let err = reconstruct_content_key(
            &request_a,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::Release(KeyReleaseError::BindingMismatch(_))
        ));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            tampered_ciphertext_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(err, CustodyError::Hpke(_)));
    }

    #[test]
    fn malicious_signed_wrong_share_is_rejected_by_content_key_commitment() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            malicious_wrong_share_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(err, CustodyError::ContentKeyCommitmentMismatch));
    }

    #[test]
    fn expiry_is_rejected() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 40,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CustodyError::Release(KeyReleaseError::Rights(
                elastos_protected_content_contracts::RightsError::Expired
            ))
        ));
    }

    #[test]
    fn contributions_are_sorted_by_manifest_coordinate() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let recovered = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap();

        assert_eq!(
            recovered.with_bytes(|bytes| *bytes),
            content_key().with_bytes(|bytes| *bytes)
        );
    }

    #[test]
    fn reconstructed_key_debug_output_is_redacted() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let key = reconstruct_content_key(
            &request,
            &envelope,
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap();
        let debug = format!("{key:?}");

        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("22"));
    }

    #[test]
    fn wrong_envelope_binding_is_rejected() {
        let request = verified_release_request();
        let envelope = provisioned_envelope();
        let contributions = vec![
            released_contribution(&request, &envelope, 1, 0x30, 0x71),
            released_contribution(&request, &envelope, 2, 0x30, 0x72),
        ];
        let terminal = terminal_receipt(
            &request,
            &envelope,
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );

        let err = reconstruct_content_key(
            &request,
            &alternate_envelope(),
            &contributions,
            &terminal,
            terminal.statement().issuer(),
            &recipient_secret(0x30),
            NOW + 8,
        )
        .unwrap_err();

        assert!(matches!(err, CustodyError::BindingMismatch("key_envelope")));
    }
}
