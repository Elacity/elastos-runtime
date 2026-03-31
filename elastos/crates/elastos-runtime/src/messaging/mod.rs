//! Inter-capsule messaging
//!
//! Provides secure, capability-controlled messaging between capsules.
//! All messages are routed through the runtime - no direct capsule-to-capsule
//! communication is allowed.
//!
//! Security properties:
//! - Messages require valid capability tokens
//! - All message delivery is audited
//! - Rate limiting per capsule
//! - Message size limits

mod channel;

#[allow(unused_imports)]
pub use channel::{Message, MessageChannel, MessageId};
