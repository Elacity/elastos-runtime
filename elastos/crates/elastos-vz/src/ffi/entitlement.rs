//! Runtime detection of the `com.apple.vm.networking` entitlement
//! on the current process. **Phase 3 Day 7.**
//!
//! Apple gates `VZBridgedNetworkDeviceAttachment` (TAP-equivalent
//! bridged networking) behind a special-grant entitlement —
//! `com.apple.vm.networking`. Apps that lack it can still use
//! NAT and unix-socket attachments, but `setNetworkDevices:` with
//! a bridged attachment on an unentitled process produces a
//! runtime error from Apple. We need to detect the entitlement
//! before constructing the attachment so the supervisor can
//! either (a) attach the bridged device, or (b) surface a clean,
//! typed `ElastosError::Compute` with remediation instructions.
//!
//! ## Why raw FFI instead of a Rust wrapper
//!
//! `Security.framework` and `CoreFoundation` provide
//! `SecTaskCreateFromSelf` + `SecTaskCopyValueForEntitlement`,
//! which together return the entitlement value for the current
//! process. There is no Rust crate in the workspace's dep tree
//! that wraps these (and we are NOT adding one — the API surface
//! we need is two function calls). Raw `#[link(framework = ...)]`
//! bindings keep the dep graph clean and the unsafe surface
//! tightly scoped.
//!
//! ## Testing
//!
//! The check is process-wide invariant in production, so the
//! result is memoized behind a `OnceLock<bool>`. Tests need to
//! exercise both branches without touching the kernel, so a
//! thread-local override is exposed via
//! [`override_for_testing`] — tests obtain a `RAII` guard that
//! flips the override on the current thread and restores it on
//! drop. This avoids the global-mutable-state hazard of `env`
//! variables in parallel tests.

#![cfg(target_os = "macos")]

use std::cell::Cell;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::sync::OnceLock;

/// Apple's special-grant entitlement key for bridged networking.
/// See [Apple Documentation: Virtualization.framework
/// entitlements](https://developer.apple.com/documentation/bundleresources/entitlements/com_apple_vm_networking).
const VM_NETWORKING_ENTITLEMENT_KEY: &str = "com.apple.vm.networking";

/// CoreFoundation UTF-8 string encoding constant. See
/// `<CoreFoundation/CFString.h>`. Hardcoded because we don't
/// link `CFStringBuiltInEncodings`.
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFErrorRef = *const c_void;
type CFAllocatorRef = *const c_void;
type SecTaskRef = *mut c_void;

// SAFETY (all extern blocks): every function below is a documented
// stable public API of Security.framework / CoreFoundation. We
// pass null allocators where Apple expects them (kCFAllocatorDefault
// is represented by a null pointer per `<CoreFoundation/CFBase.h>`),
// retain/release CFTypes through `CFRelease`, and never let
// references escape the `unsafe` block. There is no callback or
// block bridging — all calls are synchronous.

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecTaskCreateFromSelf(allocator: CFAllocatorRef) -> SecTaskRef;
    fn SecTaskCopyValueForEntitlement(
        task: SecTaskRef,
        entitlement: CFStringRef,
        error: *mut CFErrorRef,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        cstr: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(b: CFTypeRef) -> u8;
}

thread_local! {
    /// Test-only thread-local override. Set via
    /// [`override_for_testing`]; lives only for the lifetime of
    /// the returned [`EntitlementOverrideGuard`]. Production code
    /// never reads this on the main supervisor thread because
    /// tests run in a separate process by default; even if they
    /// did, the cell is `None` until a test explicitly sets it.
    static THREAD_OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Returns `true` iff the current process carries the
/// `com.apple.vm.networking` entitlement with value `true`.
///
/// Process invariant: the result is cached on first call. Tests
/// that need a different result should obtain a guard from
/// [`override_for_testing`].
pub(crate) fn has_vm_networking_entitlement() -> bool {
    if let Some(forced) = THREAD_OVERRIDE.with(|c| c.get()) {
        return forced;
    }
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(check_entitlement_via_security_framework)
}

fn check_entitlement_via_security_framework() -> bool {
    // SAFETY: standard CoreFoundation/Security retain-release
    // discipline. Every `Copy`/`Create` is matched by `CFRelease`
    // on the same pointer, including the early-return paths.
    unsafe {
        let task = SecTaskCreateFromSelf(std::ptr::null());
        if task.is_null() {
            return false;
        }

        let key_cstr = match CString::new(VM_NETWORKING_ENTITLEMENT_KEY) {
            Ok(s) => s,
            Err(_) => {
                CFRelease(task);
                return false;
            }
        };
        let key_cf =
            CFStringCreateWithCString(std::ptr::null(), key_cstr.as_ptr(), CF_STRING_ENCODING_UTF8);
        if key_cf.is_null() {
            CFRelease(task);
            return false;
        }

        let mut err: CFErrorRef = std::ptr::null();
        let value = SecTaskCopyValueForEntitlement(task, key_cf, &mut err as *mut CFErrorRef);

        // CFRelease the inputs first; their refcounts are
        // independent of the returned value's.
        CFRelease(key_cf);
        CFRelease(task);

        if value.is_null() {
            if !err.is_null() {
                CFRelease(err);
            }
            return false;
        }

        // The entitlement value must be a CFBoolean(true). Any
        // other type (string, number, dictionary) means the key
        // is present but the binary doesn't actually claim
        // bridged-networking privilege — treat as `false`.
        let result = CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value) != 0;
        CFRelease(value);
        result
    }
}

/// Test-only RAII override of the entitlement check.
///
/// Tests call `let _guard = override_for_testing(true);` at the
/// top of a test body; the override is restored on drop so
/// parallel tests on other threads are unaffected.
#[cfg(test)]
pub(crate) fn override_for_testing(value: bool) -> EntitlementOverrideGuard {
    let prior = THREAD_OVERRIDE.with(|c| c.replace(Some(value)));
    EntitlementOverrideGuard { prior }
}

#[cfg(test)]
pub(crate) struct EntitlementOverrideGuard {
    prior: Option<bool>,
}

#[cfg(test)]
impl Drop for EntitlementOverrideGuard {
    fn drop(&mut self) {
        THREAD_OVERRIDE.with(|c| c.set(self.prior));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entitlement_check_returns_false_for_unsigned_dev_binary() {
        // The CI binary is unsigned (no Developer ID, no
        // entitlement plist). The check MUST return `false`,
        // not panic, not error. This is the dev-build invariant.
        // We explicitly bypass the thread-local override by NOT
        // calling `override_for_testing` here.
        assert!(
            !has_vm_networking_entitlement(),
            "unsigned dev binary must not claim the com.apple.vm.networking entitlement"
        );
    }

    #[test]
    fn override_for_testing_round_trips_true_and_false() {
        let _guard_true = override_for_testing(true);
        assert!(has_vm_networking_entitlement());
        drop(_guard_true);

        // Without an active guard, falls back to the production
        // path (unsigned dev binary => false).
        assert!(!has_vm_networking_entitlement());

        let _guard_false = override_for_testing(false);
        assert!(!has_vm_networking_entitlement());
    }

    #[test]
    fn override_guard_restores_prior_state_on_drop() {
        // Nested overrides must restore correctly — the inner
        // drop should reveal the outer override's value.
        let _outer = override_for_testing(true);
        assert!(has_vm_networking_entitlement());
        {
            let _inner = override_for_testing(false);
            assert!(!has_vm_networking_entitlement());
        }
        assert!(
            has_vm_networking_entitlement(),
            "inner override drop must reveal outer override (true)"
        );
    }
}
