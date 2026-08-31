#![forbid(unsafe_code)]

mod cenc;
mod node_share;
mod node_store;
mod payload;
mod play;
mod possession;
mod pq_hybrid;
mod protect;
mod provision;
mod reconstruct;
mod release;
mod replay_store;
mod secrets;
mod share_wrap;

#[cfg(test)]
mod test_support;

use thiserror::Error;

pub use node_share::NodeLocalStoredShareV1;
pub use node_store::{
    NodeLocalShareReceiptV1, NodeLocalShareStoreErrorV1, NodeLocalShareStoreV1,
    ProvisionedNodeLocalShareV1,
};
pub use payload::{
    decrypt_payload_to_staging_writer_from_authenticated_operation_v1,
    seal_payload_to_staging_writer_v1, AuthenticatedChunkPayloadHeaderV1,
    AuthenticatedPayloadDecryptInputsV1, DecryptedPayloadMetadataV1, SealedPayloadMetadataV1,
    MAX_PAYLOAD_CONTENT_TYPE_BYTES_V1, PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1,
};
pub use play::{
    decrypt_validated_cenc_fmp4_segment_to_clear_v1, rewrite_validated_cenc_fmp4_init_to_clear_v1,
};
pub use possession::{
    answer_recipient_possession_challenge, issue_recipient_possession_challenge,
    mint_decrypt_session_from_seed, possession_transcript_v1, prove_recipient_possession,
    unwrap_content_key_in_decrypt_session, wrap_content_key_to_decrypt_session,
    DecryptSessionPublicKeyV1, DecryptSessionSecretKeyV1, DecryptSessionWrappedContentKeyV1,
    RecipientPossessionChallengeV1, VerifiedRecipientPossessionV1,
};
pub use pq_hybrid::{PqHybridError, SUITE_PQ_HYBRID};
pub use protect::{
    protect_validated_clear_cenc_fmp4_media_v1, protect_validated_clear_fmp4_init_to_cenc_v1,
    protect_validated_clear_fmp4_segment_to_cenc_v1,
};
pub use provision::{
    provision_custody_envelope, provision_custody_envelope_for_exact_nodes,
    ExactCustodyEnvelopeNodeV1,
};
pub(crate) use reconstruct::reconstruct_content_key_from_authenticated_operation;
pub use reconstruct::{
    reconstruct_content_key_into_decrypt_session, DecryptSessionReconstructionInputsV1,
};
pub use replay_store::DurableReplayClaimStoreV1;
pub use secrets::{
    ContentEncryptionKeyV1, NodeCustodySecretKeyV1, RecipientPublicKeyV1, RecipientSecretKeyV1,
};

use elastos_protected_content_contracts::{
    ContractError, KeyReleaseError, ReplayClaimError, RuntimeCustodyProvisioningError,
    RuntimeReleaseOperationError,
};

pub(crate) const CONTENT_KEY_BYTES: usize = 32;

#[derive(Debug, Error)]
pub enum CustodyError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Release(#[from] KeyReleaseError),
    #[error(transparent)]
    RuntimeReleaseOperation(#[from] RuntimeReleaseOperationError),
    #[error(transparent)]
    RuntimeCustodyProvisioning(#[from] RuntimeCustodyProvisioningError),
    #[error(transparent)]
    Replay(#[from] ReplayClaimError),
    #[error(transparent)]
    NodeShareStore(#[from] NodeLocalShareStoreErrorV1),
    #[error("pq-hybrid wrap failed")]
    PqHybrid(#[from] crate::pq_hybrid::PqHybridError),
    #[error("shamir operation failed: {0}")]
    Shamir(vsss_rs::Error),
    #[error("custody binding mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("malformed custody share: {0}")]
    MalformedShare(&'static str),
    #[error("reconstructed content key does not match the envelope commitment")]
    ContentKeyCommitmentMismatch,
    #[error("payload framing is invalid: {0}")]
    InvalidPayload(&'static str),
    #[error("payload I/O failed")]
    PayloadIo,
    #[error("required cryptographic randomness is unavailable")]
    RandomnessUnavailable,
}

impl From<vsss_rs::Error> for CustodyError {
    fn from(value: vsss_rs::Error) -> Self {
        Self::Shamir(value)
    }
}
