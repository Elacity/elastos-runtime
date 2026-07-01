//! Capability token system for ElastOS
//!
//! This module implements the cryptographic capability token system that
//! controls all resource access in ElastOS. Every action requires a valid
//! capability token signed by the runtime.

pub mod evaluator;
pub mod intent;
pub mod manager;
pub mod pending;
pub mod policy;
pub mod receipt;
pub mod store;
pub mod token;

#[allow(unused_imports)]
pub use evaluator::PolicyEvaluator;
#[allow(unused_imports)]
pub use intent::{
    check_intent_within_envelope, count_intent_proof, reconcile, run_intent_gate, EnvelopeCheck,
    EnvelopeDenial, IntentDeclarationV1, IntentGateOutcome, IntentProofSummary,
    IntentReconciliationV1, ReconciliationStatus, StandingGrantEnvelope, StandingGrantStore,
    INTENT_DECLARATION_SCHEMA_V1, INTENT_RECONCILIATION_SCHEMA_V1,
};
#[allow(unused_imports)]
pub use manager::CapabilityManager;
#[allow(unused_imports)]
pub use pending::{GrantDuration, PendingRequestStore, RequestStatus};
#[allow(unused_imports)]
pub use policy::{
    AutoGrantVerifier, DecisionId, GrantProposal, PolicyDecision, PolicyOutcome, PolicyRule,
    PolicyVerifier, ProposedConstraints, RuleCheck, RulesVerifier, VerifierCheck,
};
#[allow(unused_imports)]
pub use receipt::{AffordanceGrantReceiptV1, AFFORDANCE_RECEIPT_SCHEMA_V1};
#[allow(unused_imports)]
pub use store::CapabilityStore;
#[allow(unused_imports)]
pub use token::{Action, CapabilityToken, ResourceId, TokenConstraints};
