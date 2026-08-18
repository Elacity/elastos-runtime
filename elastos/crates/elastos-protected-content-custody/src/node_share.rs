use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, CanonicalContract, CustodyEnvelopeManifestV1,
    CustodyEnvelopeV1, CustodyNodeIdentityV1, HpkeCiphertextV1, KeyEnvelopeIdentityV1,
    NodePublicKey, NodeSetV1, RuntimeReleaseOperationError,
};

use crate::CustodyError;

/// One custody node's local share state for one protected object.
///
/// This value is created only by extracting the selected node's encrypted share
/// from a fully validated private provisioning envelope. It carries the public
/// object/committee identities needed to verify release authority plus exactly
/// one node-sealed share. It never carries the aggregate envelope's other node
/// shares.
#[derive(Clone, PartialEq, Eq)]
pub struct NodeLocalStoredShareV1 {
    key_envelope_identity: KeyEnvelopeIdentityV1,
    manifest: CustodyEnvelopeManifestV1,
    node_public_key: NodePublicKey,
    stored_share: HpkeCiphertextV1,
}

impl std::fmt::Debug for NodeLocalStoredShareV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeLocalStoredShareV1")
            .field("key_envelope_identity", &self.key_envelope_identity)
            .field("manifest", &self.manifest)
            .field("node_public_key", &self.node_public_key)
            .field("stored_share", &"[redacted]")
            .finish()
    }
}

impl NodeLocalStoredShareV1 {
    pub fn extract_from_envelope(
        envelope: &CustodyEnvelopeV1,
        node_public_key: NodePublicKey,
    ) -> Result<Self, CustodyError> {
        let key_envelope_identity = envelope.key_envelope_identity()?;
        let manifest = envelope.manifest().clone();
        let stored_share = envelope
            .stored_share_for_node(node_public_key)
            .ok_or(CustodyError::BindingMismatch("stored_share"))?
            .clone();
        let value = Self {
            key_envelope_identity,
            manifest,
            node_public_key,
            stored_share,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn key_envelope_identity(&self) -> &KeyEnvelopeIdentityV1 {
        &self.key_envelope_identity
    }

    pub fn manifest(&self) -> &CustodyEnvelopeManifestV1 {
        &self.manifest
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub fn node(&self) -> Result<&CustodyNodeIdentityV1, CustodyError> {
        self.manifest
            .node(self.node_public_key)
            .ok_or(CustodyError::BindingMismatch("custody_node"))
    }

    pub fn node_set(&self) -> Result<NodeSetV1, CustodyError> {
        Ok(self.manifest.node_set()?)
    }

    pub fn stored_share(&self) -> &HpkeCiphertextV1 {
        &self.stored_share
    }

    pub(crate) fn stored_share_aad_bytes(&self) -> Result<Vec<u8>, CustodyError> {
        Ok(self
            .node()?
            .stored_share_aad_bytes(self.manifest.manifest_hash()?)?)
    }

    pub(crate) fn validate_release_claim_context(
        &self,
        operation: &AuthenticatedRuntimeReleaseOperationV1,
        selected_node_public_key: NodePublicKey,
        now: u64,
    ) -> Result<(), CustodyError> {
        validate_operation_active_window(operation, now)?;
        if selected_node_public_key != self.node_public_key {
            return Err(CustodyError::BindingMismatch("custody_node"));
        }
        if operation.binding().key_envelope() != &self.key_envelope_identity {
            return Err(CustodyError::BindingMismatch("key_envelope"));
        }
        if operation.custody_epoch_identity() != self.manifest.custody_epoch() {
            return Err(CustodyError::BindingMismatch("custody_epoch"));
        }
        self.node()?;
        Ok(())
    }

    fn validate(&self) -> Result<(), CustodyError> {
        self.key_envelope_identity.canonical_bytes()?;
        self.manifest.canonical_bytes()?;
        self.stored_share.canonical_bytes()?;
        self.node()?;
        let node_set = self.manifest.node_set()?;
        if node_set.node_set_id()? != self.key_envelope_identity.node_set_id() {
            return Err(CustodyError::BindingMismatch("node_set"));
        }
        if self.manifest.encrypted_content() != self.key_envelope_identity.encrypted_content() {
            return Err(CustodyError::BindingMismatch("encrypted_content"));
        }
        if self.manifest.threshold() != self.key_envelope_identity.threshold() {
            return Err(CustodyError::BindingMismatch("threshold"));
        }
        if self.manifest.custody_pool() != self.key_envelope_identity.custody_pool() {
            return Err(CustodyError::BindingMismatch("custody_pool"));
        }
        if self.manifest.custody_epoch() != self.key_envelope_identity.custody_epoch() {
            return Err(CustodyError::BindingMismatch("custody_epoch"));
        }
        if self.manifest.custody_committee_authorization()
            != self.key_envelope_identity.custody_committee_authorization()
        {
            return Err(CustodyError::BindingMismatch(
                "custody_committee_authorization",
            ));
        }
        Ok(())
    }
}

fn validate_operation_active_window(
    operation: &AuthenticatedRuntimeReleaseOperationV1,
    now: u64,
) -> Result<(), CustodyError> {
    let statement = operation.statement();
    if now < statement.issued_at() {
        return Err(CustodyError::RuntimeReleaseOperation(
            RuntimeReleaseOperationError::NotYetValid,
        ));
    }
    if now >= statement.expires_at() {
        return Err(CustodyError::RuntimeReleaseOperation(
            RuntimeReleaseOperationError::Expired,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};
    use rand10::rngs::StdRng as ShamirStdRng;
    use rand10::SeedableRng as _;

    use super::*;
    use crate::{
        provision::provision_custody_envelope_with_rng,
        test_support::{
            authenticated_runtime_release_operation_for_envelope_and_recipient_seed, content_key,
            custody_committee_authorization_identity, custody_epoch_identity, custody_nodes,
            custody_pool_identity, digest, node_custody_secret, node_public_key,
            provisioned_envelope, NOW,
        },
    };
    use elastos_protected_content_contracts::{
        CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1, CustodyPoolIdentityV1,
        EncryptedContentIdentityV1, ThresholdV1,
    };

    #[allow(clippy::too_many_arguments)]
    fn provisioned_envelope_with(
        encrypted_content: EncryptedContentIdentityV1,
        custody_pool: CustodyPoolIdentityV1,
        custody_epoch: CustodyEpochIdentityV1,
        custody_committee_authorization: CustodyCommitteeAuthorizationIdentityV1,
        threshold: ThresholdV1,
        nodes: Vec<(
            NodePublicKey,
            elastos_protected_content_contracts::NodeCustodyPublicKeyV1,
        )>,
        seed: u8,
    ) -> CustodyEnvelopeV1 {
        provision_custody_envelope_with_rng(
            encrypted_content,
            &content_key(),
            custody_pool,
            custody_epoch,
            custody_committee_authorization,
            threshold,
            nodes,
            &mut HpkeStdRng::from_seed([seed; 32]),
            &mut ShamirStdRng::from_seed([seed.wrapping_add(1); 32]),
        )
        .unwrap()
    }

    #[test]
    fn node_local_share_extracts_only_the_selected_node_share() {
        let envelope = provisioned_envelope();
        let selected_node = node_public_key(1);
        let node_share =
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, selected_node).unwrap();

        assert_eq!(node_share.node_public_key(), selected_node);
        assert_eq!(
            node_share.key_envelope_identity(),
            &envelope.key_envelope_identity().unwrap()
        );
        assert_eq!(node_share.manifest(), envelope.manifest());
        assert_eq!(
            node_share.stored_share(),
            envelope.stored_share_for_node(selected_node).unwrap()
        );

        for other_node in [node_public_key(2), node_public_key(3)] {
            assert_ne!(
                node_share.stored_share(),
                envelope.stored_share_for_node(other_node).unwrap()
            );
        }
    }

    #[test]
    fn node_local_share_rejects_unknown_selected_node() {
        let envelope = provisioned_envelope();
        assert!(matches!(
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, node_public_key(4)),
            Err(CustodyError::BindingMismatch("stored_share"))
        ));
    }

    #[test]
    fn node_local_share_rejects_substituted_object_authority() {
        let envelope = provisioned_envelope();
        let node_share =
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, node_public_key(1)).unwrap();
        let cases = [
            (
                "content",
                provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x12), 4096).unwrap(),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x51,
                ),
            ),
            (
                "pool",
                provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    CustodyPoolIdentityV1::new(digest(0x37), 512).unwrap(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x52,
                ),
            ),
            (
                "committee_authorization",
                provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x38), 512).unwrap(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x54,
                ),
            ),
            (
                "commitment",
                provision_custody_envelope_with_rng(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    &crate::ContentEncryptionKeyV1::from_test_bytes([0x23; 32]),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    &mut HpkeStdRng::from_seed([0x57; 32]),
                    &mut ShamirStdRng::from_seed([0x58; 32]),
                )
                .unwrap(),
            ),
        ];

        for (label, substituted) in cases {
            let operation = authenticated_runtime_release_operation_for_envelope_and_recipient_seed(
                &substituted,
                0x30,
            );
            let err = node_share
                .validate_release_claim_context(&operation, node_public_key(1), NOW + 3)
                .unwrap_err();
            assert!(
                matches!(err, CustodyError::BindingMismatch("key_envelope")),
                "{label} substitution should fail at the bound key envelope, got {err:?}"
            );
        }
    }

    #[test]
    fn node_local_share_private_validation_rejects_tampered_authority_fields() {
        let envelope = provisioned_envelope();
        let node_share =
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, node_public_key(1)).unwrap();
        let mut changed_nodes = custody_nodes();
        changed_nodes[2] = (
            node_public_key(4),
            node_custody_secret(4).public_key().unwrap(),
        );
        let original_identity = node_share.key_envelope_identity().clone();
        let cases = [
            ("content", {
                let mut tampered = node_share.clone();
                tampered.manifest = provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x12), 4096).unwrap(),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x61,
                )
                .manifest()
                .clone();
                tampered
            }),
            ("pool", {
                let mut tampered = node_share.clone();
                tampered.manifest = provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    CustodyPoolIdentityV1::new(digest(0x37), 512).unwrap(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x62,
                )
                .manifest()
                .clone();
                tampered
            }),
            ("epoch", {
                let mut tampered = node_share.clone();
                tampered.manifest = provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    custody_pool_identity(),
                    CustodyEpochIdentityV1::new(digest(0x39), 512).unwrap(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x63,
                )
                .manifest()
                .clone();
                tampered
            }),
            ("committee_authorization", {
                let mut tampered = node_share.clone();
                tampered.manifest = provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x38), 512).unwrap(),
                    ThresholdV1::new(2, 3).unwrap(),
                    custody_nodes(),
                    0x64,
                )
                .manifest()
                .clone();
                tampered
            }),
            ("node_set", {
                let mut tampered = node_share.clone();
                tampered.manifest = provisioned_envelope_with(
                    EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
                    custody_pool_identity(),
                    custody_epoch_identity(),
                    custody_committee_authorization_identity(),
                    ThresholdV1::new(2, 3).unwrap(),
                    changed_nodes,
                    0x65,
                )
                .manifest()
                .clone();
                tampered
            }),
            ("threshold", {
                let mut tampered = node_share.clone();
                tampered.key_envelope_identity = KeyEnvelopeIdentityV1::new(
                    original_identity.encrypted_content().clone(),
                    original_identity.envelope_sha256(),
                    original_identity.envelope_bytes(),
                    original_identity.node_set_id(),
                    ThresholdV1::new(3, 3).unwrap(),
                    original_identity.custody_pool(),
                    original_identity.custody_epoch(),
                    original_identity.custody_committee_authorization(),
                )
                .unwrap();
                tampered
            }),
        ];

        for (label, tampered) in cases {
            let err = tampered.validate().unwrap_err();
            assert!(
                matches!(err, CustodyError::BindingMismatch(_)),
                "{label} tamper should fail closed, got {err:?}"
            );
        }
    }

    #[test]
    fn node_local_share_rejects_selected_node_substitution() {
        let envelope = provisioned_envelope();
        let node_share =
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, node_public_key(1)).unwrap();
        let operation = authenticated_runtime_release_operation_for_envelope_and_recipient_seed(
            &envelope, 0x30,
        );
        assert!(matches!(
            node_share.validate_release_claim_context(&operation, node_public_key(2), NOW + 3),
            Err(CustodyError::BindingMismatch("custody_node"))
        ));
    }

    #[test]
    fn node_local_share_debug_redacts_the_sealed_share() {
        let envelope = provisioned_envelope();
        let node_share =
            NodeLocalStoredShareV1::extract_from_envelope(&envelope, node_public_key(1)).unwrap();
        let debug = format!("{node_share:?}");
        let stored_share = envelope.stored_share_for_node(node_public_key(1)).unwrap();
        assert!(debug.contains("NodeLocalStoredShareV1"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(&format!("{:?}", stored_share.encapped_key())));
        assert!(!debug.contains(&format!("{:?}", stored_share.ciphertext())));
    }
}
