#![forbid(unsafe_code)]

//! Source-only protected-content rights evaluation boundary.
//!
//! Runtime selects the provider outside this crate. This crate consumes one
//! already authenticated Runtime release request and returns the existing signed
//! node decision response. Chain evidence is acquired through a Runtime-owned
//! invoke callback; this crate does not dial RPC, accept caller-supplied
//! evidence, or carry routes, topology, CEKs, or custody shares.

mod chain;

pub use chain::{
    chain_rights_evidence_request, evaluate_rights_via_chain, parse_chain_rights_evidence_data,
    CHAIN_PROVIDER_ID, CHAIN_RIGHTS_EVIDENCE_OP,
};

use ed25519_dalek::{Signer as _, SigningKey};
use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, KeyReleaseError, NodePublicKey,
    NodeRightsDecisionStatementV1, RightsDecisionV1, RightsEvaluationEvidenceV1,
    SignedNodeRightsDecisionV1, MAX_NODE_DECISION_LIFETIME_SECS,
};
use elastos_protected_content_provider_contracts::{
    RightsProviderResponseV1, ValidatedRightsProviderRequestV1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateCustodyRightsRequestV1 {
    PrepareEvidence { request: Value },
    SettleEvidence { request: Value, chain_data: Value },
}

impl PrivateCustodyRightsRequestV1 {
    pub fn new_prepare(request: Value) -> Self {
        Self::PrepareEvidence { request }
    }

    pub fn new_settle(request: Value, chain_data: Value) -> Self {
        Self::SettleEvidence {
            request,
            chain_data,
        }
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn request_json(&self) -> &Value {
        match self {
            Self::PrepareEvidence { request } | Self::SettleEvidence { request, .. } => request,
        }
    }

    pub fn chain_data(&self) -> Option<&Value> {
        match self {
            Self::PrepareEvidence { .. } => None,
            Self::SettleEvidence { chain_data, .. } => Some(chain_data),
        }
    }
}

pub type RightsEvidenceFutureV1<'a> = Pin<
    Box<
        dyn Future<Output = Result<RightsEvaluationEvidenceV1, RightsEvaluationErrorV1>>
            + Send
            + 'a,
    >,
>;

pub trait TrustedRightsEvidenceSourceV1 {
    fn acquire_evidence_at(
        &self,
        request: &ValidatedRightsProviderRequestV1,
        now_unix_seconds: u64,
    ) -> RightsEvidenceFutureV1<'_>;
}

pub struct ProtectedContentRightsEvaluatorV1<S> {
    node_public_key: NodePublicKey,
    node_signing_key: SigningKey,
    evidence_source: S,
}

impl<S> core::fmt::Debug for ProtectedContentRightsEvaluatorV1<S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProtectedContentRightsEvaluatorV1")
            .field("node_public_key", &self.node_public_key)
            .finish_non_exhaustive()
    }
}

impl<S: TrustedRightsEvidenceSourceV1> ProtectedContentRightsEvaluatorV1<S> {
    pub fn new(
        node_signing_key: SigningKey,
        evidence_source: S,
    ) -> Result<Self, RightsEvaluationErrorV1> {
        let node_public_key = NodePublicKey::new(node_signing_key.verifying_key().to_bytes())
            .map_err(|_| RightsEvaluationErrorV1::InvalidNodeSigningKey)?;
        Ok(Self {
            node_public_key,
            node_signing_key,
            evidence_source,
        })
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub async fn evaluate_at(
        &self,
        request: &ValidatedRightsProviderRequestV1,
        now_unix_seconds: u64,
    ) -> Result<RightsProviderResponseV1, RightsEvaluationErrorV1> {
        if request.selected_node_public_key() != self.node_public_key {
            return Err(RightsEvaluationErrorV1::WrongSelectedNode);
        }
        let evidence = self
            .evidence_source
            .acquire_evidence_at(request, now_unix_seconds)
            .await?;
        evaluate_validated_rights_with_evidence_at(
            &self.node_signing_key,
            request,
            evidence,
            now_unix_seconds,
        )
    }
}

pub fn evaluate_validated_rights_with_evidence_at(
    node_signing_key: &SigningKey,
    request: &ValidatedRightsProviderRequestV1,
    evidence: RightsEvaluationEvidenceV1,
    now_unix_seconds: u64,
) -> Result<RightsProviderResponseV1, RightsEvaluationErrorV1> {
    let node_public_key = NodePublicKey::new(node_signing_key.verifying_key().to_bytes())
        .map_err(|_| RightsEvaluationErrorV1::InvalidNodeSigningKey)?;
    if request.selected_node_public_key() != node_public_key {
        return Err(RightsEvaluationErrorV1::WrongSelectedNode);
    }
    let authenticated = request.authenticated_runtime_release_operation();
    let statement = authenticated.statement();
    evidence
        .validate_against_runtime_release_at(authenticated, now_unix_seconds)
        .map_err(|_| RightsEvaluationErrorV1::EvidenceBinding)?;
    let max_expires_at = now_unix_seconds
        .checked_add(MAX_NODE_DECISION_LIFETIME_SECS)
        .ok_or(RightsEvaluationErrorV1::DecisionWindow)?;
    let expires_at = max_expires_at
        .min(statement.expires_at())
        .min(statement.release_request().expires_at());
    if expires_at <= now_unix_seconds {
        return Err(RightsEvaluationErrorV1::DecisionWindow);
    }
    let decision = if evidence.has_access() {
        RightsDecisionV1::Allowed
    } else {
        RightsDecisionV1::Denied
    };
    let decision_statement = NodeRightsDecisionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.rights_request_hash(),
        authenticated.binding().clone(),
        authenticated.action(),
        node_public_key,
        decision,
        evidence.canonical_hash()?,
        now_unix_seconds,
        expires_at,
    )?;
    let signed_decision = SignedNodeRightsDecisionV1::new(
        decision_statement.clone(),
        node_signing_key
            .sign(&decision_statement.canonical_bytes()?)
            .to_bytes()
            .to_vec(),
    )?;
    let node_set = statement
        .custody_epoch()
        .statement()
        .node_set()
        .map_err(|_| RightsEvaluationErrorV1::DecisionBinding)?;
    authenticated
        .verify_node_rights_decision(&signed_decision, &node_set, now_unix_seconds)
        .map_err(RightsEvaluationErrorV1::from)?;
    RightsProviderResponseV1::new_decision(&signed_decision).map_err(Into::into)
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RightsEvaluationErrorV1 {
    #[error("invalid node signing key")]
    InvalidNodeSigningKey,
    #[error("selected node does not match evaluator")]
    WrongSelectedNode,
    #[error("rights evidence does not match request")]
    EvidenceBinding,
    #[error("rights decision window is invalid")]
    DecisionWindow,
    #[error("rights decision does not match request")]
    DecisionBinding,
    #[error("rights evidence source failed")]
    EvidenceSource,
    #[error("chain rights evidence is invalid")]
    ChainEvidence,
    #[error("protected-content contract error")]
    Contract,
    #[error("protected-content release binding error")]
    ReleaseBinding,
}

impl From<ContractError> for RightsEvaluationErrorV1 {
    fn from(_: ContractError) -> Self {
        Self::Contract
    }
}

impl From<KeyReleaseError> for RightsEvaluationErrorV1 {
    fn from(_: KeyReleaseError) -> Self {
        Self::ReleaseBinding
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use elastos_auth::ethereum_signed_message_hash;
    use elastos_protected_content_contracts::{
        CanonicalContract, ContentAccessIdV1, CustodyApprovedSuitesV1,
        CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeManifestV1, CustodyEnvelopeV1,
        CustodyEpochIssuerKeyV1, CustodyEpochStatementV1, CustodyPoolIdentityV1, Digest32,
        EncryptedContentIdentityV1, EvmContractAddressV1, EvmFunctionSelectorV1,
        EvmRightsMethodAbiV1, KeyReleaseRequestV1, NodeCustodyPublicKeyV1, PqHybridSealedShareV1,
        ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1,
        RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1,
        RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
        RightsSubjectSourceV1, RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1,
        RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1, ShareCoordinateV1,
        SignedCustodyEpochV1, SignedRecipientKeyAuthorizationV1, SignedRuntimeReleaseOperationV1,
        ThresholdV1, WalletAddress, WalletSignedRightsRequestV1,
        CUSTODY_X_WING_AES256GCM_SUITE_ID_V1, PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES,
        X_WING_DRAFT06_CIPHERTEXT_BYTES,
    };
    use elastos_protected_content_provider_contracts::{
        RightsProviderRequestV1, RightsProviderResponseStatusV1, ValidatedRightsProviderRequestV1,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use sha3::{Digest as _, Keccak256};
    use std::{
        future::Future,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        task::{Context, Poll, Waker},
    };
    use x_wing::kem::{Decapsulator as _, KeyExport as _};
    use x_wing::TryKeyInit as _;

    const NOW: u64 = 2_000_000_000;
    const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
    const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

    fn digest(seed: u8) -> Digest32 {
        Digest32::new([seed; 32])
    }

    fn node_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn wallet(seed: u8) -> WalletAddress {
        let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
        let encoded = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
        RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(seed.max(9))).unwrap()
    }

    fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
        recipient_public_key(seed)
            .key_identity(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
            .unwrap()
    }

    fn xwing_public_key_bytes(
        seed: u8,
    ) -> [u8; elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
        let secret = x_wing::DecapsulationKey::from([seed; x_wing::DECAPSULATION_KEY_SIZE]);
        secret.encapsulation_key().to_bytes().into()
    }

    fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        NodeCustodyPublicKeyV1::new(xwing_public_key_bytes(seed)).unwrap()
    }

    fn sealed_share(seed: u8) -> PqHybridSealedShareV1 {
        let public =
            x_wing::EncapsulationKey::new_from_slice(&xwing_public_key_bytes(seed)).unwrap();
        let (ciphertext, _) =
            public.encapsulate_deterministic(&[seed; x_wing::ENCAPSULATION_RANDOMNESS_SIZE].into());
        let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
        let mut envelope = Vec::with_capacity(PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES);
        envelope.extend_from_slice(&ciphertext);
        envelope.extend_from_slice(&[seed; PQ_HYBRID_AEAD_NONCE_BYTES]);
        envelope.extend_from_slice(&[seed ^ 0x5a; PQ_HYBRID_WRAPPED_SHARE_BYTES]);
        PqHybridSealedShareV1::new(envelope).unwrap()
    }

    fn policy_body() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            EncryptedContentIdentityV1::new(digest(0x41), 4096).unwrap(),
            ContentAccessIdV1::new([0x51; 16]).unwrap(),
            RightsActionV1::View,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap()
    }

    fn signed_custody_epoch() -> SignedCustodyEpochV1 {
        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        let nodes = vec![
            elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
                node_public_key(1),
                node_custody_public_key(0x31),
                ShareCoordinateV1::new(1).unwrap(),
            )
            .unwrap(),
            elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
                node_public_key(2),
                node_custody_public_key(0x32),
                ShareCoordinateV1::new(2).unwrap(),
            )
            .unwrap(),
            elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
                node_public_key(3),
                node_custody_public_key(0x33),
                ShareCoordinateV1::new(3).unwrap(),
            )
            .unwrap(),
        ];
        let statement = CustodyEpochStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
            CustodyApprovedSuitesV1::new(
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
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

    fn custody_envelope(seed: u8) -> CustodyEnvelopeV1 {
        let epoch = signed_custody_epoch();
        let manifest = CustodyEnvelopeManifestV1::new(
            EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap(),
            CustodyPoolIdentityV1::new(digest(seed ^ 0x34), 512).unwrap(),
            epoch.epoch_identity().unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(seed ^ 0x35), 512).unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            digest(seed ^ 0x33),
            epoch.statement().nodes().to_vec(),
        )
        .unwrap();
        let shares = [seed ^ 0x50, seed ^ 0x51, seed ^ 0x52]
            .into_iter()
            .map(sealed_share)
            .collect();
        CustodyEnvelopeV1::new(manifest, shares).unwrap()
    }

    fn binding_for_envelope(envelope: &CustodyEnvelopeV1) -> ProtectedContentBindingV1 {
        let policy = policy_body();
        ProtectedContentBindingV1::new(
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

    fn signed_operation_for_envelope(
        seed: u8,
        envelope: &CustodyEnvelopeV1,
    ) -> SignedRuntimeReleaseOperationV1 {
        let runtime_key = SigningKey::from_bytes(&[seed; 32]);
        let binding = binding_for_envelope(envelope);
        let rights_request = {
            let request = elastos_protected_content_contracts::RightsRequestV1::new(
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
        let evidence_request = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
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

    fn signed_operation(seed: u8) -> SignedRuntimeReleaseOperationV1 {
        signed_operation_for_envelope(seed, &custody_envelope(0x11))
    }

    fn validated_request(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
    ) -> ValidatedRightsProviderRequestV1 {
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(node_seed), operation).unwrap();
        ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            operation.statement().runtime_operation_issuer(),
            NOW + 3,
        )
        .unwrap()
    }

    fn evidence_for(
        request: &ValidatedRightsProviderRequestV1,
        has_access: bool,
    ) -> RightsEvaluationEvidenceV1 {
        let authenticated = request.authenticated_runtime_release_operation();
        let policy = authenticated.statement().policy_body();
        let evidence_request = authenticated.statement().evidence_request();
        RightsEvaluationEvidenceV1::new(
            authenticated.operation_hash(),
            authenticated.release_request_hash(),
            evidence_request.binding().clone(),
            evidence_request.policy_identity().clone(),
            evidence_request.binding().wallet(),
            policy.chain_id(),
            100,
            digest(0x88),
            has_access,
            NOW + 4,
            NOW + 30,
        )
        .unwrap()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Clone)]
    struct StaticEvidenceSource {
        evidence: RightsEvaluationEvidenceV1,
        calls: Arc<AtomicUsize>,
    }

    impl TrustedRightsEvidenceSourceV1 for StaticEvidenceSource {
        fn acquire_evidence_at(
            &self,
            _request: &ValidatedRightsProviderRequestV1,
            _now_unix_seconds: u64,
        ) -> RightsEvidenceFutureV1<'_> {
            let evidence = self.evidence.clone();
            let calls = Arc::clone(&self.calls);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(evidence)
            })
        }
    }

    #[derive(Clone)]
    struct FailingEvidenceSource {
        calls: Arc<AtomicUsize>,
    }

    impl TrustedRightsEvidenceSourceV1 for FailingEvidenceSource {
        fn acquire_evidence_at(
            &self,
            _request: &ValidatedRightsProviderRequestV1,
            _now_unix_seconds: u64,
        ) -> RightsEvidenceFutureV1<'_> {
            let calls = Arc::clone(&self.calls);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(RightsEvaluationErrorV1::EvidenceSource)
            })
        }
    }

    fn evaluator_with_evidence(
        node_seed: u8,
        evidence: RightsEvaluationEvidenceV1,
    ) -> (
        ProtectedContentRightsEvaluatorV1<StaticEvidenceSource>,
        Arc<AtomicUsize>,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            ProtectedContentRightsEvaluatorV1::new(
                node_signing_key(node_seed),
                StaticEvidenceSource {
                    evidence,
                    calls: Arc::clone(&calls),
                },
            )
            .unwrap(),
            calls,
        )
    }

    fn assert_no_forbidden_output(value: &[u8]) {
        let text = String::from_utf8_lossy(value).to_ascii_lowercase();
        for forbidden in [
            "cek",
            "share",
            "route",
            "endpoint",
            "host",
            "ip",
            "port",
            "rpc",
            "topology",
            "credential",
        ] {
            assert!(
                !text.contains(forbidden),
                "forbidden output marker {forbidden} in {text}"
            );
        }
    }

    #[test]
    fn typed_rights_evaluator_signs_exact_allow_and_deny_decisions() {
        let operation = signed_operation(0x42);
        let allow_request = validated_request(&operation, 1);
        let (evaluator, _) = evaluator_with_evidence(1, evidence_for(&allow_request, true));
        let allow = block_on(evaluator.evaluate_at(&allow_request, NOW + 4)).unwrap();
        assert_eq!(allow.status(), RightsProviderResponseStatusV1::Decision);
        let allow_decision = allow.signed_node_rights_decision().unwrap();
        assert_eq!(
            allow_decision.statement().decision(),
            RightsDecisionV1::Allowed
        );
        assert_eq!(
            allow_decision.statement().node_public_key(),
            node_public_key(1)
        );
        let authenticated = allow_request.authenticated_runtime_release_operation();
        let node_set = authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .unwrap();
        authenticated
            .verify_node_rights_decision(&allow_decision, &node_set, NOW + 4)
            .unwrap();

        let (deny_evaluator, _) = evaluator_with_evidence(1, evidence_for(&allow_request, false));
        let deny = block_on(deny_evaluator.evaluate_at(&allow_request, NOW + 4)).unwrap();
        assert_eq!(
            deny.signed_node_rights_decision()
                .unwrap()
                .statement()
                .decision(),
            RightsDecisionV1::Denied
        );
    }

    #[test]
    fn typed_rights_evaluator_rejects_substituted_request_evidence_and_node() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let other_operation = signed_operation_for_envelope(0x42, &custody_envelope(0x12));
        let other_request = validated_request(&other_operation, 1);
        let (evaluator, _) = evaluator_with_evidence(1, evidence_for(&other_request, true));

        assert_eq!(
            block_on(evaluator.evaluate_at(&request, NOW + 4)).unwrap_err(),
            RightsEvaluationErrorV1::EvidenceBinding
        );
        let (wrong_node, calls) = evaluator_with_evidence(2, evidence_for(&request, true));
        assert_eq!(
            block_on(wrong_node.evaluate_at(&request, NOW + 4)).unwrap_err(),
            RightsEvaluationErrorV1::WrongSelectedNode
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn typed_rights_evaluator_rejects_pre_operation_chain_evidence() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let authenticated = request.authenticated_runtime_release_operation();
        let evidence_request = authenticated.statement().evidence_request();
        let policy = authenticated.statement().policy_body();
        let stale = RightsEvaluationEvidenceV1::new(
            authenticated.operation_hash(),
            authenticated.release_request_hash(),
            evidence_request.binding().clone(),
            evidence_request.policy_identity().clone(),
            evidence_request.binding().wallet(),
            policy.chain_id(),
            100,
            digest(0x88),
            true,
            NOW + 1,
            NOW + 30,
        )
        .unwrap();
        let (evaluator, _) = evaluator_with_evidence(1, stale);
        assert_eq!(
            block_on(evaluator.evaluate_at(&request, NOW + 4)).unwrap_err(),
            RightsEvaluationErrorV1::EvidenceBinding
        );
    }

    #[test]
    fn typed_rights_evaluator_rejects_future_and_expired_evidence() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let authenticated = request.authenticated_runtime_release_operation();
        let policy = authenticated.statement().policy_body();
        let evidence_request = authenticated.statement().evidence_request();
        let future = RightsEvaluationEvidenceV1::new(
            authenticated.operation_hash(),
            authenticated.release_request_hash(),
            evidence_request.binding().clone(),
            evidence_request.policy_identity().clone(),
            evidence_request.binding().wallet(),
            policy.chain_id(),
            100,
            digest(0x88),
            true,
            NOW + 10,
            NOW + 30,
        )
        .unwrap();
        assert_eq!(
            block_on(
                evaluator_with_evidence(1, future)
                    .0
                    .evaluate_at(&request, NOW + 4)
            )
            .unwrap_err(),
            RightsEvaluationErrorV1::EvidenceBinding
        );

        let expired = RightsEvaluationEvidenceV1::new(
            authenticated.operation_hash(),
            authenticated.release_request_hash(),
            evidence_request.binding().clone(),
            evidence_request.policy_identity().clone(),
            evidence_request.binding().wallet(),
            policy.chain_id(),
            100,
            digest(0x88),
            true,
            NOW + 4,
            NOW + 6,
        )
        .unwrap();
        assert_eq!(
            block_on(
                evaluator_with_evidence(1, expired)
                    .0
                    .evaluate_at(&request, NOW + 7)
            )
            .unwrap_err(),
            RightsEvaluationErrorV1::EvidenceBinding
        );
    }

    #[test]
    fn async_rights_evaluator_propagates_source_failure_without_signing() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let calls = Arc::new(AtomicUsize::new(0));
        let evaluator = ProtectedContentRightsEvaluatorV1::new(
            node_signing_key(1),
            FailingEvidenceSource {
                calls: Arc::clone(&calls),
            },
        )
        .unwrap();

        assert_eq!(
            block_on(evaluator.evaluate_at(&request, NOW + 4)).unwrap_err(),
            RightsEvaluationErrorV1::EvidenceSource
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn async_rights_evaluator_can_be_dropped_before_evidence_or_signing() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let (evaluator, calls) = evaluator_with_evidence(1, evidence_for(&request, true));

        let pending_evaluation = evaluator.evaluate_at(&request, NOW + 4);
        drop(pending_evaluation);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn typed_rights_evaluator_is_deterministic_for_exact_replay() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let evidence = evidence_for(&request, true);
        let (evaluator, _) = evaluator_with_evidence(1, evidence);

        let first = evaluator.evaluate_at(&request, NOW + 4);
        let first = block_on(first).unwrap().to_json_vec().unwrap();
        let second = evaluator.evaluate_at(&request, NOW + 4);
        let second = block_on(second).unwrap().to_json_vec().unwrap();
        assert_eq!(first, second);
        assert_no_forbidden_output(&first);
        assert_no_forbidden_output(format!("{evaluator:?}").as_bytes());
    }

    #[test]
    fn validated_request_rejects_signature_mutation_and_wrong_runtime_issuer() {
        let operation = signed_operation(0x42);
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let mut bytes = request.to_json_vec().unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("observed_evidence"));
        assert!(!text.contains("has_access"));
        let pos = bytes
            .iter()
            .rposition(|byte| byte.is_ascii_digit())
            .expect("request contains signature digits");
        bytes[pos] = if bytes[pos] == b'0' { b'1' } else { b'0' };
        assert!(ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &bytes,
            operation.statement().runtime_operation_issuer(),
            NOW + 3,
        )
        .is_err());
        assert!(ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            RuntimeOperationIssuerKeyV1::new(
                SigningKey::from_bytes(&[0x24; 32])
                    .verifying_key()
                    .to_bytes()
            )
            .unwrap(),
            NOW + 3,
        )
        .is_err());
    }

    fn rights_request(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
    ) -> RightsProviderRequestV1 {
        RightsProviderRequestV1::new_evaluate(node_public_key(node_seed), operation).unwrap()
    }

    fn chain_data_for(evidence: &RightsEvaluationEvidenceV1) -> serde_json::Value {
        let bytes = evidence.canonical_bytes().unwrap();
        serde_json::json!({
            "schema": "elastos.chain.protected-content-rights-evidence/v1",
            "chain_id": evidence.observed_chain_id(),
            "finalized_block_number": evidence.finalized_block_number(),
            "finalized_block_hash": format!(
                "0x{}",
                hex::encode(evidence.finalized_block_hash().as_bytes())
            ),
            "rights_evaluation_evidence": format!("0x{}", hex::encode(&bytes)),
            "rights_evaluation_evidence_hash": format!(
                "0x{}",
                hex::encode(evidence.canonical_hash().unwrap().as_bytes())
            ),
        })
    }

    #[test]
    fn chain_rights_request_is_op_and_signed_operation_only() {
        let operation = signed_operation(0x42);
        let request = chain_rights_evidence_request(&operation).unwrap();
        let object = request.as_object().unwrap();
        assert_eq!(object.len(), 2);
        assert_eq!(
            object.get("op").and_then(|value| value.as_str()),
            Some(CHAIN_RIGHTS_EVIDENCE_OP)
        );
        let hex = object
            .get("signed_runtime_release_operation")
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(hex.starts_with("0x"));
        for forbidden in [
            "network",
            "rpc_url",
            "host",
            "ip",
            "port",
            "url",
            "endpoint",
            "has_access",
            "rights_evaluation_evidence",
        ] {
            assert!(!object.contains_key(forbidden));
        }
    }

    #[test]
    fn chain_evidence_parser_rejects_topology_injection_and_missing_evidence() {
        let operation = signed_operation(0x42);
        let request = validated_request(&operation, 1);
        let evidence = evidence_for(&request, true);
        let mut with_network = chain_data_for(&evidence);
        with_network["network"] = serde_json::json!("esc-mainnet");
        assert_eq!(
            parse_chain_rights_evidence_data(&with_network).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut with_rpc = chain_data_for(&evidence);
        with_rpc["rpc_url"] = serde_json::json!("http://127.0.0.1:8545");
        assert_eq!(
            parse_chain_rights_evidence_data(&with_rpc).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut with_host = chain_data_for(&evidence);
        with_host["host"] = serde_json::json!("127.0.0.1");
        assert_eq!(
            parse_chain_rights_evidence_data(&with_host).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut with_has_access = chain_data_for(&evidence);
        with_has_access["has_access"] = serde_json::json!(true);
        assert_eq!(
            parse_chain_rights_evidence_data(&with_has_access).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut with_old_finality_shape = chain_data_for(&evidence);
        with_old_finality_shape["head_block_number"] = serde_json::json!(42);
        assert_eq!(
            parse_chain_rights_evidence_data(&with_old_finality_shape).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut missing = chain_data_for(&evidence);
        missing
            .as_object_mut()
            .unwrap()
            .remove("rights_evaluation_evidence");
        assert_eq!(
            parse_chain_rights_evidence_data(&missing).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let mut hash_mismatch = chain_data_for(&evidence);
        hash_mismatch["rights_evaluation_evidence_hash"] =
            serde_json::json!(format!("0x{}", hex::encode([0xab; 32])));
        assert_eq!(
            parse_chain_rights_evidence_data(&hash_mismatch).unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );

        let bytes = evidence.canonical_bytes().unwrap();
        let minimal = serde_json::json!({
            "schema": "elastos.chain.protected-content-rights-evidence/v1",
            "rights_evaluation_evidence": format!("0x{}", hex::encode(&bytes)),
        });
        assert_eq!(
            parse_chain_rights_evidence_data(&minimal).unwrap(),
            evidence
        );
    }

    #[test]
    fn evaluate_rights_via_chain_signs_allow_and_deny_from_chain_json() {
        let operation = signed_operation(0x42);
        let request = rights_request(&operation, 1);
        let validated = validated_request(&operation, 1);
        let issuer = operation.statement().runtime_operation_issuer();
        let allow_data = chain_data_for(&evidence_for(&validated, true));
        let deny_data = chain_data_for(&evidence_for(&validated, false));

        let allow = block_on(evaluate_rights_via_chain(
            node_signing_key(1),
            issuer,
            &request,
            NOW + 4,
            |chain_request| async move {
                assert_eq!(
                    chain_request.get("op").and_then(|value| value.as_str()),
                    Some(CHAIN_RIGHTS_EVIDENCE_OP)
                );
                Ok(allow_data)
            },
        ))
        .unwrap();
        assert_eq!(allow.status(), RightsProviderResponseStatusV1::Decision);
        assert_eq!(
            allow
                .signed_node_rights_decision()
                .unwrap()
                .statement()
                .decision(),
            RightsDecisionV1::Allowed
        );

        let deny = block_on(evaluate_rights_via_chain(
            node_signing_key(1),
            issuer,
            &request,
            NOW + 4,
            |_| async move { Ok(deny_data) },
        ))
        .unwrap();
        assert_eq!(
            deny.signed_node_rights_decision()
                .unwrap()
                .statement()
                .decision(),
            RightsDecisionV1::Denied
        );
    }

    #[test]
    fn evaluate_rights_via_chain_does_not_invoke_on_invalid_request_or_wrong_node() {
        let operation = signed_operation(0x42);
        let request = rights_request(&operation, 1);
        let calls = Arc::new(AtomicUsize::new(0));

        let wrong_issuer = RuntimeOperationIssuerKeyV1::new(
            SigningKey::from_bytes(&[0x24; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        let issuer_calls = Arc::clone(&calls);
        assert_eq!(
            block_on(evaluate_rights_via_chain(
                node_signing_key(1),
                wrong_issuer,
                &request,
                NOW + 4,
                move |_| {
                    issuer_calls.fetch_add(1, Ordering::SeqCst);
                    async { panic!("chain must not be invoked") }
                },
            ))
            .unwrap_err(),
            RightsEvaluationErrorV1::Contract
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let node_calls = Arc::clone(&calls);
        assert_eq!(
            block_on(evaluate_rights_via_chain(
                node_signing_key(2),
                operation.statement().runtime_operation_issuer(),
                &request,
                NOW + 4,
                move |_| {
                    node_calls.fetch_add(1, Ordering::SeqCst);
                    async { panic!("chain must not be invoked") }
                },
            ))
            .unwrap_err(),
            RightsEvaluationErrorV1::WrongSelectedNode
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn evaluate_rights_via_chain_fails_closed_on_injected_chain_topology() {
        let operation = signed_operation(0x42);
        let request = rights_request(&operation, 1);
        let validated = validated_request(&operation, 1);
        let last_request = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&last_request);
        let mut poisoned = chain_data_for(&evidence_for(&validated, true));
        poisoned["rpc_url"] = serde_json::json!("http://127.0.0.1:8545");
        assert_eq!(
            block_on(evaluate_rights_via_chain(
                node_signing_key(1),
                operation.statement().runtime_operation_issuer(),
                &request,
                NOW + 4,
                move |chain_request| {
                    *captured.lock().unwrap() = Some(chain_request);
                    async move { Ok(poisoned) }
                },
            ))
            .unwrap_err(),
            RightsEvaluationErrorV1::ChainEvidence
        );
        let sent = last_request.lock().unwrap().clone().unwrap();
        assert_eq!(sent.as_object().unwrap().len(), 2);
        assert!(!sent.as_object().unwrap().contains_key("rpc_url"));
    }
}
