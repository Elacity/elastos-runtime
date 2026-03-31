//! Common types and utilities for ElastOS

pub mod chat_protocol;
mod error;
pub mod localhost;
mod manifest;
pub mod timestamp;
mod types;

pub use error::{ElastosError, Result};
pub use manifest::{
    CapsuleManifest, CapsuleRequirement, CapsuleType, MicroVmConfig, Permissions, RequirementKind,
    ResourceLimits, SCHEMA_V1,
};
pub use timestamp::{SecureTimestamp, CLOCK_SKEW_TOLERANCE_SECS};
pub use types::{CapsuleId, CapsuleStatus};
