//! Message handler for capsule-to-runtime communication
//!
//! This module processes RuntimeRequest messages from capsules and returns
//! RuntimeResponse messages. It enforces authorization (only the shell can
//! perform orchestrator operations) and delegates to the appropriate managers.

mod io_bridge;
mod protocol;
mod request_handler;

#[allow(unused_imports)]
pub use io_bridge::CapsuleIoBridge;
#[allow(unused_imports)]
pub use protocol::*;
#[allow(unused_imports)]
pub use request_handler::*;
