#![forbid(unsafe_code)]

mod hpke_helpers;
mod payload;
mod provision;
mod reconstruct;
mod release;
mod replay_store;
mod secrets;

#[cfg(test)]
mod test_support;

use thiserror::Error;

pub use payload::{
    decrypt_payload_to_staging_writer_from_authenticated_operation_v1,
    seal_payload_to_staging_writer_v1, AuthenticatedChunkPayloadHeaderV1,
    AuthenticatedPayloadDecryptInputsV1, DecryptedPayloadMetadataV1, SealedPayloadMetadataV1,
    MAX_PAYLOAD_CONTENT_TYPE_BYTES_V1, PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1,
};
pub use provision::provision_custody_envelope;
pub use reconstruct::reconstruct_content_key_from_authenticated_operation;
pub use replay_store::DurableReplayClaimStoreV1;
pub use secrets::{
    ContentEncryptionKeyV1, NodeCustodySecretKeyV1, RecipientPublicKeyV1, RecipientSecretKeyV1,
};

use elastos_protected_content_contracts::{
    ContractError, KeyReleaseError, ReplayClaimError, RuntimeReleaseOperationError,
};

pub(crate) const CONTENT_KEY_BYTES: usize = 32;
pub(crate) const RELEASED_SHARE_TAG_BYTES: usize = 16;

#[derive(Debug, Error)]
pub enum CustodyError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Release(#[from] KeyReleaseError),
    #[error(transparent)]
    RuntimeReleaseOperation(#[from] RuntimeReleaseOperationError),
    #[error(transparent)]
    Replay(#[from] ReplayClaimError),
    #[error("hpke operation failed")]
    Hpke(#[from] hpke::HpkeError),
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
