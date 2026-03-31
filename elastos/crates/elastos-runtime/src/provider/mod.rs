//! Provider registry for protocol routing
//!
//! Routes resource requests to appropriate providers based on URL scheme:
//! - `localhost://<file-backed-root>/...` -> Local sovereign PC2 state
//! - `elastos://...` -> Decentralized content and service namespaces
//!
//! Providers are registered at startup and can be dynamically added/removed.

pub mod bridge;
mod registry;

pub use bridge::{CapsuleProvider, ProviderBridge, ProviderConfig as BridgeProviderConfig};
pub use registry::{
    EntryType, Provider, ProviderError, ProviderRegistry, ResourceAction, ResourceResponse,
};

// Re-export for use by external provider implementations
#[allow(unused_imports)]
pub use registry::{ResourceEntry, ResourceRequest};
