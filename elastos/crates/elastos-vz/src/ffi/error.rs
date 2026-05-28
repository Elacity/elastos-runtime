//! `NSError` → Rust `String` helper.
//!
//! Apple's Virtualization.framework reports failures as
//! `NSError` instances with a localised description. We surface
//! those through normal Rust `Result<_, String>` so the calling
//! supervisor logs read identically to the crosvm path
//! (`elastos-crosvm` already returns `Result<_, String>`-shaped
//! errors via `ElastosError::Compute(String)`).
//!
//! Reused by every Vz wrapper in this module that calls an
//! `init…_error` initialiser or `validateWithError`.

#![cfg(target_os = "macos")]

use objc2_foundation::NSError;

/// Render an `NSError` as a human-readable string.
///
/// We deliberately use `localizedDescription` rather than the
/// `description` selector because Apple's docs guarantee the
/// localised string is non-nil for every framework error; the
/// raw description is best-effort.
pub(crate) fn ns_error_to_string(err: &NSError) -> String {
    err.localizedDescription().to_string()
}
