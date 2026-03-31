//! Capsule lifecycle management
//!
//! Manages capsule instances with:
//! - Unique ID generation
//! - Lifecycle tracking (running, stopped, suspended)
//! - Integration with capability tokens
//! - Metrics and audit logging
//! - Memory clearing on termination (security)

mod fetched_launch;
mod manager;

pub use fetched_launch::{prepare_fetched_capsule, PreparedFetchedCapsule};
#[allow(unused_imports)]
pub use manager::{CapsuleId, CapsuleInfo, CapsuleManager, CapsuleState};
