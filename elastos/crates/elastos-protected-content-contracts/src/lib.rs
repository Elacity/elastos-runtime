//! Canonical protected-content v1 contracts.
//!
//! This source-only crate defines identities and provider-private messages. It
//! does not select providers, transport requests, hold key shares, reconstruct
//! content keys, or expose capsule workflows.

mod canonical;
mod custody_envelope;
mod custody_epoch;
mod identity;
mod node_contribution;
mod node_decision;
mod node_set;
mod policy;
mod recipient_authorization;
mod release_request;
mod replay;
mod rights;
mod rights_receipt;
mod runtime_release_operation;
mod terminal_receipt;

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod test_support;

pub use canonical::{CanonicalContract, ContractError};
pub use custody_envelope::{
    CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyNodeIdentityV1, HpkeCiphertextV1,
    NodeCustodyPublicKeyV1, ShareCoordinateV1, CONTENT_KEY_COMMITMENT_DOMAIN_V1,
    CUSTODY_HPKE_SUITE_ID_V1, HPKE_ENCAPPED_KEY_BYTES, HPKE_SEALED_SHARE_BYTES,
    RELEASED_SHARE_HPKE_INFO_V1, STORED_SHARE_HPKE_INFO_V1,
};
pub use custody_epoch::{
    CustodyApprovedSuitesV1, CustodyEpochError, CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1,
    CustodyEpochStatementV1, SignedCustodyEpochV1, VerifiedCustodyEpochV1,
};
pub use identity::{
    Digest32, EncryptedContentIdentityV1, KeyEnvelopeIdentityV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, RightsPolicyIdentityV1,
    RuntimeSessionBindingV1, ThresholdV1, WalletAddress, MAX_ENCRYPTED_CONTENT_BYTES,
    MAX_KEY_ENVELOPE_BYTES, MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES, MAX_RIGHTS_POLICY_BYTES,
    MAX_THRESHOLD_NODES,
};
pub use node_contribution::{
    validate_node_contribution_active_window, NodeContributionStatementV1,
    RecipientSealedContributionV1, SignedNodeContributionV1, VerifiedNodeContributionV1,
    MAX_NODE_CONTRIBUTION_LIFETIME_SECS, MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES,
};
pub use node_decision::{
    NodeRightsDecisionStatementV1, SignedNodeRightsDecisionV1, VerifiedNodeRightsDecisionV1,
    MAX_NODE_DECISION_LIFETIME_SECS,
};
pub use node_set::{NodePublicKey, NodeSetV1};
pub use policy::{
    EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1,
    RightsEvaluationEvidenceRequestV1, RightsEvaluationEvidenceV1, RightsObservationFinalityV1,
    RightsPolicyBodyV1, RightsSubjectSourceV1,
};
pub use recipient_authorization::{
    RecipientAuthorizationError, RecipientKeyAuthorizationContextV1,
    RecipientKeyAuthorizationStatementV1, RecipientPublicKeyBytesV1, RuntimeOperationIssuerKeyV1,
    SignedRecipientKeyAuthorizationV1, VerifiedRecipientKeyAuthorizationV1,
    MAX_RECIPIENT_KEY_AUTHORIZATION_LIFETIME_SECS,
};
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
pub use runtime_release_operation::{
    AuthenticatedRuntimeReleaseOperationV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationError,
    RuntimeReleaseOperationStatementV1, SignedRuntimeReleaseOperationV1,
    MAX_RUNTIME_RELEASE_OPERATION_LIFETIME_SECS,
};
pub use terminal_receipt::{
    KeyReleaseOutcomeV1, NodeContributionRefV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    TerminalReceiptStatementV1, MAX_TERMINAL_RECEIPT_LIFETIME_SECS,
};
