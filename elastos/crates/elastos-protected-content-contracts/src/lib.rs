//! Canonical protected-content v1 contracts.
//!
//! This source-only crate defines identities and provider-private messages. It
//! does not select providers, transport requests, hold key shares, reconstruct
//! content keys, or expose capsule workflows.

mod canonical;
mod identity;
mod node_contribution;
mod node_decision;
mod node_set;
mod release_request;
mod replay;
mod rights;
mod rights_receipt;
mod terminal_receipt;

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod test_support;

pub use canonical::{CanonicalContract, ContractError};
pub use identity::{
    Digest32, EncryptedContentIdentityV1, KeyEnvelopeIdentityV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, RightsPolicyIdentityV1,
    RuntimeSessionBindingV1, ThresholdV1, WalletAddress, MAX_ENCRYPTED_CONTENT_BYTES,
    MAX_KEY_ENVELOPE_BYTES, MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES, MAX_RIGHTS_POLICY_BYTES,
    MAX_THRESHOLD_NODES,
};
pub use node_contribution::{
    NodeContributionStatementV1, RecipientSealedContributionV1, SignedNodeContributionV1,
    VerifiedNodeContributionV1, MAX_NODE_CONTRIBUTION_LIFETIME_SECS,
    MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES,
};
pub use node_decision::{
    NodeRightsDecisionStatementV1, SignedNodeRightsDecisionV1, VerifiedNodeRightsDecisionV1,
    MAX_NODE_DECISION_LIFETIME_SECS,
};
pub use node_set::{NodePublicKey, NodeSetV1};
pub use release_request::{
    KeyReleaseError, KeyReleaseRequestV1, VerifiedKeyReleaseRequestV1,
    MAX_RELEASE_REQUEST_LIFETIME_SECS,
};
pub use replay::{AtomicReplayClaimer, ReplayClaimError, ReplayClaimKeyV1};
pub use rights::{
    ReplayNonce16, RightsActionV1, RightsDecisionV1, RightsError, RightsRequestV1,
    RightsVerificationContextV1, VerifiedRightsRequestV1, WalletSignedRightsRequestV1,
    MAX_RIGHTS_REQUEST_LIFETIME_SECS, RIGHTS_CLOCK_SKEW_SECS,
};
pub use rights_receipt::{
    RightsReceiptIssuerKey, RightsReceiptStatementV1, SignedRightsReceiptV1,
    MAX_RIGHTS_RECEIPT_LIFETIME_SECS,
};
pub use terminal_receipt::{
    KeyReleaseOutcomeV1, NodeContributionRefV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    TerminalReceiptStatementV1, MAX_TERMINAL_RECEIPT_LIFETIME_SECS,
};
