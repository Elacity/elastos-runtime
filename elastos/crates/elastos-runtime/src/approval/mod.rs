//! Approval decisions (prototype) — the "approve" step of the control loop.
//!
//! The bridge from *preview the gate* (`invoke`) to *act* (a future dispatcher):
//! given the approval a call requires and any recorded approver decision, yield
//! [`ApprovalDecision::Approved`] | [`Denied`](ApprovalDecision::Denied) |
//! [`PendingApproval`](ApprovalDecision::PendingApproval).
//!
//! Pure and fail-closed by design: it records nothing and dispatches nothing
//! (mirroring [`crate::inspect`] and [`crate::invoke`]). Persisting a *signed*
//! decision is a mutation and belongs with the dispatcher, not here — this is
//! only the decision logic. The single rule: **we never auto-approve what we
//! cannot evaluate.**

use crate::capability::token::Action;
use elastos_common::AffordanceApprovalMode;
use serde::Serialize;

/// The outcome of an approval evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// May proceed — either declared as needing no approval, or explicitly approved.
    Approved,
    /// Explicitly refused by the approver. An explicit "no" always wins.
    Denied,
    /// Needs an explicit human/authority decision that has not yet been given.
    PendingApproval,
}

/// Derive the approval a provider operation requires from the capability actions
/// its gate demands. Fail-closed by strength: anything that writes, mutates,
/// actuates, or administers needs explicit human approval; pure reads/messages
/// need none. (Orthogonal to the capability gate — both must pass.)
pub fn required_approval(actions: &[Action]) -> AffordanceApprovalMode {
    let needs_human = actions
        .iter()
        .any(|a| !matches!(a, Action::Read | Action::Message));
    if needs_human {
        AffordanceApprovalMode::User
    } else {
        AffordanceApprovalMode::None
    }
}

/// Decide whether an action may proceed, fail-closed.
///
/// `approver` is the recorded decision, if any: `Some(true)` = approved,
/// `Some(false)` = denied, `None` = not yet decided. The *only* path to
/// `Approved` without an explicit yes is an affordance that declared it needs no
/// approval ([`AffordanceApprovalMode::None`]). `User` and `RuntimePolicy` both
/// default to [`PendingApproval`] until an explicit decision exists —
/// `RuntimePolicy` fails closed because no policy engine evaluates it yet.
pub fn decide(mode: &AffordanceApprovalMode, approver: Option<bool>) -> ApprovalDecision {
    match approver {
        Some(false) => ApprovalDecision::Denied, // an explicit "no" always wins
        Some(true) => ApprovalDecision::Approved, // an explicit "yes"
        None => match mode {
            AffordanceApprovalMode::None => ApprovalDecision::Approved,
            // User / RuntimePolicy need a decision we don't have — never auto-approve.
            _ => ApprovalDecision::PendingApproval,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_approval_scales_with_action_strength() {
        assert_eq!(
            required_approval(&[Action::Read]),
            AffordanceApprovalMode::None
        );
        assert_eq!(
            required_approval(&[Action::Read, Action::Message]),
            AffordanceApprovalMode::None
        );
        // Anything stronger than read/message demands a human.
        assert_eq!(
            required_approval(&[Action::Write]),
            AffordanceApprovalMode::User
        );
        assert_eq!(
            required_approval(&[Action::Execute]),
            AffordanceApprovalMode::User
        );
        assert_eq!(
            required_approval(&[Action::Admin]),
            AffordanceApprovalMode::User
        );
        assert_eq!(
            required_approval(&[Action::Read, Action::Admin]),
            AffordanceApprovalMode::User
        );
    }

    #[test]
    fn decide_fails_closed_without_an_explicit_yes() {
        // The G4 invariant: User/RuntimePolicy never auto-approve.
        assert_eq!(
            decide(&AffordanceApprovalMode::User, None),
            ApprovalDecision::PendingApproval
        );
        assert_eq!(
            decide(&AffordanceApprovalMode::RuntimePolicy, None),
            ApprovalDecision::PendingApproval
        );
        // Only an affordance that declared "no approval needed" auto-approves.
        assert_eq!(
            decide(&AffordanceApprovalMode::None, None),
            ApprovalDecision::Approved
        );
    }

    #[test]
    fn explicit_decisions_are_honored() {
        assert_eq!(
            decide(&AffordanceApprovalMode::User, Some(true)),
            ApprovalDecision::Approved
        );
        assert_eq!(
            decide(&AffordanceApprovalMode::User, Some(false)),
            ApprovalDecision::Denied
        );
        // An explicit "no" wins even where no approval was required.
        assert_eq!(
            decide(&AffordanceApprovalMode::None, Some(false)),
            ApprovalDecision::Denied
        );
    }
}
