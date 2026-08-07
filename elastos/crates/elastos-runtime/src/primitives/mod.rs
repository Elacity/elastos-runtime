//! Foundational runtime primitives
//!
//! These primitives MUST be in the runtime - they cannot be bypassed by any capsule.
//! They provide the security foundation for the entire system.

pub mod audit;
pub mod metrics;
pub mod time;

pub use audit::{
    verify_mandate_receipt, AuditEvent, AuditLog, ChainAttestation, MandateReceipt,
    MandateReceiptScope, MandateReceiptVerdict,
};
pub use metrics::CapsuleMetrics;
pub use time::SecureTimestamp;
