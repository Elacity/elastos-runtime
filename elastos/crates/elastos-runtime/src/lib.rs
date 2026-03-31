//! ElastOS Runtime Library
//!
//! This library provides the core runtime components for ElastOS:
//! - Bootstrap: Runtime initialization and lifecycle
//! - Capability: Token-based access control
//! - Capsule: Capsule lifecycle management
//! - Handler: Message handling between capsules and runtime
//! - Messaging: Inter-capsule communication
//! - Provider: Protocol routing and resource providers
//! - Primitives: Core types (time, audit, metrics)
//! - Session: Session management and authentication
//!
//! Content resolution and namespace mapping live in the `elastos-namespace` crate.
//! The HTTP API, CLI, and capsule loading logic live in the `elastos-server` crate.
//! This library is transport-agnostic — it has no HTTP framework dependencies.

pub mod bootstrap;
pub mod capability;
pub mod capsule;
pub mod handler;
pub mod messaging;
pub mod primitives;
pub mod provider;
pub mod session;
pub mod signature;

// Re-export namespace types for backward compatibility
pub use elastos_namespace as namespace;
pub use elastos_namespace as content;
