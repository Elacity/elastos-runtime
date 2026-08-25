use std::collections::{BTreeSet, HashMap};
use std::fmt;

use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, CanonicalContract, ContractError, Digest32,
    NodePublicKey, NodeSetV1, RightsDecisionV1, RuntimeOperationIssuerKeyV1,
    SignedNodeContributionV1, SignedNodeRightsDecisionV1, SignedRuntimeReleaseOperationV1,
    WalletAddress, WalletSignedRightsRequestV1,
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

#[async_trait::async_trait]
pub trait RuntimeRightsProvider: Send + Sync {
    async fn evaluate_rights(
        &self,
        request: &RightsProviderRequestV1,
    ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError>;
}

#[async_trait::async_trait]
pub trait RuntimeCustodyProvider: Send + Sync {
    async fn release_contribution(
        &self,
        request: &CustodyProviderRequestV1,
    ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError>;

    async fn provision_node_share(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartedOperationResolutionMode {
    InitialDispatch,
    ExactResume,
}

#[derive(Debug)]
struct StartedOperationCollection {
    offer: Option<RuntimeReleaseReconcileOffer>,
    replayable_rights_decisions: Vec<SignedNodeRightsDecisionV1>,
}

struct StartedOperationContext<'a, 'b> {
    draft: &'a RuntimeReleaseOperationDraft,
    signed_runtime_release_operation: &'a SignedRuntimeReleaseOperationV1,
    authenticated: &'a AuthenticatedRuntimeReleaseOperationV1,
    ordered_providers: &'a [&'b RuntimeSelectedProvider<'b>],
    replayable_rights_decisions: &'a [SignedNodeRightsDecisionV1],
    now_unix_seconds: u64,
    mode: StartedOperationResolutionMode,
}

impl<'a, 'b> StartedOperationContext<'a, 'b> {
    fn threshold(&self) -> Result<usize, RuntimeReleaseCoordinatorError> {
        self.authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map(|node_set| usize::from(node_set.threshold().required()))
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RuntimeReleaseReconcileOffer {
    RightsDenied {
        signed_node_rights_decision: Box<SignedNodeRightsDecisionV1>,
    },
    ContributionsReady {
        signed_node_contributions: Vec<SignedNodeContributionV1>,
    },
}

impl fmt::Debug for RuntimeReleaseReconcileOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RightsDenied { .. } => formatter.write_str("RightsDenied"),
            Self::ContributionsReady {
                signed_node_contributions,
            } => formatter
                .debug_struct("ContributionsReady")
                .field("contribution_count", &signed_node_contributions.len())
                .finish(),
        }
    }
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
    #[error("runtime release reconciliation is invalid")]
    Reconciliation,
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

    pub async fn release(
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
        self.resolve_started_operation(
            draft.operation_hash()?,
            StartedOperationContext {
                draft: &draft,
                signed_runtime_release_operation: &signed_runtime_release_operation,
                authenticated: &authenticated,
                ordered_providers: &ordered_providers,
                replayable_rights_decisions: &[],
                now_unix_seconds,
                mode: StartedOperationResolutionMode::InitialDispatch,
            },
        )
        .await
    }

    pub async fn resume_exact(
        &self,
        operation_hash: Digest32,
        now_unix_seconds: u64,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        let persisted = self
            .journal
            .load(operation_hash)
            .map_err(|error| match error {
                RuntimeReleaseJournalError::NotFound => {
                    RuntimeReleaseCoordinatorError::Reconciliation
                }
                _ => RuntimeReleaseCoordinatorError::Journal,
            })?;
        if let Some(terminal) = persisted.terminal_result().cloned() {
            return Ok(RuntimeReleaseCoordinatorOutcome::Terminal(terminal));
        }
        if !persisted.provider_effect_started() {
            return Err(RuntimeReleaseCoordinatorError::Reconciliation);
        }
        let draft = persisted.draft().clone();
        if draft.operation_hash()? != operation_hash {
            return Err(RuntimeReleaseCoordinatorError::Reconciliation);
        }
        let signed_runtime_release_operation = draft.signed_runtime_release_operation().clone();
        let authenticated = signed_runtime_release_operation
            .verify(self.expected_runtime_issuer, now_unix_seconds)
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let ordered_providers = self.selected_ordered_providers(&authenticated)?;
        self.resolve_started_operation(
            operation_hash,
            StartedOperationContext {
                draft: &draft,
                signed_runtime_release_operation: &signed_runtime_release_operation,
                authenticated: &authenticated,
                ordered_providers: &ordered_providers,
                replayable_rights_decisions: persisted.replayable_rights_decisions(),
                now_unix_seconds,
                mode: StartedOperationResolutionMode::ExactResume,
            },
        )
        .await
    }

    pub fn reconcile(
        &self,
        operation_hash: Digest32,
        offer: RuntimeReleaseReconcileOffer,
        now_unix_seconds: u64,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        let persisted = self
            .journal
            .load(operation_hash)
            .map_err(|error| match error {
                RuntimeReleaseJournalError::NotFound => {
                    RuntimeReleaseCoordinatorError::Reconciliation
                }
                _ => RuntimeReleaseCoordinatorError::Journal,
            })?;
        let draft = persisted.draft().clone();
        if draft.operation_hash()? != operation_hash {
            return Err(RuntimeReleaseCoordinatorError::Reconciliation);
        }
        if let Some(terminal) = persisted.terminal_result() {
            if offer_to_terminal(&offer) != *terminal {
                return Err(RuntimeReleaseCoordinatorError::Reconciliation);
            }
            return Ok(RuntimeReleaseCoordinatorOutcome::Terminal(terminal.clone()));
        }
        if !persisted.provider_effect_started() {
            return Err(RuntimeReleaseCoordinatorError::Reconciliation);
        }
        let signed_runtime_release_operation = draft.signed_runtime_release_operation().clone();
        let authenticated = signed_runtime_release_operation
            .verify(self.expected_runtime_issuer, now_unix_seconds)
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let node_set = authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let terminal = match offer {
            RuntimeReleaseReconcileOffer::RightsDenied {
                signed_node_rights_decision,
            } => {
                let verified = authenticated
                    .verify_node_rights_decision(
                        &signed_node_rights_decision,
                        &node_set,
                        now_unix_seconds,
                    )
                    .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
                if verified.decision() != RightsDecisionV1::Denied {
                    return Err(RuntimeReleaseCoordinatorError::ProviderResult);
                }
                RuntimeReleaseTerminalResult::RightsDenied {
                    signed_node_rights_decision,
                }
            }
            RuntimeReleaseReconcileOffer::ContributionsReady {
                signed_node_contributions,
            } => RuntimeReleaseTerminalResult::ContributionsReady {
                signed_node_contributions: exact_threshold_contributions(
                    &authenticated,
                    &node_set,
                    &signed_node_contributions,
                    now_unix_seconds,
                )?,
            },
        };
        let persisted = self.journal.mark_terminal(&draft, terminal)?;
        Ok(RuntimeReleaseCoordinatorOutcome::Terminal(
            persisted.into_terminal_result()?,
        ))
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

    async fn resolve_started_operation(
        &self,
        operation_hash: Digest32,
        context: StartedOperationContext<'_, '_>,
    ) -> Result<RuntimeReleaseCoordinatorOutcome, RuntimeReleaseCoordinatorError> {
        let collection = self.collect_rights_decisions(&context).await?;
        let Some(offer) = collection.offer else {
            let persisted_decisions = if collection.replayable_rights_decisions.is_empty() {
                context.replayable_rights_decisions.to_vec()
            } else {
                self.journal
                    .persist_replayable_rights_decisions(
                        context.draft,
                        &collection.replayable_rights_decisions,
                    )
                    .map_err(|_| RuntimeReleaseCoordinatorError::Journal)?
                    .replayable_rights_decisions()
                    .to_vec()
            };
            if persisted_decisions.len() < context.threshold()? {
                return Ok(RuntimeReleaseCoordinatorOutcome::Nonterminal {
                    operation_hash,
                    reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                });
            }
            let offer = self
                .collect_contributions(&StartedOperationContext {
                    replayable_rights_decisions: &persisted_decisions,
                    ..context
                })
                .await?;
            let Some(offer) = offer else {
                return Ok(RuntimeReleaseCoordinatorOutcome::Nonterminal {
                    operation_hash,
                    reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
                });
            };
            return self.reconcile(operation_hash, offer, context.now_unix_seconds);
        };
        self.reconcile(operation_hash, offer, context.now_unix_seconds)
    }

    async fn collect_rights_decisions(
        &self,
        context: &StartedOperationContext<'_, '_>,
    ) -> Result<StartedOperationCollection, RuntimeReleaseCoordinatorError> {
        let node_set = context
            .authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let threshold = context.threshold()?;
        let mut persisted_by_node = self.persisted_rights_decisions_by_node(context, &node_set)?;
        if persisted_by_node.len() >= threshold {
            return Ok(StartedOperationCollection {
                offer: None,
                replayable_rights_decisions: Vec::new(),
            });
        }
        let mut newly_replayable_rights_decisions = Vec::new();
        for provider in context.ordered_providers.iter().copied() {
            if persisted_by_node.contains_key(&provider.node_public_key) {
                continue;
            }
            let request = RightsProviderRequestV1::new_evaluate(
                provider.node_public_key,
                context.signed_runtime_release_operation,
            )
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
            let response = match provider.rights.evaluate_rights(&request).await {
                Ok(response) => response,
                Err(_) => match context.mode {
                    StartedOperationResolutionMode::InitialDispatch => {
                        return Ok(StartedOperationCollection {
                            offer: None,
                            replayable_rights_decisions: newly_replayable_rights_decisions,
                        });
                    }
                    StartedOperationResolutionMode::ExactResume => continue,
                },
            };
            if response
                .validate_against_request_at(
                    &request,
                    self.expected_runtime_issuer,
                    context.now_unix_seconds,
                )
                .is_err()
            {
                return match context.mode {
                    StartedOperationResolutionMode::InitialDispatch => {
                        Ok(StartedOperationCollection {
                            offer: None,
                            replayable_rights_decisions: newly_replayable_rights_decisions,
                        })
                    }
                    StartedOperationResolutionMode::ExactResume => {
                        Err(RuntimeReleaseCoordinatorError::ProviderResult)
                    }
                };
            }
            let decision = match response.signed_node_rights_decision() {
                Ok(decision) => decision,
                Err(_) => {
                    return match context.mode {
                        StartedOperationResolutionMode::InitialDispatch => {
                            Ok(StartedOperationCollection {
                                offer: None,
                                replayable_rights_decisions: newly_replayable_rights_decisions,
                            })
                        }
                        StartedOperationResolutionMode::ExactResume => {
                            Err(RuntimeReleaseCoordinatorError::ProviderResult)
                        }
                    };
                }
            };
            let verified = context
                .authenticated
                .verify_node_rights_decision(&decision, &node_set, context.now_unix_seconds)
                .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
            if verified.node_public_key() != provider.node_public_key {
                return Err(RuntimeReleaseCoordinatorError::ProviderResult);
            }
            if verified.decision() == RightsDecisionV1::Denied {
                return Ok(StartedOperationCollection {
                    offer: Some(RuntimeReleaseReconcileOffer::RightsDenied {
                        signed_node_rights_decision: Box::new(decision),
                    }),
                    replayable_rights_decisions: newly_replayable_rights_decisions,
                });
            }
            if persisted_by_node
                .insert(provider.node_public_key, decision.clone())
                .is_some()
            {
                return Err(RuntimeReleaseCoordinatorError::ProviderResult);
            }
            newly_replayable_rights_decisions.push(decision);
            if persisted_by_node.len() == threshold {
                break;
            }
        }
        Ok(StartedOperationCollection {
            offer: None,
            replayable_rights_decisions: newly_replayable_rights_decisions,
        })
    }

    async fn collect_contributions(
        &self,
        context: &StartedOperationContext<'_, '_>,
    ) -> Result<Option<RuntimeReleaseReconcileOffer>, RuntimeReleaseCoordinatorError> {
        let node_set = context
            .authenticated
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
        let threshold = context.threshold()?;
        let persisted_by_node = self.persisted_rights_decisions_by_node(context, &node_set)?;
        let mut contributions = Vec::with_capacity(threshold);
        for provider in context.ordered_providers.iter().copied() {
            let Some(decision) = persisted_by_node.get(&provider.node_public_key) else {
                continue;
            };
            let request = CustodyProviderRequestV1::new_release_contribution(
                context.signed_runtime_release_operation,
                decision,
            )
            .map_err(|_| RuntimeReleaseCoordinatorError::OperationAuthority)?;
            let response = match provider.custody.release_contribution(&request).await {
                Ok(response) => response,
                Err(_) => match context.mode {
                    StartedOperationResolutionMode::InitialDispatch => return Ok(None),
                    StartedOperationResolutionMode::ExactResume => continue,
                },
            };
            if response
                .validate_against_request_at(
                    &request,
                    self.expected_runtime_issuer,
                    provider.node_public_key,
                    context.now_unix_seconds,
                )
                .is_err()
            {
                return match context.mode {
                    StartedOperationResolutionMode::InitialDispatch => Ok(None),
                    StartedOperationResolutionMode::ExactResume => {
                        Err(RuntimeReleaseCoordinatorError::ProviderResult)
                    }
                };
            }
            let contribution = response
                .signed_node_contribution()
                .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
            validate_contribution_identity(
                context.authenticated,
                &node_set,
                &contribution,
                provider.node_public_key,
                context.now_unix_seconds,
            )?;
            contributions.push(contribution);
            if contributions.len() == threshold {
                return Ok(Some(RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: contributions,
                }));
            }
        }
        Ok(None)
    }

    fn persisted_rights_decisions_by_node(
        &self,
        context: &StartedOperationContext<'_, '_>,
        node_set: &NodeSetV1,
    ) -> Result<HashMap<NodePublicKey, SignedNodeRightsDecisionV1>, RuntimeReleaseCoordinatorError>
    {
        if context.replayable_rights_decisions.len() > context.ordered_providers.len() {
            return Err(RuntimeReleaseCoordinatorError::ProviderResult);
        }
        let selected_nodes = context
            .ordered_providers
            .iter()
            .map(|provider| provider.node_public_key)
            .collect::<BTreeSet<_>>();
        let mut persisted_by_node =
            HashMap::with_capacity(context.replayable_rights_decisions.len());
        for decision in context.replayable_rights_decisions {
            let verified = context
                .authenticated
                .verify_node_rights_decision(decision, node_set, context.now_unix_seconds)
                .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?;
            if verified.decision() != RightsDecisionV1::Allowed {
                return Err(RuntimeReleaseCoordinatorError::ProviderResult);
            }
            if !selected_nodes.contains(&verified.node_public_key()) {
                return Err(RuntimeReleaseCoordinatorError::ProviderResult);
            }
            if persisted_by_node
                .insert(verified.node_public_key(), decision.clone())
                .is_some()
            {
                return Err(RuntimeReleaseCoordinatorError::ProviderResult);
            }
        }
        Ok(persisted_by_node)
    }
}

pub(crate) fn validate_wallet_rights_signature(
    wallet_request: &WalletProviderRequestV2,
    wallet_response: &WalletProviderResponseV2,
    expected_rights_request: &elastos_protected_content_contracts::RightsRequestV1,
) -> Result<WalletSignedRightsRequestV1, RuntimeReleaseCoordinatorError> {
    let (account_id, canonical_rights_request_hex) = match &wallet_request.operation {
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id,
            canonical_rights_request_hex,
            ..
        } => (account_id, canonical_rights_request_hex),
        _ => return Err(RuntimeReleaseCoordinatorError::WalletAuthority),
    };
    let rights_request_bytes = hex::decode(canonical_rights_request_hex)
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    let rights_request =
        elastos_protected_content_contracts::RightsRequestV1::from_canonical_bytes(
            &rights_request_bytes,
        )
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    if &rights_request != expected_rights_request {
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
    if result.signer != wallet_address_hex(expected_rights_request.binding().wallet()) {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    let signed_rights_bytes = hex::decode(&result.wallet_signed_rights_request_hex)
        .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    let result_signed_rights =
        WalletSignedRightsRequestV1::from_canonical_bytes(&signed_rights_bytes)
            .map_err(|_| RuntimeReleaseCoordinatorError::WalletAuthority)?;
    if result_signed_rights.request() != expected_rights_request {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    Ok(result_signed_rights)
}

fn validate_wallet_release_binding(
    wallet_request: &WalletProviderRequestV2,
    wallet_response: &WalletProviderResponseV2,
    signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
) -> Result<(), RuntimeReleaseCoordinatorError> {
    let signed_rights = signed_runtime_release_operation
        .statement()
        .rights_request();
    let result_signed_rights =
        validate_wallet_rights_signature(wallet_request, wallet_response, signed_rights.request())?;
    if &result_signed_rights != signed_rights {
        return Err(RuntimeReleaseCoordinatorError::WalletAuthority);
    }
    Ok(())
}

fn offer_to_terminal(offer: &RuntimeReleaseReconcileOffer) -> RuntimeReleaseTerminalResult {
    match offer {
        RuntimeReleaseReconcileOffer::RightsDenied {
            signed_node_rights_decision,
        } => RuntimeReleaseTerminalResult::RightsDenied {
            signed_node_rights_decision: signed_node_rights_decision.clone(),
        },
        RuntimeReleaseReconcileOffer::ContributionsReady {
            signed_node_contributions,
        } => RuntimeReleaseTerminalResult::ContributionsReady {
            signed_node_contributions: signed_node_contributions.clone(),
        },
    }
}

fn exact_threshold_contributions(
    authenticated: &AuthenticatedRuntimeReleaseOperationV1,
    node_set: &NodeSetV1,
    contributions: &[SignedNodeContributionV1],
    now_unix_seconds: u64,
) -> Result<Vec<SignedNodeContributionV1>, RuntimeReleaseCoordinatorError> {
    let required = usize::from(node_set.threshold().required());
    if contributions.len() != required {
        return Err(RuntimeReleaseCoordinatorError::ProviderResult);
    }
    let mut seen = BTreeSet::new();
    let mut verified = Vec::with_capacity(required);
    for contribution in contributions {
        let node = authenticated
            .verify_node_contribution(contribution, node_set, now_unix_seconds)
            .map_err(|_| RuntimeReleaseCoordinatorError::ProviderResult)?
            .node_public_key();
        if !seen.insert(node) {
            return Err(RuntimeReleaseCoordinatorError::ProviderResult);
        }
        verified.push(contribution.clone());
    }
    Ok(verified)
}

fn validate_contribution_identity(
    authenticated: &AuthenticatedRuntimeReleaseOperationV1,
    node_set: &NodeSetV1,
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

pub(crate) fn wallet_address_hex(wallet: WalletAddress) -> String {
    format!("0x{}", hex::encode(wallet.as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_auth::ethereum_signed_message_hash;
    use elastos_protected_content_contracts::{
        ContentAccessIdV1, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
        CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1,
        CustodyEpochStatementV1, CustodyNodeIdentityV1, CustodyNodeProvisioningRecordIdentityV1,
        CustodyPoolFailureDomainIdV1, CustodyPoolOperatorIdV1, EncryptedContentIdentityV1,
        EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyReleaseOutcomeV1,
        KeyReleaseRequestV1, NodeContributionRefV1, NodeContributionStatementV1,
        NodeCustodyPublicKeyV1, PqHybridSealedShareV1, ProfileIdentityV1,
        ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1,
        RecipientPublicKeyBytesV1, RecipientSealedContributionV1, ReplayNonce16, RightsActionV1,
        RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
        RightsRequestV1, RightsSubjectSourceV1, RuntimeCustodyProvisioningIdV1,
        RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1,
        ShareCoordinateV1, SignedCustodyEpochV1, SignedNodeRightsDecisionV1,
        SignedRecipientKeyAuthorizationV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
        TerminalReceiptStatementV1, ThresholdV1, WalletAddress,
        CUSTODY_X_WING_AES256GCM_SUITE_ID_V1, PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES,
        X_WING_DRAFT06_CIPHERTEXT_BYTES,
    };
    use elastos_protected_content_provider_contracts::{
        CustodyProviderResponseStatusV1, DecryptProviderRequestV1, DecryptProviderResponseV1,
        ProviderFailureCodeV1, ValidatedCustodyProviderRequestV1,
        ValidatedDecryptProviderRequestV1, ValidatedRightsProviderRequestV1,
        ViewerMediaPartSelectorV1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
    };
    use elastos_wallet_contract::{
        ProtectedContentRightsSignatureResultV1, ValidatedChainOutcomeBindingV1,
        VerifiedWalletInvocationContext, WalletProviderOperationV2, WalletProviderRequestV2,
        WalletProviderResponseV2, WalletResultV2,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use sha3::{Digest as _, Keccak256};
    use tempfile::tempdir;
    use x_wing::kem::{Decapsulator as _, KeyExport as _};
    use x_wing::TryKeyInit as _;

    use serde_json::json;

    use super::*;
    use crate::test_media;
    use crate::{
        bind_buy, close_viewer_session, open_viewer_session, prepare_recipient,
        read_viewer_media_part, RuntimeContentAvailabilityRequirement, RuntimeDecryptProvider,
        RuntimeMintCoordinator, RuntimeMintCoordinatorOutcome, RuntimeMintDraft,
        RuntimeMintJournal, RuntimeMintNodeBinding, RuntimeMintNodeReceipt,
        RuntimeMintSelectedNode, RuntimeOpenError, RuntimeOpenViewerSessionInput,
        RuntimeProtectedContentPurchaseIntent, RuntimePurchaseEffectAuthority,
        RuntimeVerifiedContentAvailability, RuntimeVerifiedPurchaseEffect,
    };

    const NOW: u64 = 2_000_000_000;
    const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
    const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

    #[derive(Default)]
    struct FakeRightsProvider {
        responses: Mutex<Vec<Result<RightsProviderResponseV1, RuntimeProviderCallError>>>,
        requests: Mutex<Vec<RightsProviderRequestV1>>,
    }

    impl FakeRightsProvider {
        fn new(responses: Vec<Result<RightsProviderResponseV1, RuntimeProviderCallError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeRightsProvider for FakeRightsProvider {
        async fn evaluate_rights(
            &self,
            request: &RightsProviderRequestV1,
        ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError> {
            self.requests.lock().unwrap().push(request.clone());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            Ok(responses.remove(0)?)
        }
    }

    #[derive(Default)]
    struct FakeCustodyProvider {
        responses: Mutex<Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>>,
        requests: Mutex<Vec<CustodyProviderRequestV1>>,
    }

    impl FakeCustodyProvider {
        fn new(
            responses: Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeCustodyProvider for FakeCustodyProvider {
        async fn release_contribution(
            &self,
            request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            self.requests.lock().unwrap().push(request.clone());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            Ok(responses.remove(0)?)
        }

        async fn provision_node_share(
            &self,
            _request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            Err(RuntimeProviderCallError::NoExactResult)
        }
    }

    struct InspectingCustodyProvider {
        journal_root: PathBuf,
        operation_hash: Digest32,
        observed_replayable_rights_counts: Mutex<Vec<usize>>,
        responses: Mutex<Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>>,
        requests: Mutex<Vec<CustodyProviderRequestV1>>,
    }

    impl InspectingCustodyProvider {
        fn new(
            journal_root: PathBuf,
            operation_hash: Digest32,
            responses: Vec<Result<CustodyProviderResponseV1, RuntimeProviderCallError>>,
        ) -> Self {
            Self {
                journal_root,
                operation_hash,
                observed_replayable_rights_counts: Mutex::new(Vec::new()),
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn observed_replayable_rights_counts(&self) -> Vec<usize> {
            self.observed_replayable_rights_counts
                .lock()
                .unwrap()
                .clone()
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeCustodyProvider for InspectingCustodyProvider {
        async fn release_contribution(
            &self,
            request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            self.requests.lock().unwrap().push(request.clone());
            let persisted = RuntimeReleaseJournal::new(self.journal_root.clone())
                .load(self.operation_hash)
                .unwrap();
            self.observed_replayable_rights_counts
                .lock()
                .unwrap()
                .push(persisted.replayable_rights_decisions().len());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            Ok(responses.remove(0)?)
        }

        async fn provision_node_share(
            &self,
            _request: &CustodyProviderRequestV1,
        ) -> Result<CustodyProviderResponseV1, RuntimeProviderCallError> {
            Err(RuntimeProviderCallError::NoExactResult)
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
        policy_body_with_media(
            media_identity(0x21),
            ContentAccessIdV1::new([0x41; 16]).unwrap(),
        )
    }

    fn policy_body_with_media(
        encrypted_content: EncryptedContentIdentityV1,
        content_access_id: ContentAccessIdV1,
    ) -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            encrypted_content,
            content_access_id,
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

    fn profile_identity(seed: u8) -> ProfileIdentityV1 {
        ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn media_identity(seed: u8) -> EncryptedContentIdentityV1 {
        test_media::media_identity(seed).encrypted_content().clone()
    }

    fn content_access_id(seed: u8) -> ContentAccessIdV1 {
        ContentAccessIdV1::new([seed; 16]).unwrap()
    }

    fn binding_for_runtime_release(
        envelope: &CustodyEnvelopeV1,
        policy: &RightsPolicyBodyV1,
        profile_seed: u8,
        wallet_seed: u8,
        session_seed: u8,
    ) -> ProtectedContentBindingV1 {
        ProtectedContentBindingV1::new(
            envelope.manifest().encrypted_content().clone(),
            envelope.key_envelope_identity().unwrap(),
            policy.policy_identity().unwrap(),
            profile_identity(profile_seed),
            wallet(wallet_seed),
            RuntimeSessionBindingV1::new(digest(session_seed)).unwrap(),
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
                    node_custody_public_key(0x30 + seed),
                    ShareCoordinateV1::new(seed).unwrap(),
                )
                .unwrap()
            })
            .collect();
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
            media_identity(seed),
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
            .map(sealed_share)
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
        signed_runtime_release_operation_for_envelope(seed, 0x11)
    }

    fn signed_runtime_release_operation_for_envelope(
        seed: u8,
        envelope_seed: u8,
    ) -> SignedRuntimeReleaseOperationV1 {
        signed_runtime_release_operation_for(seed, envelope_seed, 7, 0x30)
    }

    fn signed_runtime_release_operation_for(
        seed: u8,
        envelope_seed: u8,
        wallet_seed: u8,
        recipient_seed: u8,
    ) -> SignedRuntimeReleaseOperationV1 {
        let envelope = custody_envelope(envelope_seed);
        let policy = policy_body();
        let binding =
            binding_for_runtime_release(&envelope, &policy, 0x26, wallet_seed, 0x66 ^ wallet_seed);
        signed_runtime_release_operation_with_binding(
            seed,
            binding,
            policy,
            wallet_seed,
            recipient_seed,
            0x26,
        )
    }

    fn signed_runtime_release_operation_with_binding(
        seed: u8,
        binding: ProtectedContentBindingV1,
        policy: RightsPolicyBodyV1,
        wallet_seed: u8,
        recipient_seed: u8,
        profile_seed: u8,
    ) -> SignedRuntimeReleaseOperationV1 {
        let runtime_key = runtime_key(seed);
        let rights_request = {
            let request = RightsRequestV1::new(
                binding.clone(),
                RightsActionV1::View,
                recipient_identity(recipient_seed),
                NOW,
                NOW + 180,
                ReplayNonce16::new([wallet_seed; 16]),
            )
            .unwrap();
            let (signature, recovery_id) = wallet_key(wallet_seed)
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
        let profile = SigningKey::from_bytes(&[profile_seed; 32]);
        let recipient_public_key = recipient_public_key(recipient_seed);
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
            RuntimeReleaseAuditIdV1::new(digest(0x91 ^ seed ^ wallet_seed)).unwrap(),
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
        wallet_request_response_for(
            operation,
            "profile:alpha",
            "wallet-account-alpha",
            "wallet-request:11111111111111111111111111111111",
        )
    }

    fn wallet_request_response_for(
        operation: &SignedRuntimeReleaseOperationV1,
        profile: &str,
        account_id: &str,
        request_id: &str,
    ) -> (Vec<u8>, Vec<u8>) {
        let context = VerifiedWalletInvocationContext::new(
            profile,
            "runtime-session:alpha",
            Some("proof:alpha".to_string()),
            "grant:alpha",
            "runtime",
            "launch:alpha",
        )
        .unwrap();
        let request = WalletProviderRequestV2::new(
            &context,
            request_id,
            NOW,
            NOW + 120,
            WalletProviderOperationV2::RequestProtectedContentRightsSignature {
                account_id: account_id.to_string(),
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
            account_id,
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

    #[tokio::test]
    async fn runtime_coordination_allows_and_collects_threshold_contributions() {
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
            .await
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

    #[tokio::test]
    async fn runtime_coordination_denial_is_terminal_and_skips_custody() {
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
            .await
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

    #[tokio::test]
    async fn runtime_coordination_replays_terminal_without_dispatch() {
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
            .await
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
            .await
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(r1_replay.request_count(), 0);
        assert_eq!(r2_replay.request_count(), 0);
        assert_eq!(c1_replay.request_count(), 0);
        assert_eq!(c2_replay.request_count(), 0);
    }

    #[tokio::test]
    async fn runtime_coordination_nonterminal_replay_does_not_redispatch() {
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
            .await
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
            .await
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

    #[tokio::test]
    async fn runtime_coordination_resume_exact_settles_uncertain_effect_without_new_operation() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let r1 = FakeRightsProvider::new(vec![
            Err(RuntimeProviderCallError::NoExactResult),
            Err(RuntimeProviderCallError::NoExactResult),
        ]);
        let r2 = FakeRightsProvider::new(vec![
            Err(RuntimeProviderCallError::NoExactResult),
            Ok(
                RightsProviderResponseV1::new_decision(&signed_node_rights_decision(
                    &operation,
                    2,
                    RightsDecisionV1::Allowed,
                ))
                .unwrap(),
            ),
        ]);
        let r3 = FakeRightsProvider::new(vec![
            Ok(
                RightsProviderResponseV1::new_decision(&signed_node_rights_decision(
                    &operation,
                    3,
                    RightsDecisionV1::Allowed,
                ))
                .unwrap(),
            ),
            Ok(
                RightsProviderResponseV1::new_decision(&signed_node_rights_decision(
                    &operation,
                    3,
                    RightsDecisionV1::Allowed,
                ))
                .unwrap(),
            ),
        ]);
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
            &signed_node_contribution(&operation, 2),
        )
        .unwrap())]);
        let c3 = FakeCustodyProvider::new(vec![Ok(CustodyProviderResponseV1::new_contribution(
            &signed_node_contribution(&operation, 3),
        )
        .unwrap())]);
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
            .await
            .unwrap();
        let operation_hash = match first {
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                operation_hash,
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
            } => operation_hash,
            other => panic!("unexpected first outcome: {other:?}"),
        };
        let resumed = runtime.resume_exact(operation_hash, NOW + 6).await.unwrap();
        match resumed {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions,
                },
            ) => assert_eq!(signed_node_contributions.len(), 2),
            other => panic!("unexpected resumed outcome: {other:?}"),
        }
        let persisted = RuntimeReleaseJournal::new(root)
            .load(operation_hash)
            .unwrap();
        assert!(matches!(
            persisted.terminal_result(),
            Some(RuntimeReleaseTerminalResult::ContributionsReady { .. })
        ));
        assert_eq!(c1.request_count(), 0);
        assert_eq!(c2.request_count(), 1);
        assert_eq!(c3.request_count(), 1);
    }

    #[tokio::test]
    async fn runtime_coordination_persists_replayable_rights_before_contribution_dispatch() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let draft = RuntimeReleaseOperationDraft::new(
            wallet_request.clone(),
            wallet_response.clone(),
            operation.clone(),
        )
        .unwrap();
        let operation_hash = draft.operation_hash().unwrap();
        let r1 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let r2 = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let c1 = InspectingCustodyProvider::new(
            root.clone(),
            operation_hash,
            vec![Err(RuntimeProviderCallError::NoExactResult)],
        );
        let c2 = InspectingCustodyProvider::new(
            root.clone(),
            operation_hash,
            vec![Err(RuntimeProviderCallError::NoExactResult)],
        );
        let runtime = coordinator(
            &root,
            vec![
                RuntimeSelectedProvider::new(node_public_key(1), &r1, &c1),
                RuntimeSelectedProvider::new(node_public_key(2), &r2, &c2),
            ],
        );

        let first = runtime
            .release(
                &wallet_request,
                &wallet_response,
                operation.clone(),
                NOW + 6,
            )
            .await
            .unwrap();
        let operation_hash = match first {
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                operation_hash,
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
            } => operation_hash,
            other => panic!("unexpected first outcome: {other:?}"),
        };
        let persisted = RuntimeReleaseJournal::new(root.clone())
            .load(operation_hash)
            .unwrap();
        assert_eq!(persisted.replayable_rights_decisions().len(), 2);
        let observed_counts = [
            c1.observed_replayable_rights_counts(),
            c2.observed_replayable_rights_counts(),
        ]
        .concat();
        assert_eq!(observed_counts, vec![2]);
        assert_eq!(c1.request_count() + c2.request_count(), 1);

        let r1_resume = FakeRightsProvider::default();
        let r2_resume = FakeRightsProvider::default();
        let c1_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&operation, 1))
                .unwrap(),
        )]);
        let c2_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&operation, 2))
                .unwrap(),
        )]);
        let resumed_runtime = coordinator(
            &root,
            vec![
                selected(&r1_resume, &c1_resume, 1),
                selected(&r2_resume, &c2_resume, 2),
            ],
        );
        let resumed = resumed_runtime
            .resume_exact(operation_hash, NOW + 6)
            .await
            .unwrap();
        match resumed {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions,
                },
            ) => assert_eq!(signed_node_contributions.len(), 2),
            other => panic!("unexpected resumed outcome: {other:?}"),
        }
        assert_eq!(r1_resume.request_count(), 0);
        assert_eq!(r2_resume.request_count(), 0);
        assert_eq!(c1_resume.request_count(), 1);
        assert_eq!(c2_resume.request_count(), 1);
    }

    #[tokio::test]
    async fn runtime_coordination_resume_exact_reloads_same_unresolved_operation_hash() {
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
            .await
            .unwrap();
        let operation_hash = match first {
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                operation_hash,
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
            } => operation_hash,
            other => panic!("unexpected first outcome: {other:?}"),
        };

        let r1_resume = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let r2_resume = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let c1_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&operation, 1))
                .unwrap(),
        )]);
        let c2_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&operation, 2))
                .unwrap(),
        )]);
        let resumed_runtime = coordinator(
            &root,
            vec![
                selected(&r1_resume, &c1_resume, 1),
                selected(&r2_resume, &c2_resume, 2),
            ],
        );
        let resumed = resumed_runtime
            .resume_exact(operation_hash, NOW + 6)
            .await
            .unwrap();
        match resumed {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions,
                },
            ) => assert_eq!(signed_node_contributions.len(), 2),
            other => panic!("unexpected resumed outcome: {other:?}"),
        }
        assert_eq!(r1.request_count() + r2.request_count(), 1);
        assert_eq!(r1_resume.request_count(), 1);
        assert_eq!(r2_resume.request_count(), 1);
        assert_eq!(c1_resume.request_count(), 1);
        assert_eq!(c2_resume.request_count(), 1);
    }

    #[tokio::test]
    async fn runtime_coordination_resume_exact_rejects_mismatched_replay_and_keeps_obligation() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let other = signed_runtime_release_operation_for(0x43, 0x12, 7, 0x30);
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
            .await
            .unwrap();
        let operation_hash = match first {
            RuntimeReleaseCoordinatorOutcome::Nonterminal {
                operation_hash,
                reason: RuntimeReleaseNonterminalReason::ProviderEffectUncertain,
            } => operation_hash,
            other => panic!("unexpected first outcome: {other:?}"),
        };

        let r1_resume = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let r2_resume = FakeRightsProvider::new(vec![Ok(RightsProviderResponseV1::new_decision(
            &signed_node_rights_decision(&operation, 2, RightsDecisionV1::Allowed),
        )
        .unwrap())]);
        let c1_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&other, 1))
                .unwrap(),
        )]);
        let c2_resume = FakeCustodyProvider::new(vec![Ok(
            CustodyProviderResponseV1::new_contribution(&signed_node_contribution(&other, 2))
                .unwrap(),
        )]);
        let resumed_runtime = coordinator(
            &root,
            vec![
                selected(&r1_resume, &c1_resume, 1),
                selected(&r2_resume, &c2_resume, 2),
            ],
        );
        assert_eq!(
            resumed_runtime.resume_exact(operation_hash, NOW + 6).await,
            Err(RuntimeReleaseCoordinatorError::ProviderResult)
        );
        let persisted = RuntimeReleaseJournal::new(root)
            .load(operation_hash)
            .unwrap();
        assert!(persisted.provider_effect_started());
        assert!(persisted.terminal_result().is_none());
        assert_eq!(r1_resume.request_count(), 1);
        assert_eq!(r2_resume.request_count(), 1);
        assert_eq!(c1_resume.request_count() + c2_resume.request_count(), 1);
    }

    #[tokio::test]
    async fn runtime_coordination_rejects_wallet_operation_and_result_substitution() {
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
            runtime
                .release(
                    &other_wallet_request,
                    &wallet_response,
                    operation.clone(),
                    NOW + 6
                )
                .await,
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(
            runtime
                .release(&wallet_request, &other_wallet_response, operation, NOW + 6)
                .await,
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[tokio::test]
    async fn runtime_coordination_rejects_wrong_node_set_and_threshold_selection() {
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
            wrong_node_runtime
                .release(
                    &wallet_request,
                    &wallet_response,
                    operation.clone(),
                    NOW + 6,
                )
                .await,
            Err(RuntimeReleaseCoordinatorError::ProviderSelection)
        );
        let threshold_runtime = coordinator(&owner_only_root(&temp), vec![selected(&r1, &c1, 1)]);
        assert_eq!(
            threshold_runtime
                .release(&wallet_request, &wallet_response, operation, NOW + 6)
                .await,
            Err(RuntimeReleaseCoordinatorError::ProviderSelection)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[tokio::test]
    async fn runtime_coordination_wrong_provider_result_stays_nonterminal() {
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
            .await
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

    #[tokio::test]
    async fn runtime_coordination_rejects_mismatched_contribution_identity() {
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
            .await
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

    #[tokio::test]
    async fn runtime_coordination_wallet_error_result_is_not_authority() {
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
            runtime
                .release(
                    &wallet_request,
                    &serde_json::to_vec(&response).unwrap(),
                    operation,
                    NOW + 6,
                )
                .await,
            Err(RuntimeReleaseCoordinatorError::WalletAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[tokio::test]
    async fn runtime_coordination_rejects_operation_substitution_before_dispatch() {
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
            runtime
                .release(&wallet_request, &wallet_response, other, NOW + 6)
                .await,
            Err(RuntimeReleaseCoordinatorError::OperationAuthority)
        );
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    fn persist_effect_started(
        root: &Path,
        operation: &SignedRuntimeReleaseOperationV1,
        wallet_request: &[u8],
        wallet_response: &[u8],
    ) -> Digest32 {
        let journal = RuntimeReleaseJournal::new(root.to_path_buf());
        let draft = RuntimeReleaseOperationDraft::new(
            wallet_request.to_vec(),
            wallet_response.to_vec(),
            operation.clone(),
        )
        .unwrap();
        journal.persist_before_provider_effect(&draft).unwrap();
        journal.mark_provider_effect_started(&draft).unwrap();
        draft.operation_hash().unwrap()
    }

    #[tokio::test]
    async fn runtime_reconcile_settles_exact_contributions_without_redispatch() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let operation_hash =
            persist_effect_started(&root, &operation, &wallet_request, &wallet_response);
        let r1 = FakeRightsProvider::default();
        let r2 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);
        let first = runtime
            .reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            )
            .unwrap();
        match &first {
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::ContributionsReady {
                    signed_node_contributions,
                },
            ) => assert_eq!(signed_node_contributions.len(), 2),
            other => panic!("unexpected outcome: {other:?}"),
        }
        let replay = runtime
            .reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            )
            .unwrap();
        assert_eq!(replay, first);
        let r1_release = FakeRightsProvider::default();
        let r2_release = FakeRightsProvider::default();
        let c1_release = FakeCustodyProvider::default();
        let c2_release = FakeCustodyProvider::default();
        let runtime_release = coordinator(
            &root,
            vec![
                selected(&r1_release, &c1_release, 1),
                selected(&r2_release, &c2_release, 2),
            ],
        );
        let released = runtime_release
            .release(&wallet_request, &wallet_response, operation, NOW + 6)
            .await
            .unwrap();
        assert_eq!(released, first);
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
        assert_eq!(r1_release.request_count(), 0);
        assert_eq!(c1_release.request_count(), 0);
        let record = RuntimeReleaseJournal::new(root)
            .load(operation_hash)
            .unwrap()
            .audit_record()
            .unwrap();
        assert_eq!(
            record.phase(),
            crate::RuntimeReleaseAuditPhase::TerminalContributionsReady {
                contribution_count: 2
            }
        );
        let reason = record.reason();
        for forbidden in [
            "CEK",
            "raw_share",
            "share_bytes",
            "provider_route",
            "endpoint",
            "host",
            "ip",
            "port",
            "127.0.0.1",
        ] {
            assert!(
                !reason.contains(forbidden),
                "unexpected forbidden marker {forbidden}"
            );
        }
    }

    #[test]
    fn runtime_reconcile_settles_rights_denial_without_custody() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let operation_hash =
            persist_effect_started(&root, &operation, &wallet_request, &wallet_response);
        let r1 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r1, &c1, 2)]);
        let outcome = runtime
            .reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::RightsDenied {
                    signed_node_rights_decision: Box::new(signed_node_rights_decision(
                        &operation,
                        1,
                        RightsDecisionV1::Denied,
                    )),
                },
                NOW + 6,
            )
            .unwrap();
        assert!(matches!(
            outcome,
            RuntimeReleaseCoordinatorOutcome::Terminal(
                RuntimeReleaseTerminalResult::RightsDenied { .. }
            )
        ));
        assert_eq!(c1.request_count(), 0);
    }

    #[test]
    fn runtime_reconcile_fails_closed_without_effect_and_on_wrong_identity() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let other = signed_runtime_release_operation(0x43);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let journal = RuntimeReleaseJournal::new(root.clone());
        let draft = RuntimeReleaseOperationDraft::new(
            wallet_request.clone(),
            wallet_response.clone(),
            operation.clone(),
        )
        .unwrap();
        journal.persist_before_provider_effect(&draft).unwrap();
        let r1 = FakeRightsProvider::default();
        let r2 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);
        let pending_hash = draft.operation_hash().unwrap();
        assert_eq!(
            runtime.reconcile(
                pending_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::Reconciliation)
        );
        assert!(journal
            .load(pending_hash)
            .unwrap()
            .terminal_result()
            .is_none());
        assert!(!journal
            .load(pending_hash)
            .unwrap()
            .provider_effect_started());

        let (other_request, other_response) = wallet_request_response(&other);
        let started_hash = persist_effect_started(&root, &other, &other_request, &other_response);
        let started_runtime =
            coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);
        assert_eq!(
            started_runtime.reconcile(
                started_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::OperationAuthority)
        );
        assert!(journal
            .load(started_hash)
            .unwrap()
            .terminal_result()
            .is_none());

        let operation_hash =
            persist_effect_started(&root, &operation, &wallet_request, &wallet_response);
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);
        assert_eq!(
            runtime.reconcile(
                Digest32::new([0x11; 32]),
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::Reconciliation)
        );
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![signed_node_contribution(&operation, 1)],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::ProviderResult)
        );
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 1),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::ProviderResult)
        );
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 4),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::ProviderResult)
        );
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::RightsDenied {
                    signed_node_rights_decision: Box::new(signed_node_rights_decision(
                        &operation,
                        1,
                        RightsDecisionV1::Allowed,
                    )),
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::ProviderResult)
        );
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 1_000,
            ),
            Err(RuntimeReleaseCoordinatorError::OperationAuthority)
        );
        let wrong_issuer = RuntimeReleaseCoordinator::new(
            RuntimeReleaseJournal::new(root.clone()),
            runtime_issuer(0x43),
            vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)],
        )
        .unwrap();
        assert_eq!(
            wrong_issuer.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::OperationAuthority)
        );
        assert!(journal
            .load(operation_hash)
            .unwrap()
            .terminal_result()
            .is_none());
        assert_eq!(r1.request_count(), 0);
        assert_eq!(c1.request_count(), 0);
    }

    #[test]
    fn runtime_reconcile_rejects_conflicting_terminal_offer() {
        let temp = tempdir().unwrap();
        let root = owner_only_root(&temp);
        let operation = signed_runtime_release_operation(0x42);
        let (wallet_request, wallet_response) = wallet_request_response(&operation);
        let operation_hash =
            persist_effect_started(&root, &operation, &wallet_request, &wallet_response);
        let r1 = FakeRightsProvider::default();
        let r2 = FakeRightsProvider::default();
        let c1 = FakeCustodyProvider::default();
        let c2 = FakeCustodyProvider::default();
        let runtime = coordinator(&root, vec![selected(&r1, &c1, 1), selected(&r2, &c2, 2)]);
        runtime
            .reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::RightsDenied {
                    signed_node_rights_decision: Box::new(signed_node_rights_decision(
                        &operation,
                        1,
                        RightsDecisionV1::Denied,
                    )),
                },
                NOW + 6,
            )
            .unwrap();
        assert_eq!(
            runtime.reconcile(
                operation_hash,
                RuntimeReleaseReconcileOffer::ContributionsReady {
                    signed_node_contributions: vec![
                        signed_node_contribution(&operation, 1),
                        signed_node_contribution(&operation, 2),
                    ],
                },
                NOW + 6,
            ),
            Err(RuntimeReleaseCoordinatorError::Reconciliation)
        );
    }
    fn mint_nodes() -> Vec<RuntimeMintNodeBinding> {
        [1u8, 2, 3]
            .into_iter()
            .map(|seed| {
                RuntimeMintNodeBinding::new(
                    node_public_key(seed),
                    CustodyPoolOperatorIdV1::new([0x80 + seed; 32]),
                    CustodyPoolFailureDomainIdV1::new([0x90 + seed; 32]),
                    digest(0xa0 + seed),
                )
                .unwrap()
            })
            .collect()
    }

    fn custody_provisioned_mint_matching(
        operation: &SignedRuntimeReleaseOperationV1,
    ) -> (tempfile::TempDir, crate::PersistedRuntimeMint) {
        let temp = tempdir().unwrap();
        let parent = temp.path().join("owner-only-mint-parent");
        create_owner_only_directory(&parent);
        let journal = RuntimeMintJournal::new(parent.join("runtime-mint"));
        let binding = operation.statement().rights_request().request().binding();
        let (init_segment, encrypted_segments, mime_type, codecs) =
            test_media::media_components(0x11);
        let draft = RuntimeMintDraft::new(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
            content_access_id(0x41),
            binding.key_envelope().clone(),
            binding.rights_policy().clone(),
            digest(0x19),
            ThresholdV1::new(2, 3).unwrap(),
            mint_nodes(),
        )
        .unwrap();
        journal.persist_bound(&draft).unwrap();
        for (index, node) in draft.nodes().iter().enumerate() {
            journal
                .mark_node_effect_started(draft.mint_id(), node.node_public_key())
                .unwrap();
            journal
                .mark_node_receipt(
                    draft.mint_id(),
                    RuntimeMintNodeReceipt::new(
                        node.node_public_key(),
                        RuntimeCustodyProvisioningIdV1::new(digest(0x80 + index as u8)).unwrap(),
                        CustodyNodeProvisioningRecordIdentityV1::new(
                            digest(0xa1 + index as u8),
                            128,
                        )
                        .unwrap(),
                        node.owner_state_root(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let provisioned = journal.mark_custody_provisioned(draft.mint_id()).unwrap();
        (temp, provisioned)
    }

    fn purchase_effect_for_wallet(
        mint: &crate::PersistedRuntimeMint,
        operation: &SignedRuntimeReleaseOperationV1,
    ) -> RuntimeVerifiedPurchaseEffect {
        purchase_effect_for_account(
            mint,
            operation,
            "profile:alpha",
            "wallet-account-alpha",
            "wallet-request:11111111111111111111111111111111",
            0xaa,
        )
    }

    fn purchase_effect_for_account(
        mint: &crate::PersistedRuntimeMint,
        operation: &SignedRuntimeReleaseOperationV1,
        principal_id: &str,
        account_id: &str,
        approval_request_id: &str,
        tx_byte: u8,
    ) -> RuntimeVerifiedPurchaseEffect {
        let rights_request = operation.statement().rights_request().request();
        RuntimeVerifiedPurchaseEffect::new(
            RuntimeProtectedContentPurchaseIntent::new(
                mint.draft().mint_id(),
                mint.draft().encrypted_content().clone(),
                mint.draft().key_envelope().clone(),
                mint.draft().policy().clone(),
                rights_request.action(),
                "eip155:20",
                "esc-mainnet",
                "0x2222222222222222222222222222222222222222",
                "0x1",
                "0x",
            )
            .unwrap(),
            RuntimePurchaseEffectAuthority::new(
                principal_id,
                account_id,
                wallet_address_hex(rights_request.binding().wallet()),
                approval_request_id,
            )
            .unwrap(),
            ValidatedChainOutcomeBindingV1::ManagedSigned {
                signed_transaction_sha256: format!("sha256:{}", hex::encode([tx_byte ^ 1; 32])),
            },
            format!("0x{}", hex::encode([tx_byte; 32])),
            json!({
                "schema": "elastos.chain.broadcast_receipt/v1",
                "network": "esc-mainnet",
            }),
            NOW,
        )
        .unwrap()
    }

    #[test]
    fn custody_provisioned_mint_rejects_buy_pending_content_availability_receipt() {
        let operation = signed_runtime_release_operation(0x42);
        let (_mint_temp, provisioned) = custody_provisioned_mint_matching(&operation);
        assert_eq!(
            bind_buy(
                &provisioned,
                "profile:alpha",
                profile_identity(0x26),
                &purchase_effect_for_wallet(&provisioned, &operation),
            ),
            Err(RuntimeOpenError::MintSelection)
        );
    }

    struct FakeMintCustody {
        expected_issuer: RuntimeOperationIssuerKeyV1,
        node: NodePublicKey,
        now: u64,
        journal_root: PathBuf,
        mint_id: Digest32,
        requests: Mutex<Vec<CustodyProviderRequestV1>>,
    }

    impl FakeMintCustody {
        fn new(node: NodePublicKey, journal_root: PathBuf, mint_id: Digest32) -> Self {
            Self {
                expected_issuer: runtime_issuer(0x42),
                node,
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

    fn sign_mint_statement(bytes: &[u8]) -> [u8; 64] {
        runtime_key(0x42).sign(bytes).to_bytes()
    }

    async fn provision_custody_provisioned_mint(
        operation: &SignedRuntimeReleaseOperationV1,
        envelope: &CustodyEnvelopeV1,
    ) -> (tempfile::TempDir, crate::PersistedRuntimeMint, PathBuf) {
        let temp = tempdir().unwrap();
        let parent = temp.path().join("owner-only-mint-parent");
        create_owner_only_directory(&parent);
        let root = parent.join("runtime-mint");
        let journal = RuntimeMintJournal::new(&root);
        let binding = operation.statement().rights_request().request().binding();
        let (init_segment, encrypted_segments, mime_type, codecs) =
            test_media::media_components(0x11);
        let draft = RuntimeMintDraft::new(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
            content_access_id(0x41),
            binding.key_envelope().clone(),
            binding.rights_policy().clone(),
            envelope.manifest().content_key_commitment(),
            envelope.manifest().threshold(),
            mint_nodes(),
        )
        .unwrap();
        let c1 = FakeMintCustody::new(node_public_key(1), root.clone(), draft.mint_id());
        let c2 = FakeMintCustody::new(node_public_key(2), root.clone(), draft.mint_id());
        let c3 = FakeMintCustody::new(node_public_key(3), root.clone(), draft.mint_id());
        let mint = RuntimeMintCoordinator::new(
            journal,
            runtime_issuer(0x42),
            sign_mint_statement,
            vec![
                RuntimeMintSelectedNode::new(mint_nodes()[0].clone(), &c1),
                RuntimeMintSelectedNode::new(mint_nodes()[1].clone(), &c2),
                RuntimeMintSelectedNode::new(mint_nodes()[2].clone(), &c3),
            ],
        )
        .unwrap();
        assert_eq!(
            mint.provision(&draft, envelope, NOW + 10).await.unwrap(),
            RuntimeMintCoordinatorOutcome::CustodyProvisioned {
                mint_id: draft.mint_id(),
            }
        );
        assert_eq!(c1.request_count(), 1);
        assert_eq!(c2.request_count(), 1);
        assert_eq!(c3.request_count(), 1);
        let provisioned = RuntimeMintJournal::new(&root)
            .load(draft.mint_id())
            .unwrap();
        assert_eq!(
            provisioned.custody_terminal(),
            Some(crate::RuntimeCustodyTerminalKind::CustodyProvisioned)
        );
        (temp, provisioned, root)
    }

    fn content_availability_requirement() -> RuntimeContentAvailabilityRequirement {
        RuntimeContentAvailabilityRequirement::new(
            "did:elastos:content-provider-alpha",
            "did:elastos:protected-content-object-alpha",
            "did:elastos:protected-content-publisher-alpha",
            "protected-content-replication/v1",
            3,
            300,
            30,
        )
        .unwrap()
    }

    fn verified_content_availability(
        mint: &crate::PersistedRuntimeMint,
    ) -> RuntimeVerifiedContentAvailability {
        RuntimeVerifiedContentAvailability::new(
            "bafybeiprotectedcontentavailability",
            "did:elastos:protected-content-object-alpha",
            "did:elastos:protected-content-publisher-alpha",
            &content_availability_requirement(),
            3,
            NOW + 11,
            digest(0x7e),
            mint.draft().encrypted_content().clone(),
            mint.draft().media_identity().media_manifest_root(),
        )
        .unwrap()
    }

    fn opaque_handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        let mut handle = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        handle[0] = seed.max(1);
        handle[MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1 - 1] = seed ^ 0x5a;
        handle
    }

    fn signed_terminal_receipt(
        operation: &SignedRuntimeReleaseOperationV1,
        contributions: &[SignedNodeContributionV1],
        issuer_seed: u8,
    ) -> SignedTerminalReceiptV1 {
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 6)
            .unwrap();
        let node_set = signed_custody_epoch().statement().node_set().unwrap();
        let verified = contributions
            .iter()
            .map(|contribution| {
                authenticated
                    .verify_node_contribution(contribution, &node_set, NOW + 6)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let refs = verified
            .iter()
            .map(NodeContributionRefV1::from)
            .collect::<Vec<_>>();
        let issuer = SigningKey::from_bytes(&[issuer_seed; 32]);
        let statement = TerminalReceiptStatementV1::new(
            authenticated.release_request_hash(),
            authenticated.binding().clone(),
            TerminalReceiptIssuerKey::new(issuer.verifying_key().to_bytes()).unwrap(),
            KeyReleaseOutcomeV1::Released,
            refs,
            NOW + 6,
            NOW + 35,
        )
        .unwrap();
        SignedTerminalReceiptV1::new(
            statement.clone(),
            issuer
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    struct FakeCompositeDecryptProvider {
        expected_issuer: RuntimeOperationIssuerKeyV1,
        recipient_public_key: RecipientPublicKeyBytesV1,
        recipient_identity: RecipientKeyIdentityV1,
        prepared_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        clear_init: Vec<u8>,
        clear_segment: Vec<u8>,
        requests: Mutex<Vec<DecryptProviderRequestV1>>,
    }

    impl FakeCompositeDecryptProvider {
        fn new(operation: &SignedRuntimeReleaseOperationV1) -> Self {
            Self {
                expected_issuer: operation.statement().runtime_operation_issuer(),
                recipient_public_key: operation.statement().recipient_public_key(),
                recipient_identity: operation
                    .statement()
                    .recipient_authorization()
                    .statement()
                    .recipient_identity()
                    .clone(),
                prepared_handle: opaque_handle(0x41),
                clear_init: b"clear-fmp4-init".to_vec(),
                clear_segment: b"clear-fmp4-segment".to_vec(),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn validate(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<ValidatedDecryptProviderRequestV1, RuntimeProviderCallError> {
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            let validated = ValidatedDecryptProviderRequestV1::decode_and_validate_at(
                &bytes,
                self.expected_issuer,
                NOW + 10,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.requests.lock().unwrap().push(request.clone());
            Ok(validated)
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeDecryptProvider for FakeCompositeDecryptProvider {
        async fn prepare_recipient(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let validated = self.validate(request)?;
            DecryptProviderResponseV1::new_prepared_recipient(
                validated.audit_request_id(),
                self.prepared_handle,
                self.recipient_public_key,
                &self.recipient_identity,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)
        }

        async fn open_viewer_session(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let validated = self.validate(request)?;
            if validated
                .prepared_recipient_handle()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?
                != &self.prepared_handle
            {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            DecryptProviderResponseV1::new_viewer_session_opened(
                validated.audit_request_id(),
                self.prepared_handle,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)
        }

        async fn read_viewer_media_part(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let validated = self.validate(request)?;
            if validated
                .viewer_session_handle()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?
                != &self.prepared_handle
            {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            let selector = validated
                .viewer_media_part_selector()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            let clear = if selector.is_init() {
                self.clear_init.clone()
            } else if selector.segment_index() == Some(0) {
                self.clear_segment.clone()
            } else {
                return Err(RuntimeProviderCallError::NoExactResult);
            };
            DecryptProviderResponseV1::new_viewer_media_part(
                validated.audit_request_id(),
                self.prepared_handle,
                selector.clone(),
                clear,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)
        }

        async fn cancel_prepared_recipient(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let validated = self.validate(request)?;
            DecryptProviderResponseV1::new_prepared_recipient_already_absent(
                validated.audit_request_id(),
                *validated
                    .prepared_recipient_handle()
                    .map_err(|_| RuntimeProviderCallError::NoExactResult)?,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)
        }

        async fn close_viewer_session(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let validated = self.validate(request)?;
            if validated
                .viewer_session_handle()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?
                != &self.prepared_handle
            {
                return Err(RuntimeProviderCallError::NoExactResult);
            }
            DecryptProviderResponseV1::new_closed_viewer_session(
                validated.audit_request_id(),
                self.prepared_handle,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)
        }
    }

    #[tokio::test]
    async fn inactive_two_principals_mint_verified_availability_buy_open_play_and_close() {
        let envelope = custody_envelope(0x11);
        let allowed = signed_runtime_release_operation_for(0x42, 0x11, 7, 0x30);
        let (_mint_temp, provisioned, mint_root) =
            provision_custody_provisioned_mint(&allowed, &envelope).await;
        let availability = verified_content_availability(&provisioned);
        let available = RuntimeMintJournal::new(&mint_root)
            .mark_content_available(
                provisioned.draft().mint_id(),
                &content_availability_requirement(),
                availability.clone(),
            )
            .unwrap();
        assert_eq!(available.content_availability(), Some(&availability));
        assert_eq!(
            RuntimeMintJournal::new(&mint_root)
                .mark_content_available(
                    provisioned.draft().mint_id(),
                    &content_availability_requirement(),
                    availability,
                )
                .unwrap(),
            available
        );

        let effect_a = purchase_effect_for_account(
            &available,
            &allowed,
            "profile:alpha",
            "wallet-account-alpha",
            "wallet-request:11111111111111111111111111111111",
            0xaa,
        );
        let buy = bind_buy(
            &available,
            "profile:alpha",
            profile_identity(0x26),
            &effect_a,
        )
        .unwrap();
        assert_eq!(
            bind_buy(
                &available,
                "profile:alpha",
                profile_identity(0x26),
                &effect_a
            )
            .unwrap(),
            buy
        );
        assert_eq!(
            bind_buy(
                &available,
                "profile:beta",
                profile_identity(0x26),
                &effect_a,
            ),
            Err(RuntimeOpenError::ChainEvidence)
        );

        let decrypt = FakeCompositeDecryptProvider::new(&allowed);
        let prepared = prepare_recipient(
            &decrypt,
            &buy,
            allowed
                .statement()
                .rights_request()
                .request()
                .binding()
                .runtime_session_binding(),
            allowed.statement().audit_request_id(),
            allowed.statement().runtime_operation_issuer(),
            NOW + 10,
            NOW + 40,
        )
        .await
        .unwrap();
        let (protected_init, encrypted_segments, _, _) = test_media::media_components(0x11);
        let contributions = vec![
            signed_node_contribution(&allowed, 1),
            signed_node_contribution(&allowed, 2),
        ];
        let terminal = signed_terminal_receipt(&allowed, &contributions, 0x63);
        let session = open_viewer_session(
            &decrypt,
            &RuntimeOpenViewerSessionInput {
                buy: &buy,
                prepared_recipient: &prepared,
                signed_runtime_release_operation: &allowed,
                expected_terminal_issuer: terminal.statement().issuer(),
                custody_envelope: &envelope,
                media_identity: available.draft().media_identity(),
                protected_init_segment: &protected_init,
                signed_node_contributions: &contributions,
                signed_terminal_receipt: &terminal,
                now_unix_seconds: NOW + 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            session.viewer_session_handle(),
            prepared.prepared_recipient_handle()
        );
        let init = read_viewer_media_part(
            &decrypt,
            &session,
            ViewerMediaPartSelectorV1::init(),
            NOW + 11,
        )
        .await
        .unwrap();
        let segment_selector =
            ViewerMediaPartSelectorV1::segment(0, encrypted_segments[0].clone()).unwrap();
        let segment =
            read_viewer_media_part(&decrypt, &session, segment_selector.clone(), NOW + 11)
                .await
                .unwrap();
        assert_eq!(init.audit_request_id(), session.audit_request_id());
        assert_eq!(
            init.viewer_session_handle(),
            session.viewer_session_handle()
        );
        assert_eq!(init.part_selector(), &ViewerMediaPartSelectorV1::init());
        assert_eq!(init.clear_media_part(), b"clear-fmp4-init");
        assert_eq!(segment.audit_request_id(), session.audit_request_id());
        assert_eq!(
            segment.viewer_session_handle(),
            session.viewer_session_handle()
        );
        assert_eq!(segment.part_selector(), &segment_selector);
        assert_eq!(segment.clear_media_part(), b"clear-fmp4-segment");
        assert_eq!(
            segment_selector.encrypted_segment(),
            Some(encrypted_segments[0].as_slice())
        );
        close_viewer_session(&decrypt, &session).await.unwrap();
        close_viewer_session(&decrypt, &session).await.unwrap();
        assert_eq!(decrypt.request_count(), 6);
    }
}
