//! Internal protected-content Runtime coordination foundation.
//!
//! This crate is deliberately source-only and unregistered. It owns durable
//! Runtime operation state and typed Runtime-to-provider seams; it does not
//! expose CEKs, shares, routes, endpoints, Carrier topology, Library UI, or
//! product cutover behavior.

mod journal;

pub use journal::{
    PersistedRuntimeReleaseOperation, RuntimeReleaseJournal, RuntimeReleaseJournalError,
    RuntimeReleaseOperationDraft, RuntimeReleaseTerminalResult,
};
