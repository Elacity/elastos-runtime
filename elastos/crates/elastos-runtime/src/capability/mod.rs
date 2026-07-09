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

pub use evaluator::PolicyEvaluator;
pub use intent::{
    check_intent_freshness, check_intent_within_envelope, count_intent_proof,
    dispatch_standing_act, reconcile, run_intent_gate, EnvelopeCheck, EnvelopeDenial,
    FreshnessError, IntentDeclarationV1, IntentGateOutcome, IntentProofSummary,
    IntentReconciliationV1, ReconciliationStatus, StandingGrantEnvelope, StandingGrantService,
    StandingGrantStore, INTENT_DECLARATION_SCHEMA_V1, INTENT_RECONCILIATION_SCHEMA_V1,
    MANDATE_DISPATCH_LIMIT, MANDATE_DISPATCH_LIMIT_MAX, MANDATE_DISPATCH_WINDOW_SECS,
    MAX_CLOCK_SKEW_SECS, MAX_INTENT_AGE_SECS,
};
pub use manager::CapabilityManager;
pub use pending::{GrantDuration, PendingRequestStore, RequestStatus};
pub use policy::{
    AutoGrantVerifier, DecisionId, GrantProposal, PolicyDecision, PolicyOutcome, PolicyRule,
    PolicyVerifier, ProposedConstraints, RuleCheck, RulesVerifier, VerifierCheck,
};
pub use receipt::{AffordanceGrantReceiptV1, AFFORDANCE_RECEIPT_SCHEMA_V1};
pub use store::CapabilityStore;
pub use token::{Action, CapabilityToken, ResourceId, TokenConstraints};

/// Longest `responsible_entity` the signed chain accepts.
pub const RESPONSIBLE_ENTITY_MAX_LEN: usize = 256;

/// The service-layer syntactic bound on a responsible-entity string (council S32 F6): ≤256 chars
/// of DID charset `{alnum : . - _ %}`. Re-applied at EVERY choke point that writes the value
/// verbatim onto the signed chain, so a non-HTTP caller cannot smuggle an unbounded/hostile blob.
/// SYNTACTIC only — never authentication (the HTTP surface layers a stricter `did:` shape check
/// on top). One shared home so the charset cannot drift between choke points.
pub fn responsible_entity_syntax_ok(entity: &str) -> bool {
    entity.len() <= RESPONSIBLE_ENTITY_MAX_LEN
        && entity
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '-' | '_' | '%'))
}
