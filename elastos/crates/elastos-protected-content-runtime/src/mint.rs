//! Runtime-owned mint coordinator for one media flow.
//!
//! Provisions one sealed share per selected custody node against the existing
//! v1 provisioning contracts. Journal records stay identity-only. Partial
//! provision is a durable terminal abort. Successful custody provisioning is
//! not content availability or a product listing, and this does not replace
//! live `key`/`rights`.

use std::collections::BTreeSet;
use std::fmt;

use elastos_protected_content_contracts::{
    validate_custody_epoch_against_pool_at, CanonicalContract, ContractError,
    CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1,
    CustodyNodeProvisioningRecordV1, Digest32, NodeCustodyPublicKeyV1, NodePublicKey,
    RuntimeCustodyProvisioningIdV1, RuntimeCustodyProvisioningStatementV1,
    RuntimeOperationIssuerKeyV1, SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1,
    SignedCustodyPoolV1, SignedRuntimeCustodyProvisioningV1,
    MAX_RUNTIME_CUSTODY_PROVISIONING_LIFETIME_SECS, PQ_HYBRID_SEALED_SHARE_MIN_BYTES,
};
use elastos_protected_content_provider_contracts::CustodyProviderRequestV1;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    PersistedRuntimeMint, RuntimeContentAvailabilityRequirement, RuntimeCustodyProvider,
    RuntimeCustodyTerminalKind, RuntimeMintDraft, RuntimeMintJournal, RuntimeMintJournalError,
    RuntimeMintNodeBinding, RuntimeMintNodeReceipt, RuntimeProviderCallError,
    RuntimeVerifiedContentAvailability,
};

const PROVISIONING_ID_DOMAIN: &[u8] =
    b"elastos.protected-content.runtime-mint-node-provisioning-id/v1";
const REQUIRED_NODES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeMintCoordinatorError {
    #[error("runtime mint provider selection is invalid")]
    ProviderSelection,
    #[error("runtime mint provider result is invalid")]
    ProviderResult,
    #[error("runtime mint operation authority is invalid")]
    OperationAuthority,
    #[error("runtime mint journal failed")]
    Journal,
    #[error("runtime mint record conflicts with existing authority")]
    Conflict,
}

impl From<RuntimeMintJournalError> for RuntimeMintCoordinatorError {
    fn from(error: RuntimeMintJournalError) -> Self {
        match error {
            RuntimeMintJournalError::InvalidSelection => Self::ProviderSelection,
            RuntimeMintJournalError::Conflict => Self::Conflict,
            RuntimeMintJournalError::Unavailable
            | RuntimeMintJournalError::Corrupt
            | RuntimeMintJournalError::NotFound => Self::Journal,
        }
    }
}

impl From<ContractError> for RuntimeMintCoordinatorError {
    fn from(_: ContractError) -> Self {
        Self::OperationAuthority
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMintNonterminalReason {
    ProviderEffectAlreadyStarted,
}

#[derive(Clone, PartialEq, Eq)]
pub enum RuntimeMintCoordinatorOutcome {
    CustodyProvisioned {
        mint_id: Digest32,
    },
    ContentAvailable {
        mint_id: Digest32,
    },
    AbortedPartialProvision {
        mint_id: Digest32,
        accepted_orphan_count: usize,
    },
    Nonterminal {
        mint_id: Digest32,
        reason: RuntimeMintNonterminalReason,
    },
}

impl fmt::Debug for RuntimeMintCoordinatorOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CustodyProvisioned { mint_id } => formatter
                .debug_struct("CustodyProvisioned")
                .field("mint_id", mint_id)
                .finish(),
            Self::ContentAvailable { mint_id } => formatter
                .debug_struct("ContentAvailable")
                .field("mint_id", mint_id)
                .finish(),
            Self::AbortedPartialProvision {
                mint_id,
                accepted_orphan_count,
            } => formatter
                .debug_struct("AbortedPartialProvision")
                .field("mint_id", mint_id)
                .field("accepted_orphan_count", accepted_orphan_count)
                .finish(),
            Self::Nonterminal { mint_id, reason } => formatter
                .debug_struct("Nonterminal")
                .field("mint_id", mint_id)
                .field("reason", reason)
                .finish(),
        }
    }
}

pub struct RuntimeMintSelectedNode<'a> {
    binding: RuntimeMintNodeBinding,
    custody: &'a dyn RuntimeCustodyProvider,
}

/// Runtime-owned custody provider selection data.
///
/// This contains only the configured provider identity and its opaque
/// owner-state-root commitment. Pool operator and failure-domain claims are
/// deliberately absent: they come only from the verified signed pool.
pub struct RuntimeMintConfiguredCustodyProvider<'a> {
    node_public_key: NodePublicKey,
    custody_public_key: NodeCustodyPublicKeyV1,
    owner_state_root: Digest32,
    custody: &'a dyn RuntimeCustodyProvider,
}

impl<'a> RuntimeMintConfiguredCustodyProvider<'a> {
    pub fn new(
        node_public_key: NodePublicKey,
        custody_public_key: NodeCustodyPublicKeyV1,
        owner_state_root: Digest32,
        custody: &'a dyn RuntimeCustodyProvider,
    ) -> Result<Self, RuntimeMintCoordinatorError> {
        if owner_state_root == Digest32::new([0; 32]) {
            return Err(RuntimeMintCoordinatorError::ProviderSelection);
        }
        Ok(Self {
            node_public_key,
            custody_public_key,
            owner_state_root,
            custody,
        })
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn custody_public_key(&self) -> NodeCustodyPublicKeyV1 {
        self.custody_public_key
    }

    pub const fn owner_state_root(&self) -> Digest32 {
        self.owner_state_root
    }
}

impl fmt::Debug for RuntimeMintConfiguredCustodyProvider<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintConfiguredCustodyProvider")
            .field("node_public_key", &self.node_public_key)
            .field("custody_public_key", &self.custody_public_key)
            .field("owner_state_root", &self.owner_state_root)
            .field("custody", &"[redacted]")
            .finish()
    }
}

/// Resolve exactly the signed two-of-three custody committee to Runtime-owned
/// configured providers, in the epoch's canonical node order.
pub fn resolve_runtime_mint_selected_nodes<'a>(
    expected_policy_authority: CustodyEpochIssuerKeyV1,
    expected_authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    signed_pool: &SignedCustodyPoolV1,
    signed_epoch: &SignedCustodyEpochV1,
    signed_committee_authorization: &SignedCustodyCommitteeAuthorizationV1,
    now_unix_seconds: u64,
    configured: &'a [RuntimeMintConfiguredCustodyProvider<'a>],
) -> Result<Vec<RuntimeMintSelectedNode<'a>>, RuntimeMintCoordinatorError> {
    let validated = validate_custody_epoch_against_pool_at(
        expected_policy_authority,
        expected_authorization_identity,
        signed_pool,
        signed_epoch,
        signed_committee_authorization,
        now_unix_seconds,
    )
    .map_err(|_| RuntimeMintCoordinatorError::ProviderSelection)?;
    let verified_pool = signed_pool
        .verify(expected_policy_authority)
        .map_err(|_| RuntimeMintCoordinatorError::ProviderSelection)?;
    let committee = validated.committee().nodes();
    if committee.len() != REQUIRED_NODES || configured.len() != REQUIRED_NODES {
        return Err(RuntimeMintCoordinatorError::ProviderSelection);
    }

    let mut configured_nodes = BTreeSet::new();
    let mut configured_custody_keys = BTreeSet::new();
    let mut configured_roots = BTreeSet::new();
    for (index, candidate) in configured.iter().enumerate() {
        if !configured_nodes.insert(candidate.node_public_key)
            || !configured_custody_keys.insert(candidate.custody_public_key.as_bytes())
            || !configured_roots.insert(candidate.owner_state_root)
            || configured[..index]
                .iter()
                .any(|previous| std::ptr::eq(previous.custody, candidate.custody))
        {
            return Err(RuntimeMintCoordinatorError::ProviderSelection);
        }
    }

    committee
        .iter()
        .map(|node| {
            let candidate = configured
                .iter()
                .find(|candidate| candidate.node_public_key == node.node_public_key())
                .ok_or(RuntimeMintCoordinatorError::ProviderSelection)?;
            if candidate.custody_public_key != node.custody_public_key() {
                return Err(RuntimeMintCoordinatorError::ProviderSelection);
            }
            let member = verified_pool
                .member(node.node_public_key())
                .ok_or(RuntimeMintCoordinatorError::ProviderSelection)?;
            if member.custody_public_key() != node.custody_public_key() {
                return Err(RuntimeMintCoordinatorError::ProviderSelection);
            }
            let binding = RuntimeMintNodeBinding::new(
                node.node_public_key(),
                member.operator_id(),
                member.failure_domain_id(),
                candidate.owner_state_root,
            )
            .map_err(|_| RuntimeMintCoordinatorError::ProviderSelection)?;
            Ok(RuntimeMintSelectedNode::new(binding, candidate.custody))
        })
        .collect()
}

impl<'a> RuntimeMintSelectedNode<'a> {
    pub fn new(binding: RuntimeMintNodeBinding, custody: &'a dyn RuntimeCustodyProvider) -> Self {
        Self { binding, custody }
    }

    pub fn binding(&self) -> &RuntimeMintNodeBinding {
        &self.binding
    }
}

impl fmt::Debug for RuntimeMintSelectedNode<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintSelectedNode")
            .field("binding", &self.binding)
            .field("custody", &"[redacted]")
            .finish()
    }
}

pub struct RuntimeMintCoordinator<'a> {
    journal: RuntimeMintJournal,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    sign_statement: fn(&[u8]) -> [u8; 64],
    selected: Vec<RuntimeMintSelectedNode<'a>>,
}

impl fmt::Debug for RuntimeMintCoordinator<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintCoordinator")
            .field("journal", &self.journal)
            .field("expected_runtime_issuer", &self.expected_runtime_issuer)
            .field("sign_statement", &"[redacted]")
            .field("selected", &self.selected)
            .finish()
    }
}

impl<'a> RuntimeMintCoordinator<'a> {
    pub fn new(
        journal: RuntimeMintJournal,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        sign_statement: fn(&[u8]) -> [u8; 64],
        selected: Vec<RuntimeMintSelectedNode<'a>>,
    ) -> Result<Self, RuntimeMintCoordinatorError> {
        if selected.len() != REQUIRED_NODES {
            return Err(RuntimeMintCoordinatorError::ProviderSelection);
        }
        let mut keys = BTreeSet::new();
        let mut operators = BTreeSet::new();
        let mut domains = BTreeSet::new();
        let mut roots = BTreeSet::new();
        for node in &selected {
            if !keys.insert(node.binding.node_public_key())
                || !operators.insert(node.binding.operator_id())
                || !domains.insert(node.binding.failure_domain_id())
                || !roots.insert(node.binding.owner_state_root())
            {
                return Err(RuntimeMintCoordinatorError::ProviderSelection);
            }
        }
        Ok(Self {
            journal,
            expected_runtime_issuer,
            sign_statement,
            selected,
        })
    }

    pub async fn provision(
        &self,
        draft: &RuntimeMintDraft,
        envelope: &CustodyEnvelopeV1,
        now_unix_seconds: u64,
    ) -> Result<RuntimeMintCoordinatorOutcome, RuntimeMintCoordinatorError> {
        self.require_selected_match_draft(draft)?;
        match self.journal.load(draft.mint_id()) {
            Ok(existing) => {
                if let Some(outcome) = self.outcome_from_persisted(&existing)? {
                    return Ok(outcome);
                }
            }
            Err(RuntimeMintJournalError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        validate_pq_hybrid_envelope(draft, envelope)?;
        let persisted = self.journal.persist_bound(draft)?;
        if let Some(outcome) = self.outcome_from_persisted(&persisted)? {
            return Ok(outcome);
        }
        for node in draft.nodes() {
            let selected = self.selected_for(node.node_public_key())?;
            self.journal
                .mark_node_effect_started(draft.mint_id(), node.node_public_key())?;
            let request = signed_provision_request(
                draft,
                envelope,
                node,
                self.expected_runtime_issuer,
                self.sign_statement,
                now_unix_seconds,
            )?;
            match selected.custody.provision_node_share(&request).await {
                Ok(response) => {
                    response
                        .validate_against_request_at(
                            &request,
                            self.expected_runtime_issuer,
                            node.node_public_key(),
                            now_unix_seconds,
                        )
                        .map_err(|_| RuntimeMintCoordinatorError::ProviderResult)?;
                    let receipt = RuntimeMintNodeReceipt::new(
                        node.node_public_key(),
                        response
                            .provisioned_id()
                            .map_err(|_| RuntimeMintCoordinatorError::ProviderResult)?,
                        response
                            .provisioned_record_identity()
                            .map_err(|_| RuntimeMintCoordinatorError::ProviderResult)?,
                        node.owner_state_root(),
                    )?;
                    self.journal.mark_node_receipt(draft.mint_id(), receipt)?;
                }
                Err(RuntimeProviderCallError::NoExactResult) => {
                    let aborted = self
                        .journal
                        .mark_aborted_partial_provision(draft.mint_id())?;
                    return Ok(abort_outcome(&aborted));
                }
            }
        }
        let provisioned = self.journal.mark_custody_provisioned(draft.mint_id())?;
        Ok(RuntimeMintCoordinatorOutcome::CustodyProvisioned {
            mint_id: provisioned.draft().mint_id(),
        })
    }

    pub fn record_content_availability(
        &self,
        draft: &RuntimeMintDraft,
        requirement: &RuntimeContentAvailabilityRequirement,
        evidence: RuntimeVerifiedContentAvailability,
    ) -> Result<RuntimeMintCoordinatorOutcome, RuntimeMintCoordinatorError> {
        self.require_selected_match_draft(draft)?;
        let available =
            self.journal
                .mark_content_available(draft.mint_id(), requirement, evidence)?;
        Ok(RuntimeMintCoordinatorOutcome::ContentAvailable {
            mint_id: available.draft().mint_id(),
        })
    }
}

impl RuntimeMintCoordinator<'_> {
    fn require_selected_match_draft(
        &self,
        draft: &RuntimeMintDraft,
    ) -> Result<(), RuntimeMintCoordinatorError> {
        if draft.nodes().len() != REQUIRED_NODES || self.selected.len() != REQUIRED_NODES {
            return Err(RuntimeMintCoordinatorError::ProviderSelection);
        }
        for node in draft.nodes() {
            let selected = self.selected_for(node.node_public_key())?;
            if selected.binding != *node {
                return Err(RuntimeMintCoordinatorError::ProviderSelection);
            }
        }
        Ok(())
    }

    fn selected_for(
        &self,
        node_public_key: NodePublicKey,
    ) -> Result<&RuntimeMintSelectedNode<'_>, RuntimeMintCoordinatorError> {
        self.selected
            .iter()
            .find(|node| node.binding.node_public_key() == node_public_key)
            .ok_or(RuntimeMintCoordinatorError::ProviderSelection)
    }

    fn outcome_from_persisted(
        &self,
        persisted: &PersistedRuntimeMint,
    ) -> Result<Option<RuntimeMintCoordinatorOutcome>, RuntimeMintCoordinatorError> {
        match persisted.custody_terminal() {
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned) => {
                Ok(Some(if persisted.content_availability().is_some() {
                    RuntimeMintCoordinatorOutcome::ContentAvailable {
                        mint_id: persisted.draft().mint_id(),
                    }
                } else {
                    RuntimeMintCoordinatorOutcome::CustodyProvisioned {
                        mint_id: persisted.draft().mint_id(),
                    }
                }))
            }
            Some(RuntimeCustodyTerminalKind::AbortedPartialProvision) => {
                Ok(Some(abort_outcome(persisted)))
            }
            None if persisted.any_effect_started() => {
                Ok(Some(RuntimeMintCoordinatorOutcome::Nonterminal {
                    mint_id: persisted.draft().mint_id(),
                    reason: RuntimeMintNonterminalReason::ProviderEffectAlreadyStarted,
                }))
            }
            None => Ok(None),
        }
    }
}

fn abort_outcome(persisted: &PersistedRuntimeMint) -> RuntimeMintCoordinatorOutcome {
    RuntimeMintCoordinatorOutcome::AbortedPartialProvision {
        mint_id: persisted.draft().mint_id(),
        accepted_orphan_count: persisted.accepted_orphans().len(),
    }
}

fn validate_pq_hybrid_envelope(
    draft: &RuntimeMintDraft,
    envelope: &CustodyEnvelopeV1,
) -> Result<(), RuntimeMintCoordinatorError> {
    let identity = envelope
        .key_envelope_identity()
        .map_err(|_| RuntimeMintCoordinatorError::ProviderSelection)?;
    if identity != *draft.key_envelope() {
        return Err(RuntimeMintCoordinatorError::ProviderSelection);
    }
    if envelope.manifest().encrypted_content() != draft.encrypted_content()
        || envelope.manifest().threshold() != draft.threshold()
        || envelope.manifest().content_key_commitment() != draft.content_key_commitment()
    {
        return Err(RuntimeMintCoordinatorError::ProviderSelection);
    }
    for node in draft.nodes() {
        let share = envelope
            .stored_share_for_node(node.node_public_key())
            .ok_or(RuntimeMintCoordinatorError::ProviderSelection)?;
        if share.envelope().len() < PQ_HYBRID_SEALED_SHARE_MIN_BYTES {
            return Err(RuntimeMintCoordinatorError::ProviderSelection);
        }
    }
    Ok(())
}

fn signed_provision_request(
    draft: &RuntimeMintDraft,
    envelope: &CustodyEnvelopeV1,
    node: &RuntimeMintNodeBinding,
    issuer: RuntimeOperationIssuerKeyV1,
    sign_statement: fn(&[u8]) -> [u8; 64],
    now_unix_seconds: u64,
) -> Result<CustodyProviderRequestV1, RuntimeMintCoordinatorError> {
    let sealed_share = envelope
        .stored_share_for_node(node.node_public_key())
        .ok_or(RuntimeMintCoordinatorError::ProviderSelection)?
        .clone();
    let record = CustodyNodeProvisioningRecordV1::new(
        draft.key_envelope().clone(),
        envelope.manifest().clone(),
        node.node_public_key(),
        sealed_share,
    )?;
    let statement = RuntimeCustodyProvisioningStatementV1::new(
        issuer,
        record.record_identity()?,
        mint_node_provisioning_id(draft.mint_id(), node.node_public_key())?,
        now_unix_seconds,
        now_unix_seconds
            .checked_add(MAX_RUNTIME_CUSTODY_PROVISIONING_LIFETIME_SECS)
            .ok_or(RuntimeMintCoordinatorError::OperationAuthority)?,
    )?;
    let signature = sign_statement(
        &statement
            .canonical_bytes()
            .map_err(|_| RuntimeMintCoordinatorError::OperationAuthority)?,
    );
    let signed = SignedRuntimeCustodyProvisioningV1::new(statement, signature.to_vec())?;
    CustodyProviderRequestV1::new_provision_node_share(&record, &signed)
        .map_err(|_| RuntimeMintCoordinatorError::OperationAuthority)
}

fn mint_node_provisioning_id(
    mint_id: Digest32,
    node_public_key: NodePublicKey,
) -> Result<RuntimeCustodyProvisioningIdV1, RuntimeMintCoordinatorError> {
    let mut hasher = Sha256::new();
    hasher.update(PROVISIONING_ID_DOMAIN);
    hasher.update(mint_id.as_bytes());
    hasher.update(node_public_key.as_bytes());
    RuntimeCustodyProvisioningIdV1::new(Digest32::new(hasher.finalize().into()))
        .map_err(|_| RuntimeMintCoordinatorError::OperationAuthority)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_protected_content_contracts::{
        CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
        CustodyCommitteeAuthorizationStatementV1, CustodyEnvelopeManifestV1, CustodyEnvelopeV1,
        CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
        CustodyNodeIdentityV1, CustodyPoolFailureDomainIdV1, CustodyPoolIdentityV1,
        CustodyPoolMemberStateV1, CustodyPoolMemberV1, CustodyPoolOperatorIdV1,
        CustodyPoolStatementV1, Digest32, KeyEnvelopeIdentityV1, NodeCustodyPublicKeyV1,
        NodePublicKey, PqHybridSealedShareV1, RightsPolicyIdentityV1, RuntimeOperationIssuerKeyV1,
        ShareCoordinateV1, SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1,
        SignedCustodyPoolV1, ThresholdV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES, X_WING_DRAFT06_CIPHERTEXT_BYTES,
    };
    use elastos_protected_content_provider_contracts::{
        CencFmp4MediaIdentityV1, CustodyProviderRequestV1, CustodyProviderResponseV1,
        ValidatedCustodyProviderRequestV1,
    };
    use tempfile::tempdir;
    use x_wing::kem::{Decapsulator as _, KeyExport as _};
    use x_wing::TryKeyInit as _;

    use super::*;
    use crate::test_media;
    use crate::RuntimeMintJournal;

    const NOW: u64 = 2_000_000_000;
    const RUNTIME_SEED: u8 = 0x71;
    const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
    const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

    struct FakeMintCustody {
        expected_issuer: RuntimeOperationIssuerKeyV1,
        node: NodePublicKey,
        fail: bool,
        now: u64,
        journal_root: PathBuf,
        mint_id: Digest32,
        requests: Mutex<Vec<CustodyProviderRequestV1>>,
    }

    impl FakeMintCustody {
        fn new(node: NodePublicKey, fail: bool, journal_root: PathBuf, mint_id: Digest32) -> Self {
            Self {
                expected_issuer: runtime_issuer(),
                node,
                fail,
                now: NOW + 10,
                journal_root,
                mint_id,
                requests: Mutex::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeCustodyProvider for FakeMintCustody {
        async fn release_contribution(
            &self,
            _request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            Err(RuntimeProviderCallError::NoExactResult)
        }

        async fn provision_node_share(
            &self,
            request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            let record_path = self.journal_root.join(hex::encode(self.mint_id.as_bytes()));
            if !record_path.exists() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            self.requests.lock().unwrap().push(request.clone());
            if self.fail {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            let validated = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
                &bytes,
                self.expected_issuer,
                self.node,
                self.now,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            let provision = validated
                .provision_node_share()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            CustodyProviderResponseV1::new_provisioned(provision)
                .map_err(|_| RuntimeProviderCallError::NoExactResult)
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

    fn runtime_key() -> SigningKey {
        SigningKey::from_bytes(&[RUNTIME_SEED; 32])
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

    fn runtime_issuer() -> RuntimeOperationIssuerKeyV1 {
        RuntimeOperationIssuerKeyV1::new(runtime_key().verifying_key().to_bytes()).unwrap()
    }

    fn sign_statement(bytes: &[u8]) -> [u8; 64] {
        runtime_key().sign(bytes).to_bytes()
    }

    fn binding(seed: u8) -> RuntimeMintNodeBinding {
        RuntimeMintNodeBinding::new(
            node_public_key(seed),
            CustodyPoolOperatorIdV1::new([0x80 + seed; 32]),
            CustodyPoolFailureDomainIdV1::new([0x90 + seed; 32]),
            digest(0xa0 + seed),
        )
        .unwrap()
    }

    fn selection_policy() -> (
        CustodyEpochIssuerKeyV1,
        CustodyCommitteeAuthorizationIdentityV1,
        SignedCustodyPoolV1,
        SignedCustodyEpochV1,
        SignedCustodyCommitteeAuthorizationV1,
    ) {
        let policy_key = SigningKey::from_bytes(&[0x74; 32]);
        let issuer = CustodyEpochIssuerKeyV1::new(policy_key.verifying_key().to_bytes()).unwrap();
        let suites = CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap();
        let pool_statement = CustodyPoolStatementV1::new(
            issuer,
            [
                (1, 0x31, 0xa1, 0xb1),
                (2, 0x32, 0xa2, 0xb2),
                (3, 0x33, 0xa3, 0xb3),
            ]
            .into_iter()
            .map(|(node, custody, operator, domain)| {
                CustodyPoolMemberV1::new(
                    node_public_key(node),
                    node_custody_public_key(custody),
                    CustodyPoolOperatorIdV1::new([operator; 32]),
                    CustodyPoolFailureDomainIdV1::new([domain; 32]),
                    suites.clone(),
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                )
                .unwrap()
            })
            .collect(),
        )
        .unwrap();
        let pool = SignedCustodyPoolV1::new(
            pool_statement.clone(),
            policy_key
                .sign(&pool_statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let epoch_statement = CustodyEpochStatementV1::new(
            issuer,
            suites,
            ThresholdV1::new(2, 3).unwrap(),
            vec![
                CustodyNodeIdentityV1::new(
                    node_public_key(1),
                    node_custody_public_key(0x31),
                    ShareCoordinateV1::new(1).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(2),
                    node_custody_public_key(0x32),
                    ShareCoordinateV1::new(2).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(3),
                    node_custody_public_key(0x33),
                    ShareCoordinateV1::new(3).unwrap(),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let epoch = SignedCustodyEpochV1::new(
            epoch_statement.clone(),
            policy_key
                .sign(&epoch_statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let authorization_statement = CustodyCommitteeAuthorizationStatementV1::new(
            issuer,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        )
        .unwrap();
        let authorization = SignedCustodyCommitteeAuthorizationV1::new(
            authorization_statement.clone(),
            policy_key
                .sign(&authorization_statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let authorization_identity = authorization.authorization_identity().unwrap();
        (issuer, authorization_identity, pool, epoch, authorization)
    }

    fn configured_provider<'a>(
        node_seed: u8,
        custody_seed: u8,
        root: u8,
        custody: &'a dyn RuntimeCustodyProvider,
    ) -> RuntimeMintConfiguredCustodyProvider<'a> {
        RuntimeMintConfiguredCustodyProvider::new(
            node_public_key(node_seed),
            node_custody_public_key(custody_seed),
            digest(root),
            custody,
        )
        .unwrap()
    }

    fn envelope() -> CustodyEnvelopeV1 {
        let media_identity = media_identity();
        let nodes = vec![
            CustodyNodeIdentityV1::new(
                node_public_key(1),
                node_custody_public_key(0x31),
                ShareCoordinateV1::new(1).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(2),
                node_custody_public_key(0x32),
                ShareCoordinateV1::new(2).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(3),
                node_custody_public_key(0x33),
                ShareCoordinateV1::new(3).unwrap(),
            )
            .unwrap(),
        ];
        let manifest = CustodyEnvelopeManifestV1::new(
            media_identity.encrypted_content().clone(),
            CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
            CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            digest(0x19),
            nodes,
        )
        .unwrap();
        let shares = [0x51u8, 0x52, 0x53].into_iter().map(sealed_share).collect();
        CustodyEnvelopeV1::new(manifest, shares).unwrap()
    }

    fn media_identity() -> CencFmp4MediaIdentityV1 {
        test_media::media_identity(0x11)
    }

    fn media_components() -> (Vec<u8>, Vec<Vec<u8>>, &'static str, &'static str) {
        test_media::media_components(0x11)
    }

    fn draft_for(envelope: &CustodyEnvelopeV1) -> RuntimeMintDraft {
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        RuntimeMintDraft::new(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
            envelope.key_envelope_identity().unwrap(),
            RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
            envelope.manifest().content_key_commitment(),
            envelope.manifest().threshold(),
            vec![binding(1), binding(2), binding(3)],
        )
        .unwrap()
    }

    fn owner_only_journal_root(temp: &tempfile::TempDir) -> PathBuf {
        let parent = temp.path().join("owner-only-parent");
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&parent).unwrap();
        parent.join("runtime-mint")
    }

    fn coordinator_with<'a>(
        journal: RuntimeMintJournal,
        nodes: Vec<RuntimeMintSelectedNode<'a>>,
    ) -> RuntimeMintCoordinator<'a> {
        RuntimeMintCoordinator::new(journal, runtime_issuer(), sign_statement, nodes).unwrap()
    }

    #[test]
    fn one_node_selection_is_rejected() {
        let one = FakeMintCustody::new(node_public_key(1), false, PathBuf::new(), digest(1));
        assert!(RuntimeMintCoordinator::new(
            RuntimeMintJournal::new("/tmp/mint-one-node"),
            runtime_issuer(),
            sign_statement,
            vec![RuntimeMintSelectedNode::new(binding(1), &one)],
        )
        .is_err());
        let nodes = vec![binding(1)];
        let media = media_identity();
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &encrypted_segments,
                mime_type,
                codecs,
                KeyEnvelopeIdentityV1::new(
                    media.encrypted_content().clone(),
                    digest(0x22),
                    512,
                    digest(0x23),
                    ThresholdV1::new(2, 3).unwrap(),
                    CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
                    CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
                    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
                )
                .unwrap(),
                RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
                digest(0x19),
                ThresholdV1::new(2, 3).unwrap(),
                nodes,
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );
    }

    #[test]
    fn duplicate_operator_or_failure_domain_is_rejected() {
        let mut nodes = vec![binding(1), binding(2), binding(3)];
        nodes[2] = RuntimeMintNodeBinding::new(
            node_public_key(3),
            binding(1).operator_id(),
            binding(3).failure_domain_id(),
            binding(3).owner_state_root(),
        )
        .unwrap();
        let envelope = envelope();
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &encrypted_segments,
                mime_type,
                codecs,
                envelope.key_envelope_identity().unwrap(),
                RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
                envelope.manifest().content_key_commitment(),
                envelope.manifest().threshold(),
                nodes,
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );
    }

    #[tokio::test]
    async fn three_distinct_nodes_commit_custody_provisioning() {
        let temp = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temp);
        let envelope = envelope();
        let draft = draft_for(&envelope);
        let node1 = FakeMintCustody::new(
            node_public_key(1),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node2 = FakeMintCustody::new(
            node_public_key(2),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node3 = FakeMintCustody::new(
            node_public_key(3),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let coordinator = coordinator_with(
            RuntimeMintJournal::new(&journal_root),
            vec![
                RuntimeMintSelectedNode::new(binding(1), &node1),
                RuntimeMintSelectedNode::new(binding(2), &node2),
                RuntimeMintSelectedNode::new(binding(3), &node3),
            ],
        );

        let outcome = coordinator
            .provision(&draft, &envelope, NOW + 10)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RuntimeMintCoordinatorOutcome::CustodyProvisioned {
                mint_id: draft.mint_id()
            }
        );
        assert_eq!(node1.request_count(), 1);
        assert_eq!(node2.request_count(), 1);
        assert_eq!(node3.request_count(), 1);

        let loaded = RuntimeMintJournal::new(&journal_root)
            .load(draft.mint_id())
            .unwrap();
        assert_eq!(
            loaded.custody_terminal(),
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned)
        );
        assert_eq!(loaded.accepted_orphans().len(), 3);

        let replay = coordinator
            .provision(&draft, &envelope, NOW + 10)
            .await
            .unwrap();
        assert_eq!(replay, outcome);
        assert_eq!(node1.request_count(), 1);
        assert_eq!(node2.request_count(), 1);
        assert_eq!(node3.request_count(), 1);

        let record_bytes =
            fs::read(journal_root.join(hex::encode(draft.mint_id().as_bytes()))).unwrap();
        for seed in [0x51u8, 0x52, 0x53] {
            let share = sealed_share(seed);
            assert!(
                !record_bytes
                    .windows(share.envelope().len())
                    .any(|window| window == share.envelope()),
                "mint journal must not persist sealed share bytes"
            );
        }
        let debug = format!("{loaded:?} {draft:?} {coordinator:?} {outcome:?}");
        assert!(!debug.contains("sealed_share"));
        assert!(!debug.contains("/tmp/"));
    }

    #[tokio::test]
    async fn partial_provision_is_durable_abort() {
        let temp = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temp);
        let envelope = envelope();
        let draft = draft_for(&envelope);
        let node1 = FakeMintCustody::new(
            node_public_key(1),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node2 = FakeMintCustody::new(
            node_public_key(2),
            true,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node3 = FakeMintCustody::new(
            node_public_key(3),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let coordinator = coordinator_with(
            RuntimeMintJournal::new(&journal_root),
            vec![
                RuntimeMintSelectedNode::new(binding(1), &node1),
                RuntimeMintSelectedNode::new(binding(2), &node2),
                RuntimeMintSelectedNode::new(binding(3), &node3),
            ],
        );

        let outcome = coordinator
            .provision(&draft, &envelope, NOW + 10)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RuntimeMintCoordinatorOutcome::AbortedPartialProvision {
                mint_id: draft.mint_id(),
                accepted_orphan_count: 1,
            }
        );
        assert_eq!(node1.request_count(), 1);
        assert_eq!(node2.request_count(), 1);
        assert_eq!(node3.request_count(), 0);

        let loaded = RuntimeMintJournal::new(&journal_root)
            .load(draft.mint_id())
            .unwrap();
        assert_eq!(
            loaded.custody_terminal(),
            Some(RuntimeCustodyTerminalKind::AbortedPartialProvision)
        );
        assert_eq!(loaded.accepted_orphans().len(), 1);
        assert_eq!(
            RuntimeMintJournal::new(&journal_root).mark_custody_provisioned(draft.mint_id()),
            Err(RuntimeMintJournalError::Conflict)
        );

        let replay = coordinator
            .provision(&draft, &envelope, NOW + 10)
            .await
            .unwrap();
        assert_eq!(replay, outcome);
        assert_eq!(node1.request_count(), 1);
        assert_eq!(node3.request_count(), 0);
    }

    #[tokio::test]
    async fn restart_after_effect_started_stays_nonterminal() {
        let temp = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temp);
        let envelope = envelope();
        let draft = draft_for(&envelope);
        let journal = RuntimeMintJournal::new(&journal_root);
        journal.persist_bound(&draft).unwrap();
        journal
            .mark_node_effect_started(draft.mint_id(), node_public_key(1))
            .unwrap();

        let node1 = FakeMintCustody::new(
            node_public_key(1),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node2 = FakeMintCustody::new(
            node_public_key(2),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node3 = FakeMintCustody::new(
            node_public_key(3),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let coordinator = coordinator_with(
            RuntimeMintJournal::new(&journal_root),
            vec![
                RuntimeMintSelectedNode::new(binding(1), &node1),
                RuntimeMintSelectedNode::new(binding(2), &node2),
                RuntimeMintSelectedNode::new(binding(3), &node3),
            ],
        );
        assert_eq!(
            coordinator
                .provision(&draft, &envelope, NOW + 10)
                .await
                .unwrap(),
            RuntimeMintCoordinatorOutcome::Nonterminal {
                mint_id: draft.mint_id(),
                reason: RuntimeMintNonterminalReason::ProviderEffectAlreadyStarted,
            }
        );
        assert_eq!(node1.request_count(), 0);
        let loaded = RuntimeMintJournal::new(&journal_root)
            .load(draft.mint_id())
            .unwrap();
        assert!(loaded.custody_terminal().is_none());
    }

    #[tokio::test]
    async fn custody_provisioned_replays_without_redispatch() {
        let temp = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temp);
        let envelope = envelope();
        let draft = draft_for(&envelope);
        let journal = RuntimeMintJournal::new(&journal_root);
        journal.persist_bound(&draft).unwrap();
        for seed in [1u8, 2, 3] {
            let node = binding(seed);
            journal
                .mark_node_effect_started(draft.mint_id(), node.node_public_key())
                .unwrap();
            journal
                .mark_node_receipt(
                    draft.mint_id(),
                    RuntimeMintNodeReceipt::new(
                        node.node_public_key(),
                        mint_node_provisioning_id(draft.mint_id(), node.node_public_key()).unwrap(),
                        elastos_protected_content_contracts::CustodyNodeProvisioningRecordIdentityV1::new(
                            digest(0xb0 + seed),
                            128,
                        )
                        .unwrap(),
                        node.owner_state_root(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        journal.mark_custody_provisioned(draft.mint_id()).unwrap();
        let loaded = journal.load(draft.mint_id()).unwrap();
        assert_eq!(
            loaded.custody_terminal(),
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned)
        );

        let node1 = FakeMintCustody::new(
            node_public_key(1),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node2 = FakeMintCustody::new(
            node_public_key(2),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let node3 = FakeMintCustody::new(
            node_public_key(3),
            false,
            journal_root.clone(),
            draft.mint_id(),
        );
        let coordinator = coordinator_with(
            RuntimeMintJournal::new(&journal_root),
            vec![
                RuntimeMintSelectedNode::new(binding(1), &node1),
                RuntimeMintSelectedNode::new(binding(2), &node2),
                RuntimeMintSelectedNode::new(binding(3), &node3),
            ],
        );
        assert_eq!(
            coordinator
                .provision(&draft, &envelope, NOW + 10)
                .await
                .unwrap(),
            RuntimeMintCoordinatorOutcome::CustodyProvisioned {
                mint_id: draft.mint_id()
            }
        );
        assert_eq!(node1.request_count(), 0);
        assert_eq!(
            RuntimeMintJournal::new(&journal_root)
                .load(draft.mint_id())
                .unwrap()
                .custody_terminal(),
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned)
        );
    }

    #[test]
    fn resolver_maps_only_the_validated_committee_in_canonical_order() {
        let temporary = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temporary);
        let mint_id = digest(0x51);
        let node1 = FakeMintCustody::new(node_public_key(1), false, journal_root.clone(), mint_id);
        let node2 = FakeMintCustody::new(node_public_key(2), false, journal_root.clone(), mint_id);
        let node3 = FakeMintCustody::new(node_public_key(3), false, journal_root, mint_id);
        let (issuer, authorization_identity, pool, epoch, authorization) = selection_policy();
        let configured = vec![
            configured_provider(3, 0x33, 0xd3, &node3),
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
        ];

        let selected = resolve_runtime_mint_selected_nodes(
            issuer,
            authorization_identity,
            &pool,
            &epoch,
            &authorization,
            NOW,
            &configured,
        )
        .unwrap();

        assert_eq!(selected.len(), 3);
        assert_eq!(
            selected
                .iter()
                .map(|node| node.binding().node_public_key())
                .collect::<Vec<_>>(),
            epoch
                .verify()
                .unwrap()
                .nodes()
                .iter()
                .map(|node| node.node_public_key())
                .collect::<Vec<_>>(),
        );
        for selected_node in &selected {
            let configured_node = configured
                .iter()
                .find(|candidate| {
                    candidate.node_public_key() == selected_node.binding().node_public_key()
                })
                .unwrap();
            assert_eq!(
                selected_node.binding().owner_state_root(),
                configured_node.owner_state_root()
            );
        }
        assert_eq!(node1.request_count(), 0);
        assert_eq!(node2.request_count(), 0);
        assert_eq!(node3.request_count(), 0);
    }

    #[test]
    fn resolver_rejects_nonexact_or_mismatched_configured_providers() {
        let temporary = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temporary);
        let mint_id = digest(0x52);
        let node1 = FakeMintCustody::new(node_public_key(1), false, journal_root.clone(), mint_id);
        let node2 = FakeMintCustody::new(node_public_key(2), false, journal_root.clone(), mint_id);
        let node3 = FakeMintCustody::new(node_public_key(3), false, journal_root.clone(), mint_id);
        let node4 = FakeMintCustody::new(node_public_key(4), false, journal_root, mint_id);
        let (issuer, authorization_identity, pool, epoch, authorization) = selection_policy();
        let resolve = |configured| {
            resolve_runtime_mint_selected_nodes(
                issuer,
                authorization_identity,
                &pool,
                &epoch,
                &authorization,
                NOW,
                configured,
            )
        };
        let exact = [
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        assert!(resolve(&exact[..2]).is_err(), "missing provider must fail");

        let extra = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
            configured_provider(4, 0x34, 0xd4, &node4),
        ];
        assert!(resolve(&extra).is_err(), "extra provider must fail");

        let duplicate_node = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(1, 0x31, 0xd2, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        assert!(resolve(&duplicate_node).is_err());

        let alternate_node = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
            configured_provider(4, 0x34, 0xd3, &node4),
        ];
        assert!(resolve(&alternate_node).is_err());

        let wrong_custody_key = vec![
            configured_provider(1, 0x41, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        assert!(resolve(&wrong_custody_key).is_err());

        let duplicate_root = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd1, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        assert!(resolve(&duplicate_root).is_err());
        assert!(RuntimeMintConfiguredCustodyProvider::new(
            node_public_key(1),
            node_custody_public_key(0x31),
            Digest32::new([0; 32]),
            &node1,
        )
        .is_err());

        let duplicate_provider = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node1),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        assert!(resolve(&duplicate_provider).is_err());
        assert_eq!(node1.request_count(), 0);
        assert_eq!(node2.request_count(), 0);
        assert_eq!(node3.request_count(), 0);
        assert_eq!(node4.request_count(), 0);
    }

    #[test]
    fn resolver_rejects_wrong_pinned_policy_or_committee_authorization() {
        let temporary = tempdir().unwrap();
        let journal_root = owner_only_journal_root(&temporary);
        let mint_id = digest(0x53);
        let node1 = FakeMintCustody::new(node_public_key(1), false, journal_root.clone(), mint_id);
        let node2 = FakeMintCustody::new(node_public_key(2), false, journal_root.clone(), mint_id);
        let node3 = FakeMintCustody::new(node_public_key(3), false, journal_root, mint_id);
        let (issuer, authorization_identity, pool, epoch, authorization) = selection_policy();
        let configured = vec![
            configured_provider(1, 0x31, 0xd1, &node1),
            configured_provider(2, 0x32, 0xd2, &node2),
            configured_provider(3, 0x33, 0xd3, &node3),
        ];
        let wrong_issuer = CustodyEpochIssuerKeyV1::new(
            SigningKey::from_bytes(&[0x75; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        assert!(resolve_runtime_mint_selected_nodes(
            wrong_issuer,
            authorization_identity,
            &pool,
            &epoch,
            &authorization,
            NOW,
            &configured,
        )
        .is_err());
        assert!(resolve_runtime_mint_selected_nodes(
            issuer,
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0xf1), 1).unwrap(),
            &pool,
            &epoch,
            &authorization,
            NOW,
            &configured,
        )
        .is_err());
        assert_eq!(node1.request_count(), 0);
        assert_eq!(node2.request_count(), 0);
        assert_eq!(node3.request_count(), 0);
    }
}
