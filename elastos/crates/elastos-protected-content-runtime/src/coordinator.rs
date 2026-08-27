use std::collections::{BTreeSet, HashMap};
use std::fmt;

use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, CanonicalContract, ContractError, Digest32,
    NodePublicKey, RightsDecisionV1, RuntimeOperationIssuerKeyV1, SignedNodeContributionV1,
    SignedRuntimeReleaseOperationV1, WalletAddress, WalletSignedRightsRequestV1,
};
use elastos_protected_content_provider_contracts::{
    CustodyProviderRequestV1, CustodyProviderResponseV1, RightsProviderRequestV1,
    RightsProviderResponseV1,
};
use elastos_wallet_contract::{
    ProtectedContentRightsSignatureResultV1, WalletProviderOperationV2, WalletProviderRequestV2,
    WalletProviderResponseV2, WalletResultV2,
};
use thiserror::Error;

use crate::{
    RuntimeReleaseJournal, RuntimeReleaseJournalError, RuntimeReleaseOperationDraft,
    RuntimeReleaseTerminalResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeProviderCallError {
    #[error("provider call did not return an exact result")]
    NoExactResult,
}

pub trait RuntimeRightsProvider {
    fn evaluate_rights(
        &self,
        request: &RightsProviderRequestV1,
    ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError>;
}

pub trait RuntimeCustodyProvider {
    fn release_contribution(
        &self,
        request: &CustodyProviderRequestV1,
    ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError>;
}

pub struct RuntimeSelectedProvider<'a> {
    node_public_key: NodePublicKey,
    rights: &'a dyn RuntimeRightsProvider,
    custody: &'a dyn RuntimeCustodyProvider,
}

impl<'a> RuntimeSelectedProvider<'a> {
    pub fn new(
        node_public_key: NodePublicKey,
        rights: &'a dyn RuntimeRightsProvider,
        custody: &'a dyn RuntimeCustodyProvider,
    ) -> Self {
        Self {
            node_public_key,
            rights,
            custody,
        }
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }
}

impl fmt::Debug for RuntimeSelectedProvider<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSelectedProvider")
            .field("node_public_key", &self.node_public_key)
            .field("rights", &"[redacted]")
            .field("custody", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeReleaseCoordinatorOutcome {
    Terminal(RuntimeReleaseTerminalResult),
    Nonterminal {
        operation_hash: Digest32,
        reason: RuntimeReleaseNonterminalReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeReleaseNonterminalReason {
    ProviderEffectAlreadyStarted,
    ProviderEffectUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeReleaseCoordinatorError {
    #[error("runtime release wallet authority is invalid")]
    WalletAuthority,
    #[error("runtime release operation authority is invalid")]
    OperationAuthority,
    #[error("runtime release provider selection is invalid")]
    ProviderSelection,
    #[error("runtime release provider result is invalid")]
    ProviderResult,
    #[error("runtime release journal failed")]
    Journal,
}

impl From<RuntimeReleaseJournalError> for RuntimeReleaseCoordinatorError {
    fn from(_: RuntimeReleaseJournalError) -> Self {
        Self::Journal
    }
}

impl From<ContractError> for RuntimeReleaseCoordinatorError {
    fn from(_: ContractError) -> Self {
        Self::OperationAuthority
    }
}

pub struct RuntimeReleaseCoordinator<'a> {
    journal: RuntimeReleaseJournal,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    selected_providers: Vec<RuntimeSelectedProvider<'a>>,
}

impl<'a> RuntimeReleaseCoordinator<'a> {
    pub fn new(
        journal: RuntimeReleaseJournal,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        selected_providers: Vec<RuntimeSelectedProvider<'a>>,
    ) -> Result<Self, RuntimeReleaseCoordinatorError> {
        let mut nodes = BTreeSet::new();
        for provider in &selected_providers {
            if !nodes.insert(provider.node_public_key) {
                return Err(RuntimeReleaseCoordinatorError::ProviderSelection);
            }
        }
        Ok(Self {
            journal,
            expected_runtime_issuer,
            selected_providers,
        })
    }

    pub fn release(
        &self,
        wallet_request_bytes: &[u8],
        wallet_response_bytes: &[u8],
        signed_runtime_release_operation: SignedRuntimeReleaseOperationV1,
        now_unix_seconds: u64,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        let wallet_request =
            WalletProviderRequestV2::decode_at(wallet_request_bytes, now_unix_seconds)
                .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
        let wallet_response =
            WalletProviderResponseV2::decode_for_request(wallet_response_bytes, &wallet_request)
                .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
        validate_wallet_release_binding(
            &wallet_request,
            &wallet_response,
            &signed_runtime_release_operation,
        )?;
        let authenticated = signed_runtime_release_operation
            .verify(self.expected_runtime_issuer, now_unix_seconds)
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let draft = RuntimeReleaseOperationDraft::new(
            wallet_request_bytes.to_vec(),
            wallet_response_bytes.to_vec(),
            signed_runtime_release_operation.clone(),
        )?;
        let persisted = self.journal.persist_before_provider_effect(&draft)?;
        if let Some(terminal) = persisted.terminal_result().cloned() {
            return Ok(RuntimeReleaseCoordinatorOutcome::Terminal(terminal));
        }
        if persisted.provider_effect_started() {
            return Ok(RuntimeReleaseCoordinatorOutcome::Nonterminal {
                operation_hash: draft.operation_hash()?,
                reason: RuntimeReleaseNonterminalReason::ProviderEffectAlreadyStarted,
            });
        }
        let ordered_providers = self.selected_ordered_providers(&authenticated)?;
        self.journal.mark_provider_effect_started(&draft)?;
        self.invoke_providers(
            &draft,
            &signed_runtime_release_operation,
            &authenticated,
            &ordered_providers,
            now_unix_seconds,
        )
    }

    fn selected_ordered_providers(
        &'a self,
        authenticated: &AuthenticatedRuntimeReleaseOperationV1,
    ) -> Result<Vec<&'a RuntimeSelectedProvider<'a>>, RuntimeReleaseCoordinatorError> {
        let node_set = authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let mut configured = HashMap::with_capacity(self.selected_providers.len());
        for provider in &self.selected_providers {
            if !node_set.contains(provider.node_public_key) {
                return Err(RuntimeReleaseCoordinatorError::ProviderSelection);
            }
            configured.insert(provider.node_public_key, provider);
        }
        let selected = node_set
            .members()
            .iter()
            .filter_map(|node| configured.get(node).copied())
            .collect::<Vec<_>>();
        if selected.len() < usize::from(node_set.threshold().required()) {
            return Err(RuntimeReleaseCoordinatorError::ProviderSelection);
        }
        Ok(selected)
    }

    fn invoke_providers(
        &self,
        draft: &RuntimeReleaseOperationDraft,
        signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
        authenticated: &AuthenticatedRuntimeReleaseOperationV1,
        ordered_providers: &[&RuntimeSelectedProvider<'_>],
        now_unix_seconds: u64,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        let node_set = authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let threshold = usize::from(node_set.threshold().required());
        let mut decisions = Vec::with_capacity(ordered_providers.len());
        for provider in ordered_providers {
            let request = RightsProviderRequestV1::new_evaluate(
                provider.node_public_key,
                signed_runtime_release_operation,
            )
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
            let response = match provider.rights.evaluate_rights(&request) {
                Ok(response) => response,
                Err(_) => {
                    return self.nonterminal(
                        draft,
                        RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                    );
                }
            };
            if response
                .validate_against_request_at(
                    &request,
                    self.expected_runtime_issuer,
                    now_unix_seconds,
                )
                .is_err()
            {
                return self.nonterminal(
                    draft,
                    RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                );
            }
            let decision = match response.signed_node_rights_decision() {
                Ok(decision) => decision,
                Err(_) => {
                    return self.nonterminal(
                        draft,
                        RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                    );
                }
            };
            let verified = authenticated
                .verify_node_rights_decision(&decision, &node_set, now_unix_seconds)
                .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
            if verified.decision() == RightsDecisionV1::Denied {
                let terminal = RuntimeReleaseTerminalResult::RightsDenied {
                    signed_node_rights_decision: Box::new(decision),
                };
                let persisted = self.journal.mark_terminal(draft, terminal)?;
                return Ok(RuntimeReleaseCoordinatorOutcome::Terminal(
                    persisted.into_terminal_result()?,
                ));
            }
            decisions.push((provider, decision));
            if decisions.len() == threshold {
                break;
            }
        }

        let mut contributions = Vec::with_capacity(threshold);
        for (provider, decision) in decisions {
            let request = CustodyProviderRequestV1::new_release_contribution(
                signed_runtime_release_operation,
                &decision,
            )
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
            let response = match provider.custody.release_contribution(&request) {
                Ok(response) => response,
                Err(_) => {
                    return self.nonterminal(
                        draft,
                        RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                    );
                }
            };
            if response
                .validate_against_request_at(
                    &request,
                    self.expected_runtime_issuer,
                    provider.node_public_key,
                    now_unix_seconds,
                )
                .is_err()
            {
                return self.nonterminal(
                    draft,
                    RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                );
            }
            let contribution = response
                .signed_node_contribution()
                .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
            validate_contribution_identity(
                authenticated,
                &node_set,
                &contribution,
                provider.node_public_key,
                now_unix_seconds,
            )?;
            contributions.push(contribution);
            if contributions.len() == threshold {
                let terminal = RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions: contributions,
                };
                let persisted = self.journal.mark_terminal(draft, terminal)?;
                return Ok(RuntimeReleaseCoordinatorOutcome::Terminal(
                    persisted.into_terminal_result()?,
                ));
            }
        }

        self.nonterminal(
            draft,
            RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
        )
    }

    fn nonterminal(
        &self,
        draft: &RuntimeReleaseOperationDraft,
        reason: RuntimeReleaseNonterminalReason,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        Ok(RuntimeReleaseCoordinatorOutcome::Nonterminal {
            operation_hash: draft.operation_hash()?,
            reason,
        })
    }
}

fn validate_wallet_release_binding(
    wallet_request: &WalletProviderRequestV2,
    wallet_response: &WalletProviderResponseV2,
    signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
) -> Result<(), RuntimeReleaseCoordinatorError> {
    let (account_id, canonical_rights_request_hex) = match &wallet_request.operation {
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id,
            canonical_rights_request_hex,
            ..
        } => (account_id, canonical_rights_request_hex),
        _ => return Err(RuntimeReleaseCoordinatorError::WalletAuthority),
    };
    let signed_rights = signed_runtime_release_operation
        .statement()
        .rights_request();
    let rights_request_bytes = hex::decode(canonical_rights_request_hex)
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    let rights_request =
        elastos_protected_content_contracts::RightsRequestV1::from_canonical_bytes(
            &rights_request_bytes,
        )
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    if &rights_request != signed_rights.request() {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    let result = match &wallet_response.result {
        WalletResultV2::Ok { data } => {
            serde_json::from_value::<ProtectedContentRightsSignatureResultV1>(data.clone())
                .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?
        }
        WalletResultV2::Error { .. } => {
            return Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        }
    };
    result
        .validate()
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    if &result.account_id != account_id {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    if result.signer != wallet_address_hex(signed_rights.request().binding().wallet()) {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    let signed_rights_bytes = hex::decode(&result.wallet_signed_rights_request_hex)
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    let result_signed_rights =
        WalletSignedRightsRequestV1::from_canonical_bytes(&signed_rights_bytes)
            .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    if &result_signed_rights != signed_rights {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    Ok(())
}

fn validate_contribution_identity(
    authenticated: &AuthenticatedRuntimeReleaseOperationV1,
    node_set: &elastos_protected_content_contracts::NodeSetV1,
    contribution: &SignedNodeContributionV1,
    expected_node: NodePublicKey,
    now_unix_seconds: u64,
) -> Result<(), RuntimeReleaseCoordinatorError> {
    let verified = authenticated
        .verify_node_contribution(contribution, node_set, now_unix_seconds)
        .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
    if verified.node_public_key() != expected_node {
        return Err(RuntimeReleaseCoordinatorError::ProviderResult);
    }
    Ok(())
}

fn wallet_address_hex(wallet: WalletAddress) -> String {
    format!("0x{}", hex::encode(wallet.as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_auth::ethereum_signed_message_hash;
    use elastos_protected_content_contracts::{
        CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
        CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1,
        CustodyEpochStatementV1, CustodyNodeIdentityV1, EncryptedContentIdentityV1,
        EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1, HpkeCiphertextV1,
        KeyReleaseRequestV1, NodeContributionStatementV1, NodeCustodyPublicKeyV1,
        RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1, RecipientPublicKeyBytesV1,
        RecipientSealedContributionV1, ReplayNonce16, RightsActionV1,
        RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
        RightsRequestV1, RightsSubjectSourceV1, RuntimeReleaseAuditIdV1,
        RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1, ShareCoordinateV1,
        SignedCustodyEpochV1, SignedNodeRightsDecisionV1, SignedRecipientKeyAuthorizationV1,
        ThresholdV1, WalletAddress, CUSTODY_HPKE_SUITE_ID_V1, HPKE_ENCAPPED_KEY_BYTES,
        HPKE_SEALED_SHARE_BYTES,
    };
    use elastos_protected_content_provider_contracts::{
        CustodyProviderResponseStatusV1, ProviderFailureCodeV1, ValidatedCustodyProviderRequestV1,
        ValidatedRightsProviderRequestV1,
    };
    use elastos_wallet_contract::{
        ProtectedContentRightsSignatureResultV1, VerifiedWalletInvocationContext,
        WalletProviderOperationV2, WalletProviderRequestV2, WalletProviderResponseV2,
        WalletResultV2,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use sha3::{Digest as _, Keccak256};
    use tempfile::tempdir;

    use super::*;

    const NOW: u64 = 2_000_000_000;

    #[derive(Default)]
    struct FakeRightsProvider {
        responses: RefCell<Vec<Result<RightsProviderResponseV1, RuntimeProviderCallError>>>,
        requests: RefCell<Vec<RightsProviderRequestV1>>,
    }

    impl FakeRightsProvider {
        fn new(responses: Vec<Result<RightsProviderResponseV1, RuntimeProviderCallError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.borrow().len()
        }
    }

    impl RuntimeRightsProvider for FakeRightsProvider {
        fn evaluate_rights(
            &self,
            request: &RightsProviderRequestV1,
        ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError> {
            self.requests.borrow_mut().push(request.clone());
            if self.responses.borrow().is_empty() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            self.responses.borrow_mut().remove(0)
        }
    }

    #[derive(Default)]
    struct FakeCustodyProvider {
        responses: RefCell<Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>>,
        requests: RefCell<Vec<CustodyProviderRequestV1>>,
    }

    impl FakeCustodyProvider {
        fn new(
            responses: Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>,
        ) -> Self {
            Self {
                responses: RefCell::new(responses),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.borrow().len()
        }
    }

    impl RuntimeCustodyProvider for FakeCustodyProvider {
        fn release_contribution(
            &self,
            request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            self.requests.borrow_mut().push(request.clone());
            if self.responses.borrow().is_empty() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            self.responses.borrow_mut().remove(0)
        }
    }

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn node_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        NodePublicKey::new(node_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn wallet_key(seed: u8) -> WalletSigningKey {
        WalletSigningKey::from_slice(&[seed; 32]).unwrap()
    }

    fn wallet(seed: u8) -> WalletAddress {
        let key = wallet_key(seed);
        let encoded = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
        let mut bytes = [0u8; 32];
        bytes[0] = seed.max(9);
        RecipientPublicKeyBytesV1::new(bytes).unwrap()
    }

    fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
        recipient_public_key(seed)
            .key_identity(CUSTODY_HPKE_SUITE_ID_V1)
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

    fn signed_custody_epoch() -> SignedCustodyEpochV1 {
        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        let nodes = [1, 2, 3]
            .into_iter()
            .map(|seed| {
                CustodyNodeIdentityV1::new(
                    node_public_key(seed),
                    NodeCustodyPublicKeyV1::new([0x30 + seed; 32]).unwrap(),
                    ShareCoordinateV1::new(seed).unwrap(),
                )
                .unwrap()
            })
            .collect();
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

    fn custody_envelope(seed: u8) -> CustodyEnvelopeV1 {
        let epoch = signed_custody_epoch();
        let manifest = CustodyEnvelopeManifestV1::new(
            EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap(),
            elastos_protected_content_contracts::CustodyPoolIdentityV1::new(
                digest(seed ^ 0x34),
                512,
            )
            .unwrap(),
            epoch.epoch_identity().unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(seed ^ 0x35), 512).unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            digest(seed ^ 0x33),
            epoch.statement().nodes().to_vec(),
        )
        .unwrap();
        let shares = [seed ^ 0x50, seed ^ 0x51, seed ^ 0x52]
            .into_iter()
            .map(|share_seed| {
                let mut encapped_key = [0u8; HPKE_ENCAPPED_KEY_BYTES];
                encapped_key[0] = share_seed.max(9);
                let mut ciphertext = [0u8; HPKE_SEALED_SHARE_BYTES];
                ciphertext.fill(share_seed);
                HpkeCiphertextV1::new(encapped_key, ciphertext).unwrap()
            })
            .collect();
        CustodyEnvelopeV1::new(manifest, shares).unwrap()
    }

    fn runtime_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
        RuntimeOperationIssuerKeyV1::new(runtime_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn signed_runtime_release_operation(seed: u8) -> SignedRuntimeReleaseOperationV1 {
        let runtime_key = runtime_key(seed);
        let envelope = custody_envelope(0x11);
        let policy = policy_body();
        let binding = elastos_protected_content_contracts::ProtectedContentBindingV1::new(
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
        .unwrap();
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
            let (signature, recovery_id) = wallet_key(7)
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

    fn signed_node_rights_decision(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
        decision: RightsDecisionV1,
    ) -> SignedNodeRightsDecisionV1 {
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 3)
            .unwrap();
        let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
            authenticated.release_request_hash(),
            authenticated.rights_request_hash(),
            authenticated.binding().clone(),
            authenticated.action(),
            node_public_key(node_seed),
            decision,
            digest(0x80 ^ node_seed),
            NOW + 4,
            NOW + 35,
        )
        .unwrap();
        SignedNodeRightsDecisionV1::new(
            statement.clone(),
            node_key(node_seed)
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn signed_node_contribution(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
    ) -> SignedNodeContributionV1 {
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 5)
            .unwrap();
        let decision = signed_node_rights_decision(operation, node_seed, RightsDecisionV1::Allowed);
        let sealed = RecipientSealedContributionV1::new(
            authenticated.recipient().clone(),
            vec![node_seed; 96],
        )
        .unwrap();
        let statement = NodeContributionStatementV1::new(
            authenticated.release_request_hash(),
            authenticated.binding().clone(),
            decision,
            sealed,
            NOW + 5,
            NOW + 35,
        )
        .unwrap();
        SignedNodeContributionV1::new(
            statement.clone(),
            node_key(node_seed)
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn wallet_request_response(operation: &SignedRuntimeReleaseOperationV1) -> (Vec<u8>, Vec<u8>) {
        let context = VerifiedWalletInvocationContext::new(
            "profile:alpha",
            "runtime-session:alpha",
            Some("proof:alpha".to_string()),
            "grant:alpha",
            "runtime",
            "launch:alpha",
        )
        .unwrap();
        let request = WalletProviderRequestV2::new(
            &context,
            "wallet-request:11111111111111111111111111111111",
            NOW,
            NOW + 120,
            WalletProviderOperationV2::RequestProtectedContentRightsSignature {
                account_id: "wallet-account-alpha".to_string(),
                canonical_rights_request_hex: hex::encode(
                    operation
                        .statement()
                        .rights_request()
                        .request()
                        .canonical_bytes()
                        .unwrap(),
                ),
                reason: "Open protected content".to_string(),
            },
        )
        .unwrap();
        let result = ProtectedContentRightsSignatureResultV1::new(
            "wallet-account-alpha",
            wallet_address_hex(
                operation
                    .statement()
                    .rights_request()
                    .request()
                    .binding()
                    .wallet(),
            ),
            hex::encode(
                operation
                    .statement()
                    .rights_request()
                    .canonical_bytes()
                    .unwrap(),
            ),
        )
        .unwrap();
        let response = WalletProviderResponseV2::for_request(
            &request,
            WalletResultV2::Ok {
                data: serde_json::to_value(result).unwrap(),
            },
        );
        (
            serde_json::to_vec(&request).unwrap(),
            serde_json::to_vec(&response).unwrap(),
        )
    }

    fn owner_only_root(temp: &tempfile::TempDir) -> PathBuf {
        let parent = temp.path().join("owner-only-parent");
        create_owner_only_directory(&parent);
        parent.join("runtime-release")
    }

    fn create_owner_only_directory(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn coordinator<'a>(
        root: &Path,
        providers: Vec<RuntimeSelectedProvider<'a>>,
    ) -> RuntimeReleaseCoordinator<'a> {
        RuntimeReleaseCoordinator::new(
            RuntimeReleaseJournal::new(root.to_path_buf()),
            runtime_issuer(0x42),
            providers,
        )
        .unwrap()
    }

    fn selected<'a>(
        rights: &'a FakeRightsProvider,
        custody: &'a FakeCustodyProvider,
        node_seed: u8,
    ) -> RuntimeSelectedProvider<'a> {
        RuntimeSelectedProvider::new(node_public_key(node_seed), rights, custody)
    }

    fn provider_triplet(
        operation: &SignedRuntimeReleaseOperationV1,
    ) -> (
        FakeRightsProvider,
        FakeRightsProvider,
        FakeRightsProvider,
        FakeCustodyProvider,
        FakeCustodyProvider,
        FakeCustodyProvider,
    ) {
        (
            FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
                &signed_node_rights_decision(operation, 1, RightsDecisionV1::Allowed),
            )
            .unwrap())]),
            FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
                &signed_node_rights_decision(operation, 2, RightsDecisionV1::Allowed),
            )
            .unwrap())]),
            FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
                &signed_node_rights_decision(operation, 3, RightsDecisionV1::Allowed),
            )
            .unwrap())]),
            FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
                &signed_node_contribution(operation, 1),
            )
            .unwrap())]),
            FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
                &signed_node_contribution(operation, 2),
            )
            .unwrap())]),
            FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
                &signed_node_contribution(operation, 3),
            )
            .unwrap())]),
        )
    }

    #[test]
    fn runtime_coordination_allows_and_collects_threshold_contributions() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let (r1, r2, r3, c1, c2, c3) = provider_triplet(&operation);
        let runtime = coordinator(
            &owner_only_root(&temp),
            vec![
                selected(&r1, &c1, 1),
                selected(&r2, &c2, 2),
                selected(&r3, &c3, 3),
            ],
        );

        let outcome = runtime
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        match outcome {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions,
                },
            ) => assert_eq!(signed_node_contributions.len(), 2),
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(r1.request_count(), 1);
        assert_eq!(r2.request_count(), 1);
        assert_eq!(c1.request_count(), 1);
        assert_eq!(c2.request_count(), 1);
        assert_eq!(r3.request_count(), 0);
        assert_eq!(c3.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_denial_is_terminal_and_skips_custody() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Denied),
        )
        .unwrap())]);
        let r2 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Denied),
        )
        .unwrap())]);
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(
            &owner_only_root(&temp),
            vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)],
        );

        let outcome = runtime
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        match outcome {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::RightsDenied { .. },
            ) => {}
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(r1.request_count() + r2.request_count(), 1);
        assert_eq!(c1.request_count(), 0);
        assert_eq!(c2.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_replays_terminal_without_dispatch() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let (r1, r2, r3, c1, c2, c3) = provider_triplet(&operation);
        let runtime = coordinator(
            &root,
            vec![
                selected(&r1, &c1, 1),
                selected(&r2, &c2, 2),
                selected(&r3, &c3, 3),
            ],
        );
        let first = runtime
            .release(
                &wallet_request,
                &wallet_response,
                operation.clone(),
                NOW + 6,
            )
            .unwrap();
        let r1_replay = FakeRightsProvider::default();
        let r2_replay = FakeRightsProvider::default();
        let c1_replay = FakeCustodyProvider::default();
        let c2_replay = FakeCustodyProvider::default();
        let runtime_replay = coordinator(
            &root,
            vec![
                selected(&r1_replay, &c1_replay, 1),
                selected(&r2_replay, &c2_replay, 2),
            ],
        );

        let replay = runtime_replay
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(r1_replay.request_count(), 0);
        assert_eq!(r2_replay.request_count(), 0);
        assert_eq!(c1_replay.request_count(), 0);
        assert_eq!(c2_replay.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_nonterminal_replay_does_not_redispatch() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::new(vec![Err(RuntimeProviderCallError::NoExactResult)]);
        let r2 = FakeRightsProvider::new(vec![Err(RuntimeProviderCallError::NoExactResult)]);
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);

        let first = runtime
            .release(
                &wallet_request,
                &wallet_response,
                operation.clone(),
                NOW + 6,
            )
            .unwrap();
        assert!(matches!(
            first,
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                ..
            }
        ));
        assert_eq!(r1.request_count() + r2.request_count(), 1);

        let r1_replay = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Denied),
        )
        .unwrap())]);
        let r2_replay = FakeRightsProvider::default();
        let c1_replay = FakeCustodyProvider::default();
        let c2_replay = FakeCustodyProvider::default();
        let runtime_replay = coordinator(
            &root,
            vec![
                selected(&r1_replay, &c1_replay, 1),
                selected(&r2_replay, &c2_replay, 2),
            ],
        );
        let replay = runtime_replay
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        assert!(matches!(
            replay,
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                reason: RuntimeReleaseNonterminalReason::ProviderEffectAlreadyStarted,
                ..
            }
        ));
        assert_eq!(r1_replay.request_count(), 0);
        assert_eq!(r2_replay.request_count(), 0);
        assert_eq!(c1_replay.request_count(), 0);
        assert_eq!(c2_replay.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_rejects_wallet_operation_and_result_substitution() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let mut request: WalletProviderRequestV2 = serde_json::from_slice(&wallet_request).unwrap();
        if let WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            canonical_rights_request_hex,
            ..
        } = &mut request.operation
        {
            *canonical_rights_request_hex = hex::encode(
                signed_runtime_release_operation(0x43)
                    .statement()
                    .rights_request()
                    .request()
                    .canonical_bytes()
                    .unwrap(),
            );
        }
        request.request_sha256 = "0x".to_string();
        let other_wallet_request = serde_json::to_vec(&request).unwrap();
        let mut response: WalletProviderResponseV2 =
            serde_json::from_slice(&wallet_response).unwrap();
        response.request_id = "wallet-request:22222222222222222222222222222222".to_string();
        let other_wallet_response = serde_json::to_vec(&response).unwrap();
        let r1 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let runtime = coordinator(&owner_only_root(&temp), vec![selected(&r1, &c1, 1)]);

        assert_eq!(
            runtime.release(
                &other_wallet_request,
                &wallet_response,
                operation.clone(),
                NOW + 6
            ),
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(
            runtime.release(&wallet_request, &other_wallet_response, operation, NOW + 6),
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_rejects_wrong_node_set_and_threshold_selection() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let wrong_node_runtime = coordinator(
            &owner_only_root(&temp),
            vec![RuntimeSelectedProvider::new(node_public_key(4), &r1, &c1)],
        );
        assert_eq!(
            wrong_node_runtime.release(
                &wallet_request,
                &wallet_response,
                operation.clone(),
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::ProviderSelection)
        );
        let threshold_runtime = coordinator(&owner_only_root(&temp), vec![selected(&r1, &c1, 1)]);
        assert_eq!(
            threshold_runtime.release(&wallet_request, &wallet_response, operation, NOW + 6),
            Err(RuntimeReleaseCoordinatorError::ProviderSelection)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_wrong_provider_result_stays_nonterminal() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let r2 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(
            &owner_only_root(&temp),
            vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)],
        );

        let outcome = runtime
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        assert!(matches!(
            outcome,
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                ..
            }
        ));
        assert_eq!(r1.request_count() + r2.request_count(), 1);
        assert_eq!(c1.request_count(), 0);
        assert_eq!(c2.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_rejects_mismatched_contribution_identity() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let r2 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let c1 = FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
            &signed_node_contribution(&operation, 3),
        )
        .unwrap())]);
        let c2 = FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
            &signed_node_contribution(&operation, 2),
        )
        .unwrap())]);
        let runtime = coordinator(
            &owner_only_root(&temp),
            vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)],
        );

        let outcome = runtime
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .unwrap();
        assert!(matches!(
            outcome,
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                ..
            }
        ));
        assert_eq!(c1.request_count(), 1);
        assert_eq!(c1.request_count() + c2.request_count(), 2);
    }

    #[test]
    fn runtime_coordination_uses_typed_requests_without_secret_or_topology_fields() {
        let operation = signed_runtime_release_operation(0x42);
        let rights_request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let decision = signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let custody_request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        let surface = format!(
            "{}{}{:?}{:?}",
            rights_request.to_json_vec().unwrap().escape_ascii(),
            custody_request.to_json_vec().unwrap().escape_ascii(),
            RuntimeSelectedProvider::new(
                node_public_key(1),
                &FakeRightsProvider::default(),
                &FakeCustodyProvider::default(),
            ),
            RuntimeReleaseCoordinatorError::ProviderResult
        );
        for forbidden in [
            "CEK",
            "raw_share",
            "share_bytes",
            "provider_route",
            "endpoint",
            "host",
            "ip",
            "port",
            "Carrier",
            "http://",
            "https://",
        ] {
            assert!(
                !surface.contains(forbidden),
                "unexpected forbidden marker {forbidden}"
            );
        }

        let validated = ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &rights_request.to_json_vec().unwrap(),
            runtime_issuer(0x42),
            NOW + 6,
        )
        .unwrap();
        assert_eq!(validated.selected_node_public_key(), node_public_key(1));

        let validated_custody = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &custody_request.to_json_vec().unwrap(),
            runtime_issuer(0x42),
            node_public_key(1),
            NOW + 6,
        )
        .unwrap();
        assert_eq!(
            validated_custody
                .release_contribution()
                .unwrap()
                .selected_node_public_key(),
            node_public_key(1)
        );
        assert_eq!(
            CustodyProviderResponseV1::new_failure(
                validated_custody.release_contribution().unwrap(),
                ProviderFailureCodeV1::BackendUnavailable,
            )
            .unwrap()
            .status(),
            CustodyProviderResponseStatusV1::Failure
        );
    }

    #[test]
    fn runtime_coordination_wallet_error_result_is_not_authority() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, _) = wallet_request_response(&operation);
        let request: WalletProviderRequestV2 = serde_json::from_slice(&wallet_request).unwrap();
        let response = WalletProviderResponseV2::for_request(
            &request,
            WalletResultV2::Error {
                code: "denied".to_string(),
                message: "user denied".to_string(),
            },
        );
        let r1 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let runtime = coordinator(&owner_only_root(&temp), vec![selected(&r1, &c1, 1)]);

        assert_eq!(
            runtime.release(
                &wallet_request,
                &serde_json::to_vec(&response).unwrap(),
                operation,
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[test]
    fn runtime_coordination_rejects_operation_substitution_before_dispatch() {
        let temp = tempdir().unwrap();
        let operation = signed_runtime_release_operation(0x42);
        let other = signed_runtime_release_operation(0x43);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::default();
        let r2 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(
            &owner_only_root(&temp),
            vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)],
        );

        assert_eq!(
            runtime.release(&wallet_request, &wallet_response, other, NOW + 6),
            Err(RuntimeReleaseCoordinatorError::OperationAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }
}
