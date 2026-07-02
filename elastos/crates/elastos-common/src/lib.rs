//! Common types and utilities for ElastOS

pub mod chat_protocol;
mod error;
pub mod localhost;
mod manifest;
pub mod protected_content;
pub mod timestamp;
mod types;

pub use error::{ElastosError, Result};
pub use manifest::{
    AffordanceApprovalMode, AffordanceAuditMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleRequirement, CapsuleRole, CapsuleType,
    MicroVmConfig, Permissions, ProviderAuthority, ProviderCapabilitySchema, RequirementKind,
    ResourceLimits, SCHEMA_V1,
};
use sha2::{Digest, Sha256};
pub use timestamp::{SecureTimestamp, CLOCK_SKEW_TOLERANCE_SECS};
pub use types::{CapsuleId, CapsuleStatus};

/// Convert a Runtime principal/profile label into the non-reversible Browser
/// profile key used for profile-disk names and guest boot args.
pub fn browser_profile_key_from_value(value: &str) -> Option<String> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }
    Some(format!(
        "profile-{}",
        hex::encode(Sha256::digest(raw.as_bytes()))
    ))
}

pub fn is_safe_browser_profile_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod browser_profile_tests {
    use super::*;

    #[test]
    fn browser_profile_key_is_safe_and_stable() {
        assert_eq!(
            browser_profile_key_from_value("did:elastos:alice").as_deref(),
            Some("profile-99bb2b58175e1e062cd2fb6b1b00feec63d169f520dd0a8cfe7230517cfc43e4")
        );
        assert_eq!(
            browser_profile_key_from_value("/tmp/runtime/profile-a").as_deref(),
            Some("profile-e53af130f0beb4a2a8e376df656ce8b91a15006345f62fddff4d29f1599adf09")
        );
        assert_ne!(
            browser_profile_key_from_value("/tmp/runtime/profile-a"),
            browser_profile_key_from_value("profile-a")
        );
        assert_eq!(browser_profile_key_from_value(" \n "), None);
        assert!(is_safe_browser_profile_key("profile-a_1.ext4"));
        assert!(!is_safe_browser_profile_key("../profile"));
    }
}
