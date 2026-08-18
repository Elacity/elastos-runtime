use elastos_protected_content_contracts::{
    CustodyEnvelopeManifestV1, CustodyEnvelopeV1, EncryptedContentIdentityV1, ThresholdV1,
    ValidatedCustodyCommitteeV1,
};
use hpke::rand_core::{CryptoRng as HpkeCryptoRng, RngCore as HpkeRngCore};
use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};
use rand10::{
    rngs::{StdRng as ShamirStdRng, SysRng as ShamirSysRng},
    CryptoRng as CryptoRng10, SeedableRng as _,
};
use vsss_rs::Gf256;

use crate::{
    hpke_helpers::seal_share, secrets::ContentEncryptionKeyV1, CustodyError, CONTENT_KEY_BYTES,
};

pub fn provision_custody_envelope(
    encrypted_content: EncryptedContentIdentityV1,
    content_key: &ContentEncryptionKeyV1,
    committee: &ValidatedCustodyCommitteeV1,
) -> Result<CustodyEnvelopeV1, CustodyError> {
    let mut hpke_rng =
        HpkeStdRng::try_from_os_rng().map_err(|_| CustodyError::RandomnessUnavailable)?;
    let mut shamir_rng = ShamirStdRng::try_from_rng(&mut ShamirSysRng)
        .map_err(|_| CustodyError::RandomnessUnavailable)?;
    provision_custody_envelope_with_rng(
        encrypted_content,
        content_key,
        committee,
        &mut hpke_rng,
        &mut shamir_rng,
    )
}

pub(crate) fn provision_custody_envelope_with_rng<RHpke, RShamir>(
    encrypted_content: EncryptedContentIdentityV1,
    content_key: &ContentEncryptionKeyV1,
    committee: &ValidatedCustodyCommitteeV1,
    hpke_rng: &mut RHpke,
    shamir_rng: &mut RShamir,
) -> Result<CustodyEnvelopeV1, CustodyError>
where
    RHpke: HpkeCryptoRng + HpkeRngCore,
    RShamir: CryptoRng10,
{
    let verified_epoch = committee.committee();
    let manifest = CustodyEnvelopeManifestV1::new(
        encrypted_content,
        committee.pool_identity(),
        verified_epoch.epoch_identity(),
        committee.authorization_identity(),
        verified_epoch.threshold(),
        content_key.commitment(),
        verified_epoch.nodes().to_vec(),
    )?;
    let manifest_hash = manifest.manifest_hash()?;
    let share_values = split_content_key(content_key, manifest.threshold(), shamir_rng)?;
    let mut stored_shares = Vec::with_capacity(share_values.len());
    for (node, share_value) in manifest.nodes().iter().zip(share_values.iter()) {
        let aad = node.stored_share_aad_bytes(manifest_hash)?;
        stored_shares.push(seal_share(
            node.custody_public_key().as_bytes(),
            elastos_protected_content_contracts::STORED_SHARE_HPKE_INFO_V1,
            &aad,
            share_value,
            hpke_rng,
        )?);
    }
    CustodyEnvelopeV1::new(manifest, stored_shares).map_err(Into::into)
}

fn split_content_key<R: CryptoRng10>(
    content_key: &ContentEncryptionKeyV1,
    threshold: ThresholdV1,
    shamir_rng: &mut R,
) -> Result<Vec<zeroize::Zeroizing<[u8; CONTENT_KEY_BYTES]>>, CustodyError> {
    let raw_shares = zeroize::Zeroizing::new(content_key.with_bytes(|bytes| {
        Gf256::split_bytes(
            usize::from(threshold.required()),
            usize::from(threshold.total()),
            bytes,
            shamir_rng,
        )
    })?);
    normalize_share_values(raw_shares, threshold.total())
}

fn normalize_share_values(
    mut raw_shares: zeroize::Zeroizing<Vec<Vec<u8>>>,
    total: u8,
) -> Result<Vec<zeroize::Zeroizing<[u8; CONTENT_KEY_BYTES]>>, CustodyError> {
    let mut ordered = vec![None; usize::from(total)];
    while let Some(share) = raw_shares.pop() {
        let share = zeroize::Zeroizing::new(share);
        if share.len() != CONTENT_KEY_BYTES + 1 {
            return Err(CustodyError::MalformedShare("split_share_length"));
        }
        let coordinate = share[0];
        if coordinate == 0 || coordinate > total {
            return Err(CustodyError::MalformedShare("split_share_coordinate"));
        }
        let mut value = zeroize::Zeroizing::new([0u8; CONTENT_KEY_BYTES]);
        value.copy_from_slice(&share[1..]);
        let slot = &mut ordered[usize::from(coordinate - 1)];
        if slot.replace(value).is_some() {
            return Err(CustodyError::MalformedShare(
                "duplicate_split_share_coordinate",
            ));
        }
    }
    ordered
        .into_iter()
        .map(|share| share.ok_or(CustodyError::MalformedShare("missing_split_share")))
        .collect()
}

#[cfg(test)]
mod tests {
    use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};
    use rand10::{rngs::StdRng as ShamirStdRng, SeedableRng as _};

    use elastos_protected_content_contracts::NodeCustodyPublicKeyV1;

    use super::*;
    use crate::test_support::{content_key, digest, validated_custody_committee};

    #[test]
    fn provision_is_deterministic_under_test_rng_and_binds_identity() {
        let mut hpke_rng_a = HpkeStdRng::from_seed([0x41; 32]);
        let mut shamir_rng_a = ShamirStdRng::from_seed([0x42; 32]);
        let mut hpke_rng_b = HpkeStdRng::from_seed([0x41; 32]);
        let mut shamir_rng_b = ShamirStdRng::from_seed([0x42; 32]);
        let envelope_a = provision_custody_envelope_with_rng(
            EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
            &content_key(),
            &validated_custody_committee(),
            &mut hpke_rng_a,
            &mut shamir_rng_a,
        )
        .unwrap();
        let envelope_b = provision_custody_envelope_with_rng(
            EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
            &content_key(),
            &validated_custody_committee(),
            &mut hpke_rng_b,
            &mut shamir_rng_b,
        )
        .unwrap();
        assert_eq!(envelope_a, envelope_b);
        assert!(envelope_a
            .matches_key_envelope_identity(&envelope_a.key_envelope_identity().unwrap())
            .unwrap());
    }

    #[test]
    fn provision_derives_authority_from_validated_committee_only() {
        let committee = validated_custody_committee();
        let envelope = provision_custody_envelope(
            EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
            &content_key(),
            &committee,
        )
        .unwrap();
        assert_eq!(
            envelope.manifest().custody_pool(),
            committee.pool_identity()
        );
        assert_eq!(
            envelope.manifest().custody_epoch(),
            committee.committee().epoch_identity()
        );
        assert_eq!(
            envelope.manifest().custody_committee_authorization(),
            committee.authorization_identity()
        );
        assert_eq!(
            envelope.manifest().threshold(),
            committee.committee().threshold()
        );
        assert_eq!(envelope.manifest().nodes(), committee.committee().nodes());
    }

    #[test]
    fn contract_rejects_low_order_node_custody_public_key_before_provision() {
        let mut low_order = [0u8; 32];
        low_order[0] = 1;
        assert_eq!(
            NodeCustodyPublicKeyV1::new(low_order),
            Err(
                elastos_protected_content_contracts::ContractError::InvalidField(
                    "node_custody_public_key"
                )
            )
        );
    }
}
