//! Foundational runtime primitives
//!
//! These primitives MUST be in the runtime - they cannot be bypassed by any capsule.
//! They provide the security foundation for the entire system.

pub mod audit;
pub mod metrics;
pub mod spend;
pub mod time;

#[allow(unused_imports)]
pub use audit::{AuditEvent, AuditLog, ChainAttestation};
#[allow(unused_imports)]
pub use metrics::CapsuleMetrics;
#[allow(unused_imports)]
pub use spend::{BudgetSnapshot, SpendError, SpendMeter, SpendUnits};
#[allow(unused_imports)]
pub use time::SecureTimestamp;
