//! Internal runtime-control handler.
//!
//! This module processes `RuntimeRequest` messages for shell control, the
//! legacy stdio bridge, and explicitly authorized internal flows. It is not the
//! public capsule-kernel ABI exposed by `elastos-guest`.

mod io_bridge;
mod protocol;
mod request_handler;

#[allow(unused_imports)]
pub use io_bridge::CapsuleIoBridge;
#[allow(unused_imports)]
pub use protocol::*;
#[allow(unused_imports)]
pub use request_handler::*;
