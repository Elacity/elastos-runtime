//! Common types and utilities for ElastOS

pub mod canonical_hash;
pub mod chat_protocol;
mod error;
pub mod localhost;
mod manifest;
pub mod protected_content;
pub mod reach;
pub mod timestamp;
mod types;

pub use canonical_hash::canonical_input_hash;
pub use error::{ElastosError, Result};
pub use manifest::{
    AffordanceApprovalMode, AffordanceAuditMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleRequirement, CapsuleRole, CapsuleType,
    MicroVmConfig, Permissions, ProviderAuthority, ProviderCapabilitySchema, RequirementKind,
    ResourceLimits, SCHEMA_V1,
};
pub use reach::{
    EgressAllowlist, EgressReach, IsolationTier, ReachDescriptorV1, ResourceScope, Reversibility,
    REACH_DESCRIPTOR_SCHEMA_V1,
};
pub use timestamp::{SecureTimestamp, CLOCK_SKEW_TOLERANCE_SECS};
pub use types::{CapsuleId, CapsuleStatus};
