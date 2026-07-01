//! Inspect access control for runtime-owned live object mirrors.
//!
//! The inspect surface is intentionally small and fail-closed: a System mirror
//! may see every object, while a SelfOnly mirror may see only the caller's own
//! capsule object. This module is pure so provider, gateway, and Carrier paths
//! can share one decision without depending on any transport.

/// Capability pattern for the privileged, system-wide inspect view.
pub const INSPECT_SYSTEM: &str = "elastos://inspect/*";

/// Capability pattern for the caller's own inspect view.
pub const INSPECT_SELF: &str = "elastos://inspect/self";

/// Visibility granted to an inspect caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectScope {
    System,
    SelfOnly,
}

/// Map a granted resource to an inspect scope.
pub fn scope_for_grant(resource: &str) -> Option<InspectScope> {
    match resource {
        INSPECT_SYSTEM => Some(InspectScope::System),
        INSPECT_SELF => Some(InspectScope::SelfOnly),
        _ => None,
    }
}

impl InspectScope {
    /// Strongest inspect scope implied by the caller's grants.
    pub fn from_grants<I, S>(is_shell: bool, granted: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if is_shell {
            return Some(Self::System);
        }
        let mut scope = None;
        for resource in granted {
            match scope_for_grant(resource.as_ref()) {
                Some(Self::System) => return Some(Self::System),
                Some(Self::SelfOnly) => scope = Some(Self::SelfOnly),
                None => {}
            }
        }
        scope
    }

    /// Whether `caller` may view `target`.
    pub fn can_view(self, caller: &str, target: &str) -> bool {
        match self {
            Self::System => true,
            Self::SelfOnly => caller == target,
        }
    }
}

/// Decide whether an inspect caller may view a target capsule.
pub fn authorize_view(is_shell: bool, caller: &str, target: &str, granted: &[String]) -> bool {
    InspectScope::from_grants(is_shell, granted.iter())
        .map(|scope| scope.can_view(caller, target))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_scope_fails_closed_without_grant() {
        assert_eq!(InspectScope::from_grants(false, Vec::<String>::new()), None);
        assert!(!authorize_view(false, "capsule:a", "capsule:a", &[]));
    }

    #[test]
    fn self_only_scope_cannot_cross_capsule_boundary() {
        let grants = vec![INSPECT_SELF.to_string()];
        assert!(authorize_view(false, "capsule:a", "capsule:a", &grants));
        assert!(!authorize_view(false, "capsule:a", "capsule:b", &grants));
    }

    #[test]
    fn system_scope_can_view_everything() {
        let grants = vec![INSPECT_SYSTEM.to_string()];
        assert!(authorize_view(false, "system", "capsule:b", &grants));
        assert!(authorize_view(true, "shell", "capsule:b", &[]));
    }

    #[test]
    fn system_grant_wins_over_self_grant() {
        let grants = [INSPECT_SELF.to_string(), INSPECT_SYSTEM.to_string()];
        assert_eq!(
            InspectScope::from_grants(false, grants.iter()),
            Some(InspectScope::System)
        );
    }
}
