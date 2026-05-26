//! Compatibility re-export for shared Runtime authority primitives.
//!
//! The concrete proof/session types live in `elastos-auth` so provider capsules
//! can share the same verification logic without depending on the full runtime
//! execution stack.

pub use elastos_auth::*;
