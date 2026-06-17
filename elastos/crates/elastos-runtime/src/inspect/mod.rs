//! Inspect access control — the security core of the Capsule Inspector.
//!
//! The Inspector exposes a read-only, object-centered view of live capsules
//! (see `docs/CAPSULE_INSPECTOR.md`). A *system* view aggregates every
//! capsule's manifest, capability grants, and audit trail, so handing it to an
//! ordinary app would be an information-disclosure / privilege-escalation hole
//! — the precise opposite of what this surface exists to demonstrate.
//!
//! Visibility is therefore scoped, and this module owns that decision as a
//! pure, testable unit. The runtime-side inspect handler MUST call
//! [`authorize_view`] before returning any per-capsule detail, and MUST audit
//! denials. Keeping the decision here (no async, no I/O) lets us prove the
//! invariant in isolation, independent of the handler wiring.
//!
//! ## Two tiers (encoded as capability grant patterns)
//!
//! - [`INSPECT_SYSTEM`] (`elastos://inspect/*`): the privileged, system-wide
//!   view. This wildcard pattern is what lets a caller reach the system
//!   endpoints (`elastos://inspect/capsules`, `.../capsule`). Granted only to
//!   the shell / System surface.
//! - [`INSPECT_SELF`] (`elastos://inspect/self`): the self-only view. A caller
//!   holding this may reach only `elastos://inspect/self` and sees only its
//!   own capsule record.
//!
//! Because capability validation matches the requested URI against the token's
//! resource *pattern*, a self-only token (`elastos://inspect/self`, no
//! wildcard) cannot satisfy a request to `elastos://inspect/capsules` — so the
//! tier boundary is enforced by the existing capability layer, and
//! [`authorize_view`] is the defense-in-depth gate on top.

/// Capability grant pattern for the privileged, system-wide inspect view.
pub const INSPECT_SYSTEM: &str = "elastos://inspect/*";

/// Capability grant pattern for the self-only inspect view.
pub const INSPECT_SELF: &str = "elastos://inspect/self";

/// The visibility a caller is entitled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectScope {
    /// May inspect every capsule (shell / System only).
    System,
    /// May inspect only its own capsule record.
    SelfOnly,
}

/// Map a single granted capability resource to the inspect scope it confers.
///
/// Returns `None` for any resource that is not an inspect grant.
pub fn scope_for_grant(resource: &str) -> Option<InspectScope> {
    match resource {
        INSPECT_SYSTEM => Some(InspectScope::System),
        INSPECT_SELF => Some(InspectScope::SelfOnly),
        _ => None,
    }
}

impl InspectScope {
    /// Derive the scope a caller is entitled to.
    ///
    /// Shell callers always receive [`InspectScope::System`]. Otherwise the
    /// scope is the strongest one implied by the caller's *granted* inspect
    /// capabilities. Returns `None` when the caller holds no inspect
    /// capability at all — in which case no inspect data may be returned.
    pub fn from_grants<I, S>(is_shell: bool, granted: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if is_shell {
            return Some(InspectScope::System);
        }

        let mut scope = None;
        for resource in granted {
            match scope_for_grant(resource.as_ref()) {
                // System is the strongest tier; nothing widens it further.
                Some(InspectScope::System) => return Some(InspectScope::System),
                Some(InspectScope::SelfOnly) => scope = Some(InspectScope::SelfOnly),
                None => {}
            }
        }
        scope
    }

    /// Whether a caller with this scope may inspect `target`, given the
    /// caller's own capsule id.
    pub fn can_view(self, caller: &str, target: &str) -> bool {
        match self {
            InspectScope::System => true,
            InspectScope::SelfOnly => caller == target,
        }
    }
}

/// Decide whether `caller` may inspect `target`.
///
/// This is the single authorization gate the inspect handler must call before
/// returning any per-capsule detail. It fails closed: a caller with no inspect
/// capability (and that is not the shell) is denied. Denials are the handler's
/// responsibility to audit.
pub fn authorize_view(is_shell: bool, caller: &str, target: &str, granted: &[String]) -> bool {
    match InspectScope::from_grants(is_shell, granted.iter()) {
        Some(scope) => scope.can_view(caller, target),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "cap_alice";
    const BOB: &str = "cap_bob";

    #[test]
    fn scope_for_grant_maps_known_patterns() {
        assert_eq!(scope_for_grant(INSPECT_SYSTEM), Some(InspectScope::System));
        assert_eq!(scope_for_grant(INSPECT_SELF), Some(InspectScope::SelfOnly));
        assert_eq!(scope_for_grant("elastos://inspect/capsules"), None);
        assert_eq!(scope_for_grant("elastos://storage/foo"), None);
    }

    #[test]
    fn shell_always_gets_system_scope_even_without_grants() {
        let scope = InspectScope::from_grants(true, Vec::<String>::new());
        assert_eq!(scope, Some(InspectScope::System));
    }

    #[test]
    fn system_grant_yields_system_scope() {
        let scope = InspectScope::from_grants(false, [INSPECT_SYSTEM.to_string()]);
        assert_eq!(scope, Some(InspectScope::System));
    }

    #[test]
    fn self_grant_yields_self_only_scope() {
        let scope = InspectScope::from_grants(false, [INSPECT_SELF.to_string()]);
        assert_eq!(scope, Some(InspectScope::SelfOnly));
    }

    #[test]
    fn no_inspect_capability_yields_no_scope() {
        let scope = InspectScope::from_grants(false, ["elastos://storage/foo".to_string()]);
        assert_eq!(scope, None);
    }

    #[test]
    fn system_grant_wins_regardless_of_order() {
        let forward = InspectScope::from_grants(
            false,
            [INSPECT_SELF.to_string(), INSPECT_SYSTEM.to_string()],
        );
        let reverse = InspectScope::from_grants(
            false,
            [INSPECT_SYSTEM.to_string(), INSPECT_SELF.to_string()],
        );
        assert_eq!(forward, Some(InspectScope::System));
        assert_eq!(reverse, Some(InspectScope::System));
    }

    #[test]
    fn system_scope_can_view_any_target() {
        assert!(InspectScope::System.can_view(ALICE, ALICE));
        assert!(InspectScope::System.can_view(ALICE, BOB));
    }

    #[test]
    fn self_only_scope_can_view_self_but_not_others() {
        assert!(InspectScope::SelfOnly.can_view(ALICE, ALICE));
        assert!(!InspectScope::SelfOnly.can_view(ALICE, BOB));
    }

    #[test]
    fn authorize_view_self_only_capsule_sees_only_itself() {
        let grants = vec![INSPECT_SELF.to_string()];
        // Alice inspecting herself: allowed.
        assert!(authorize_view(false, ALICE, ALICE, &grants));
        // Alice inspecting Bob: denied — this is the privilege-escalation guard.
        assert!(!authorize_view(false, ALICE, BOB, &grants));
    }

    #[test]
    fn authorize_view_system_surface_sees_everything() {
        let grants = vec![INSPECT_SYSTEM.to_string()];
        assert!(authorize_view(false, "system", BOB, &grants));
        // Shell needs no explicit grant.
        assert!(authorize_view(true, "shell", BOB, &[]));
    }

    #[test]
    fn authorize_view_fails_closed_without_capability() {
        // No inspect capability and not the shell: denied even for self.
        assert!(!authorize_view(false, ALICE, ALICE, &[]));
        assert!(!authorize_view(false, ALICE, BOB, &[]));
    }
}
