//! `VZGenericPlatformConfiguration` + persistent
//! `VZGenericMachineIdentifier`.
//!
//! Phase 0 §D pitfall #2 (`docs/vz-backend/PHASE_0_SCOPE.md`): a
//! generic platform's `machineIdentifier` is the closest thing Vz
//! exposes to a stable hardware fingerprint. Apple's docs are
//! explicit that re-attaching a saved state to a different
//! identifier is undefined behaviour, and that running two VMs
//! with the same identifier concurrently is equally undefined. The
//! capsule-level expectation is "this VM keeps the same identity
//! across reboots so anything that reads
//! `/sys/class/dmi/.../product_uuid` stays consistent."
//!
//! We meet that contract with a tiny on-disk artifact:
//! `<state_dir>/<vm_id>/identifier.bin`. The probe verified Vz
//! produces a 70-byte serialised form; we treat any non-empty
//! blob as opaque and round-trip it through
//! `dataRepresentation` / `initWithDataRepresentation`.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::NSData;
use objc2_virtualization::{VZGenericMachineIdentifier, VZGenericPlatformConfiguration};

/// Per-VM identifier filename, relative to `<state_dir>/<vm_id>/`.
///
/// Kept as a module-level constant so the supervisor, the eventual
/// `vm-debug` CLI, and the tests below all agree on the location.
pub(crate) const IDENTIFIER_FILE: &str = "identifier.bin";

/// Result of resolving (and possibly creating) the per-VM
/// `VZGenericPlatformConfiguration`.
#[derive(Debug)]
pub(crate) struct BuiltPlatform {
    /// Configured platform ready to attach to
    /// `VZVirtualMachineConfiguration::setPlatform`.
    pub(crate) config: Retained<VZGenericPlatformConfiguration>,
    /// Resolved on-disk path to the identifier blob. Useful for
    /// log lines and the (future) `elastos doctor` view.
    #[allow(dead_code)]
    pub(crate) identifier_path: PathBuf,
}

/// Build a `VZGenericPlatformConfiguration` for `vm_id` rooted at
/// `state_dir`.
///
/// - If `<state_dir>/<vm_id>/identifier.bin` exists and decodes
///   into a valid identifier, the saved identifier is used so the
///   guest sees stable hardware UUIDs across reboots.
/// - Otherwise a fresh `VZGenericMachineIdentifier` is generated
///   and persisted before being attached.
///
/// Returns `Err(String)` for filesystem failures or for a corrupt
/// identifier blob (the latter is logged with the offending path).
pub(crate) fn build_platform(state_dir: &Path, vm_id: &str) -> Result<BuiltPlatform, String> {
    let vm_state_dir = state_dir.join(vm_id);
    fs::create_dir_all(&vm_state_dir).map_err(|e| {
        format!(
            "platform: could not create VM state dir at {}: {e}",
            vm_state_dir.display()
        )
    })?;
    let identifier_path = vm_state_dir.join(IDENTIFIER_FILE);

    let identifier = load_or_create_identifier(&identifier_path)?;

    // SAFETY: `VZGenericPlatformConfiguration::new()` allocates and
    // initialises a generic platform; we then set the identifier
    // via Apple's documented copy-property setter.
    let platform = unsafe { VZGenericPlatformConfiguration::new() };
    unsafe { platform.setMachineIdentifier(&identifier) };

    Ok(BuiltPlatform {
        config: platform,
        identifier_path,
    })
}

fn load_or_create_identifier(path: &Path) -> Result<Retained<VZGenericMachineIdentifier>, String> {
    if path.exists() {
        let bytes = fs::read(path).map_err(|e| {
            format!(
                "platform: could not read existing identifier at {}: {e}",
                path.display()
            )
        })?;

        if !bytes.is_empty() {
            if let Some(id) = identifier_from_bytes(&bytes) {
                return Ok(id);
            }
            // Corrupt or zero-byte file — surface that explicitly
            // so the operator can rotate the identifier rather
            // than silently regenerating (which would change the
            // guest's stable IDs).
            return Err(format!(
                "platform: identifier file at {} is unreadable as a Vz machine identifier; \
                 remove it manually to mint a fresh one (the guest will see new hardware IDs)",
                path.display()
            ));
        }
    }

    // SAFETY: `VZGenericMachineIdentifier::new()` allocates and
    // initialises a fresh identifier; no other thread can have a
    // reference to it yet.
    let fresh = unsafe { VZGenericMachineIdentifier::new() };
    let data = unsafe { fresh.dataRepresentation() };
    let serialised = data.to_vec();

    if serialised.is_empty() {
        return Err(format!(
            "platform: VZGenericMachineIdentifier serialised to 0 bytes (path: {})",
            path.display()
        ));
    }

    fs::write(path, &serialised).map_err(|e| {
        format!(
            "platform: could not persist identifier to {}: {e}",
            path.display()
        )
    })?;

    Ok(fresh)
}

fn identifier_from_bytes(bytes: &[u8]) -> Option<Retained<VZGenericMachineIdentifier>> {
    // NSData copies the slice contents on construction, so the
    // original Rust `Vec<u8>` is free to drop afterwards.
    let data = NSData::with_bytes(bytes);
    // SAFETY: `initWithDataRepresentation` is Apple's documented
    // round-trip from `dataRepresentation`. It returns nil for
    // corrupt input, which becomes `Option::None` here.
    unsafe {
        VZGenericMachineIdentifier::initWithDataRepresentation(
            VZGenericMachineIdentifier::alloc(),
            &data,
        )
    }
}

/// Diagnostic helper for the (future) `elastos doctor` view —
/// not used by Phase 2 main yet. Renders the on-disk path Vz will
/// read for `vm_id` without actually loading or generating
/// anything.
#[allow(dead_code)]
pub(crate) fn identifier_path_for(state_dir: &Path, vm_id: &str) -> PathBuf {
    state_dir.join(vm_id).join(IDENTIFIER_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn identifier_persists_across_calls() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();
        let vm_id = "phase2-day2-test";

        let first = build_platform(&state_dir, vm_id).expect("first build_platform succeeds");
        let first_bytes = fs::read(&first.identifier_path).expect("identifier file exists");
        assert!(
            !first_bytes.is_empty(),
            "identifier file must be non-empty after first build_platform"
        );

        // Drop the Retained handle so any subsequent call has to
        // re-read from disk rather than re-using an in-process
        // identifier.
        drop(first);

        let second = build_platform(&state_dir, vm_id).expect("second build_platform succeeds");
        let second_bytes = fs::read(&second.identifier_path).expect("identifier file exists");

        assert_eq!(
            first_bytes, second_bytes,
            "second build_platform must reuse the persisted identifier"
        );
    }

    #[test]
    fn different_vm_ids_get_different_identifiers() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();

        let a = build_platform(&state_dir, "vm-a").unwrap();
        let b = build_platform(&state_dir, "vm-b").unwrap();

        let a_bytes = fs::read(a.identifier_path).unwrap();
        let b_bytes = fs::read(b.identifier_path).unwrap();
        assert_ne!(
            a_bytes, b_bytes,
            "distinct VM ids must mint distinct identifiers (Phase 0 §D #2)"
        );
    }

    #[test]
    fn corrupt_identifier_is_a_typed_error_not_silent_regen() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();
        let vm_id = "corruption-victim";

        // Pre-create a corrupt identifier file.
        let vm_dir = state_dir.join(vm_id);
        fs::create_dir_all(&vm_dir).unwrap();
        fs::write(vm_dir.join(IDENTIFIER_FILE), b"this is not a Vz identifier").unwrap();

        let err = build_platform(&state_dir, vm_id).unwrap_err();
        assert!(
            err.contains("unreadable"),
            "expected typed unreadable error, got: {err}"
        );
    }

    #[test]
    fn identifier_path_for_returns_predictable_layout() {
        let p = identifier_path_for(Path::new("/state"), "vm-1");
        assert_eq!(p, Path::new("/state/vm-1/identifier.bin"));
    }
}
