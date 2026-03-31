//! Capability token system for ElastOS
//!
//! This module implements the cryptographic capability token system that
//! controls all resource access in ElastOS. Every action requires a valid
//! capability token signed by the runtime.

pub mod evaluator;
pub mod manager;
pub mod pending;
pub mod policy;
pub mod store;
pub mod token;

#[allow(unused_imports)]
pub use evaluator::PolicyEvaluator;
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
pub use store::CapabilityStore;
#[allow(unused_imports)]
pub use token::{Action, CapabilityToken, ResourceId, TokenConstraints};
