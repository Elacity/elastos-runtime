//! Compute abstraction layer for ElastOS

mod traits;

pub mod providers;

pub use traits::{CapsuleHandle, CapsuleInfo, ComputeProvider};
