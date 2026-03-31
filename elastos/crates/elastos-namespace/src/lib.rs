//! ElastOS Namespace
//!
//! Content-addressed storage resolution and namespace (path → CID) mapping.
//!
//! This crate is transport-agnostic — content fetching is injected via the
//! `ContentFetcher` trait. Audit logging is injected via the `AuditSink` trait.

mod namespace;
mod resolver;
mod store;

pub use namespace::{ContentId, Namespace, NamespaceEntry, NamespaceError};
pub use resolver::{
    AuditSink, ContentFetcher, ContentResolver, ContentUri, FetchError, FetchResult, FetchSource,
    NullAuditSink, NullFetcher, ResolverConfig,
};
pub use store::*;
