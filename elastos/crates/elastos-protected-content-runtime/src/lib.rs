//! Internal protected-content Runtime coordination foundation.
//!
//! This crate is deliberately source-only. It owns durable Runtime release and
//! mint journal state plus typed Runtime-to-provider seams. It does not expose
//! CEKs, shares, routes, endpoints, Carrier topology, Library UI, or product
//! cutover behavior. Provider registration stays inactive and must not replace
//! the provisional `key`/`rights` product routes.

mod coordinator;
mod journal;
mod mint;
mod mint_journal;
mod open;
#[cfg(test)]
mod test_media;

pub use coordinator::{
    RuntimeCustodyProvider, RuntimeProviderCallError, RuntimeReleaseCoordinator,
    RuntimeReleaseCoordinatorError, RuntimeReleaseCoordinatorOutcome,
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
    PersistedRuntimeMint, RuntimeContentAvailabilityRequirement, RuntimeCustodyTerminalKind,
    RuntimeMintDraft, RuntimeMintIntent, RuntimeMintJournal, RuntimeMintJournalError,
    RuntimeMintNodeBinding, RuntimeMintNodeReceipt, RuntimeVerifiedContentAvailability,
};
pub use open::{
    bind_buy, cancel_prepared_recipient, close_viewer_session, open_viewer_session,
    prepare_recipient, read_viewer_media_part, reject_bearer_playback, RuntimeBuyReceipt,
    RuntimeDecryptProvider, RuntimeOpenError, RuntimeOpenViewerSessionInput,
    RuntimePreparedRecipient, RuntimeProtectedContentPurchaseIntent,
    RuntimePurchaseEffectAuthority, RuntimeVerifiedPurchaseEffect, RuntimeViewerMediaPart,
    RuntimeViewerSession,
};
