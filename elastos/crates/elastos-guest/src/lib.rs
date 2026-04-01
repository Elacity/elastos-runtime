//! ElastOS Guest SDK
//!
//! This crate provides APIs for capsule developers to interact with the ElastOS runtime.
//!
//! # Example
//!
//! ```rust,ignore
//! use elastos_guest::prelude::*;
//!
//! fn main() {
//!     // Get capsule information
//!     let info = CapsuleInfo::from_env();
//!     println!("Running as capsule: {}", info.name());
//!     println!("Capsule ID: {}", info.id());
//!
//!     // With runtime feature, communicate with the runtime
//!     #[cfg(feature = "serde")]
//!     {
//!         let mut client = RuntimeClient::new();
//!         let capsules = client.list_capsules().unwrap();
//!         for cap in capsules {
//!             println!("  {} - {}", cap.id, cap.name);
//!         }
//!     }
//! }
//! ```

/// Runtime communication module (requires "serde" feature)
#[cfg(feature = "serde")]
pub mod runtime;

/// Prelude module with common imports
pub mod prelude {
    pub use crate::CapsuleInfo;
    pub use crate::{capsule_id, capsule_name};
    pub use crate::{log, log_error};

    // Re-export runtime types when serde is enabled
    #[cfg(feature = "serde")]
    pub use crate::runtime::{
        CapabilityConstraints, CapsuleListEntry, IncomingMessage, LaunchConfig, RuntimeClient,
        RuntimeRequest, RuntimeResponse,
    };
}

/// Information about the running capsule
#[derive(Debug, Clone, Default)]
pub struct CapsuleInfo {
    name: String,
    id: String,
}

impl CapsuleInfo {
    /// Create CapsuleInfo from environment variables
    ///
    /// Reads ELASTOS_CAPSULE_NAME and ELASTOS_CAPSULE_ID from the environment.
    pub fn from_env() -> Self {
        Self {
            name: std::env::var("ELASTOS_CAPSULE_NAME").unwrap_or_default(),
            id: std::env::var("ELASTOS_CAPSULE_ID").unwrap_or_default(),
        }
    }

    /// Create CapsuleInfo with known values
    pub fn new(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: id.into(),
        }
    }

    /// Get the capsule name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the capsule ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Check if running inside ElastOS runtime
    pub fn is_elastos_runtime(&self) -> bool {
        !self.name.is_empty() || !self.id.is_empty()
    }
}

/// Get the current capsule name from the environment
///
/// Returns the value of ELASTOS_CAPSULE_NAME or an empty string if not set.
pub fn capsule_name() -> String {
    std::env::var("ELASTOS_CAPSULE_NAME").unwrap_or_default()
}

/// Get the current capsule ID from the environment
///
/// Returns the value of ELASTOS_CAPSULE_ID or an empty string if not set.
pub fn capsule_id() -> String {
    std::env::var("ELASTOS_CAPSULE_ID").unwrap_or_default()
}

/// Check if running inside ElastOS runtime
pub fn is_elastos_runtime() -> bool {
    std::env::var("ELASTOS_CAPSULE_NAME").is_ok() || std::env::var("ELASTOS_CAPSULE_ID").is_ok()
}

/// ElastOS SDK / capsule build version.
///
/// Release builds can stamp `ELASTOS_RELEASE_VERSION` so capsules report the
/// same human-facing version as the runtime. Local builds fall back to the
/// package version with a `-dev` suffix to make drift obvious.
pub const VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Log a message to the runtime
///
/// Uses stdout which routes through WASI.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        print!("[elastos] ");
        println!($($arg)*);
    };
}

/// Log an error message to the runtime
///
/// Uses stderr which routes through WASI.
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        eprint!("[elastos:error] ");
        eprintln!($($arg)*);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capsule_info() {
        let info = CapsuleInfo::new("test", "test-123");
        assert_eq!(info.name(), "test");
        assert_eq!(info.id(), "test-123");
        assert!(info.is_elastos_runtime());
    }

    #[test]
    fn test_capsule_info_default() {
        let info = CapsuleInfo::default();
        assert!(info.name().is_empty());
        assert!(info.id().is_empty());
        assert!(!info.is_elastos_runtime());
    }

    #[test]
    fn test_version() {
        assert_ne!(VERSION, "");
    }
}
