//! Canonical protected-content v1 contracts.
//!
//! This source-only crate defines identities and provider-private messages. It
//! does not select providers, transport requests, hold key shares, reconstruct
//! content keys, or expose capsule workflows.

mod canonical;
mod identity;
mod node_set;

pub use canonical::{CanonicalContract, ContractError};
pub use identity::{
    Digest32, EncryptedContentIdentityV1, KeyEnvelopeIdentityV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, RightsPolicyIdentityV1,
    RuntimeSessionBindingV1, ThresholdV1, WalletAddress, MAX_ENCRYPTED_CONTENT_BYTES,
    MAX_KEY_ENVELOPE_BYTES, MAX_RECIPIENT_ENCRYPTION_SUITE_ID_BYTES, MAX_RIGHTS_POLICY_BYTES,
    MAX_THRESHOLD_NODES,
};
pub use node_set::{NodePublicKey, NodeSetV1};
