//! Internal protected-content Runtime coordination foundation.
//!
//! This crate is deliberately source-only. It owns durable Runtime release and
//! mint journal state plus typed Runtime-to-provider seams. It does not expose
//! CEKs, shares, routes, endpoints, Carrier topology, or Library UI.
//! Registration of the Runtime-only provider targets lives in elastos-server.

mod coordinator;
mod journal;
mod mint;
mod mint_journal;
mod open;
#[cfg(test)]
mod test_media;
#[cfg(test)]
mod test_object;

pub use coordinator::{
    wallet_rights_signature_result, RuntimeCustodyProvider, RuntimeProviderCallError,
    RuntimeReleaseCoordinator, RuntimeReleaseCoordinatorError, RuntimeReleaseCoordinatorOutcome,
    RuntimeReleaseNonterminalReason, RuntimeReleaseReconcileOffer, RuntimeRightsProvider,
    RuntimeSelectedProvider,
};
pub use journal::{
    PersistedRuntimeReleaseOperation, RuntimeReleaseAuditPhase, RuntimeReleaseAuditRecord,
    RuntimeReleaseJournal, RuntimeReleaseJournalError, RuntimeReleaseOperationDraft,
    RuntimeReleaseTerminalResult,
};
pub use mint::{
    resolve_runtime_mint_selected_nodes, RuntimeMintConfiguredCustodyProvider,
    RuntimeMintCoordinator, RuntimeMintCoordinatorError, RuntimeMintCoordinatorOutcome,
    RuntimeMintNonterminalReason, RuntimeMintSelectedNode,
};
pub use mint_journal::{
    ExclusiveFileLock, PersistedRuntimeMint, RuntimeContentAvailabilityRequirement,
    RuntimeContentIdentityV1, RuntimeCustodyTerminalKind, RuntimeMediaPreparationRecord,
    RuntimeMediaPreparationState, RuntimeMintCreatorDesiredTerms, RuntimeMintCreatorEffectBinding,
    RuntimeMintCreatorState, RuntimeMintCreatorTerminalEvidence, RuntimeMintDraft,
    RuntimeMintIntent, RuntimeMintIntentContentV1, RuntimeMintJournal, RuntimeMintJournalError,
    RuntimeMintNodeBinding, RuntimeMintNodeReceipt, RuntimeVerifiedContentAvailability,
    RuntimeVerifiedContentIdentityRootV1,
};
pub use open::{
    bind_buy, cancel_prepared_recipient, cancel_prepared_recipient_with_result_by_handle,
    close_viewer_session, close_viewer_session_with_result, open_viewer_session, prepare_recipient,
    read_viewer_media_part, read_viewer_object_chunk, reject_bearer_playback, RuntimeBuyReceipt,
    RuntimeDecryptProvider, RuntimeOpenError, RuntimeOpenViewerContentV1,
    RuntimeOpenViewerSessionInput, RuntimePreparedRecipient, RuntimePreparedRecipientCancelResult,
    RuntimeProtectedContentPurchaseIntent, RuntimePurchaseEffectAuthority,
    RuntimeVerifiedPurchaseEffect, RuntimeViewerMediaPart, RuntimeViewerObjectChunk,
    RuntimeViewerSession, RuntimeViewerSessionCloseResult,
};
