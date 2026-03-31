//! Policy evaluation service
//!
//! Wraps a `PolicyVerifier` and emits audit events on every grant/deny.
//! Wedge-1: observational only — does not change grant/deny outcomes.

use std::sync::Arc;

use super::pending::PendingCapabilityRequest;
use super::policy::{
    CheckSeverity, DecisionId, GrantProposal, PolicyDecision, PolicyOutcome, PolicyVerifier,
    ProposedConstraints, VerifierCheck,
};
use crate::primitives::audit::AuditLog;
use crate::primitives::time::SecureTimestamp;

/// Pass-through verifier that echoes the proposal's recommended outcome.
///
/// Produces a coherent audit trail: grant proposals yield grant decisions,
/// deny proposals yield deny decisions. Used during Wedge-1 while the shell
/// is the sole decision-maker.
pub struct ShellPassthroughVerifier;

impl PolicyVerifier for ShellPassthroughVerifier {
    fn verify(
        &self,
        request: &PendingCapabilityRequest,
        proposal: &GrantProposal,
        shadow: bool,
    ) -> PolicyDecision {
        PolicyDecision {
            id: DecisionId::new(),
            request_id: request.id.to_string(),
            resource: request.resource.to_string(),
            action: request.action.to_string(),
            outcome: proposal.recommended_outcome,
            checks: vec![VerifierCheck {
                name: "shell_passthrough".to_string(),
                passed: true,
                reason: proposal.rationale.clone(),
                severity: CheckSeverity::Advisory,
            }],
            effective_constraints: ProposedConstraints::default(),
            effective_expiry: None,
            rationale: proposal.rationale.clone(),
            decided_at: SecureTimestamp::now(),
            shadow,
        }
    }
}

/// Policy evaluation service.
///
/// Builds a `GrantProposal` from the shell's outcome, runs the verifier,
/// and emits `PolicyProposal` + `PolicyDecisionMade` audit events.
/// Optionally runs a shadow verifier in parallel for divergence auditing.
pub struct PolicyEvaluator {
    verifier: Box<dyn PolicyVerifier>,
    shadow_verifier: Option<Box<dyn PolicyVerifier>>,
    audit_log: Arc<AuditLog>,
}

impl PolicyEvaluator {
    pub fn new(verifier: Box<dyn PolicyVerifier>, audit_log: Arc<AuditLog>) -> Self {
        Self {
            verifier,
            shadow_verifier: None,
            audit_log,
        }
    }

    /// Create an evaluator with a shadow verifier for divergence auditing.
    ///
    /// The shadow verifier runs on the same request but its outcome is never
    /// enforced. When the real and shadow outcomes diverge, a `PolicyDivergence`
    /// audit event is emitted.
    pub fn with_shadow(
        verifier: Box<dyn PolicyVerifier>,
        shadow_verifier: Box<dyn PolicyVerifier>,
        audit_log: Arc<AuditLog>,
    ) -> Self {
        Self {
            verifier,
            shadow_verifier: Some(shadow_verifier),
            audit_log,
        }
    }

    /// Evaluate a shell grant/deny decision through the policy pipeline.
    ///
    /// 1. Build a synthetic `GrantProposal` from the shell's decision
    /// 2. Emit `PolicyProposal` audit event
    /// 3. Run the verifier
    /// 4. Emit `PolicyDecisionMade` audit event
    /// 5. Return the `PolicyDecision`
    pub fn evaluate(
        &self,
        request: &PendingCapabilityRequest,
        shell_outcome: PolicyOutcome,
        rationale: &str,
    ) -> PolicyDecision {
        // 1. Build synthetic proposal
        let proposal = GrantProposal::new(
            request.id.to_string(),
            shell_outcome,
            ProposedConstraints::default(),
            rationale.to_string(),
            1.0, // shell decisions have full confidence
            vec![],
        );

        // 2. Emit proposal audit event
        self.audit_log.policy_proposal(
            request.id.as_str(),
            &shell_outcome.to_string(),
            1.0,
            rationale,
        );

        // 3. Run verifier
        let decision = self.verifier.verify(request, &proposal, false);

        // 4. Count checks and emit decision audit event
        let checks_passed = decision.checks.iter().filter(|c| c.passed).count();
        let checks_failed = decision.checks.iter().filter(|c| !c.passed).count();

        self.audit_log.policy_decision_made(
            decision.id.as_str(),
            decision.request_id.as_str(),
            &decision.outcome.to_string(),
            checks_passed,
            checks_failed,
            decision.shadow,
            &decision.rationale,
        );

        // Shadow evaluation: run second verifier, audit its decision, detect divergence
        if let Some(ref shadow) = self.shadow_verifier {
            let shadow_decision = shadow.verify(request, &proposal, true);

            let shadow_passed = shadow_decision.checks.iter().filter(|c| c.passed).count();
            let shadow_failed = shadow_decision.checks.iter().filter(|c| !c.passed).count();

            self.audit_log.policy_decision_made(
                shadow_decision.id.as_str(),
                shadow_decision.request_id.as_str(),
                &shadow_decision.outcome.to_string(),
                shadow_passed,
                shadow_failed,
                true,
                &shadow_decision.rationale,
            );

            if decision.outcome != shadow_decision.outcome {
                self.audit_log.policy_divergence(
                    request.id.as_str(),
                    decision.id.as_str(),
                    shadow_decision.id.as_str(),
                    &decision.outcome.to_string(),
                    &shadow_decision.outcome.to_string(),
                    &decision.rationale,
                    &shadow_decision.rationale,
                );
            }
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::policy::AutoGrantVerifier;
    use crate::capability::token::{Action, ResourceId};
    use crate::session::SessionId;

    fn make_test_request() -> PendingCapabilityRequest {
        PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            300,
        )
    }

    /// Test verifier that always denies — used to force divergence with AutoGrantVerifier.
    struct AlwaysDenyVerifier;

    impl PolicyVerifier for AlwaysDenyVerifier {
        fn verify(
            &self,
            request: &PendingCapabilityRequest,
            _proposal: &GrantProposal,
            shadow: bool,
        ) -> PolicyDecision {
            PolicyDecision {
                id: DecisionId::new(),
                request_id: request.id.to_string(),
                resource: request.resource.to_string(),
                action: request.action.to_string(),
                outcome: PolicyOutcome::Deny,
                checks: vec![VerifierCheck {
                    name: "always_deny".to_string(),
                    passed: false,
                    reason: "Always deny verifier".to_string(),
                    severity: CheckSeverity::Blocking,
                }],
                effective_constraints: ProposedConstraints::default(),
                effective_expiry: None,
                rationale: "Always deny".to_string(),
                decided_at: SecureTimestamp::now(),
                shadow,
            }
        }
    }

    #[test]
    fn test_evaluate_grant_emits_two_audit_events() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Shell auto-grant");

        assert_eq!(decision.outcome, PolicyOutcome::Grant);

        let events = audit_log.recent_events(10);
        assert_eq!(events.len(), 2);
        // newest first
        assert_eq!(events[0].event_type_name(), "policy_decision_made");
        assert_eq!(events[1].event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_evaluate_deny_emits_two_audit_events() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Deny, "Denied by user");

        assert_eq!(decision.outcome, PolicyOutcome::Deny);

        let events = audit_log.recent_events(10);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type_name(), "policy_decision_made");
        assert_eq!(events[1].event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_evaluate_records_correct_request_id() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());
        let request = make_test_request();
        let expected_id = request.id.to_string();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        assert_eq!(decision.request_id, expected_id);
    }

    #[test]
    fn test_evaluate_counts_checks() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        // ShellPassthroughVerifier produces 1 passing advisory check
        assert_eq!(decision.checks.len(), 1);
        assert!(decision.checks[0].passed);
        assert_eq!(decision.checks[0].name, "shell_passthrough");
    }

    // --- Shadow verifier tests ---

    #[test]
    fn test_evaluate_without_shadow_unchanged() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::new(Box::new(ShellPassthroughVerifier), audit_log.clone());
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert!(!decision.shadow);
        // proposal + real decision = 2 events
        assert_eq!(audit_log.recent_events(10).len(), 2);
    }

    #[test]
    fn test_shadow_evaluation_emits_shadow_decision() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(ShellPassthroughVerifier),
            Box::new(AutoGrantVerifier),
            audit_log.clone(),
        );
        let request = make_test_request();

        let _decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        // proposal + real decision + shadow decision = 3 events (no divergence since both grant)
        let events = audit_log.recent_events(10);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type_name(), "policy_decision_made"); // shadow
        assert_eq!(events[1].event_type_name(), "policy_decision_made"); // real
        assert_eq!(events[2].event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_divergence_emits_audit_event() {
        let audit_log = Arc::new(AuditLog::new());
        // Real: passthrough echoes deny. Shadow: auto-grant always grants.
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(ShellPassthroughVerifier),
            Box::new(AutoGrantVerifier),
            audit_log.clone(),
        );
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Deny, "Denied by user");

        assert_eq!(decision.outcome, PolicyOutcome::Deny);

        // proposal + real decision + shadow decision + divergence = 4 events
        let events = audit_log.recent_events(10);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_type_name(), "policy_divergence");
        assert_eq!(events[1].event_type_name(), "policy_decision_made"); // shadow
        assert_eq!(events[2].event_type_name(), "policy_decision_made"); // real
        assert_eq!(events[3].event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_no_divergence_no_event() {
        let audit_log = Arc::new(AuditLog::new());
        // Both grant → no divergence
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(ShellPassthroughVerifier),
            Box::new(AutoGrantVerifier),
            audit_log.clone(),
        );
        let request = make_test_request();

        let _decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Granted");

        let events = audit_log.recent_events(10);
        // proposal + real + shadow = 3, no divergence
        assert_eq!(events.len(), 3);
        assert!(events
            .iter()
            .all(|e| e.event_type_name() != "policy_divergence"));
    }

    #[test]
    fn test_shadow_does_not_change_returned_outcome() {
        let audit_log = Arc::new(AuditLog::new());
        // Real: always deny. Shadow: auto-grant.
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(AlwaysDenyVerifier),
            Box::new(AutoGrantVerifier),
            audit_log.clone(),
        );
        let request = make_test_request();

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        // AlwaysDenyVerifier ignores the proposal and always denies
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        assert!(!decision.shadow);
    }

    #[test]
    fn test_shadow_decision_has_shadow_flag() {
        let audit_log = Arc::new(AuditLog::new());
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(ShellPassthroughVerifier),
            Box::new(AutoGrantVerifier),
            audit_log.clone(),
        );
        let request = make_test_request();

        let _decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "test");

        // Check that the shadow decision audit event has shadow=true
        let events = audit_log.recent_events(10);
        let shadow_events: Vec<_> = events
            .iter()
            .filter(|e| {
                if let crate::primitives::audit::AuditEvent::PolicyDecisionMade { shadow, .. } = e {
                    *shadow
                } else {
                    false
                }
            })
            .collect();
        assert_eq!(shadow_events.len(), 1);
    }

    #[test]
    fn test_rules_shadow_returns_real_decision_unchanged() {
        use crate::capability::policy::RulesVerifier;

        let audit_log = Arc::new(AuditLog::new());
        // Real: ShellPassthroughVerifier (echoes proposal).
        // Shadow: RulesVerifier (may deny blocked resources).
        let evaluator = PolicyEvaluator::with_shadow(
            Box::new(ShellPassthroughVerifier),
            Box::new(RulesVerifier::with_defaults()),
            audit_log.clone(),
        );

        // Grant a request for a blocked resource — shadow would deny, but
        // the returned decision must be the real (passthrough) one.
        let request = PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/admin/secrets"),
            Action::Read,
            300,
        );

        let decision = evaluator.evaluate(&request, PolicyOutcome::Grant, "Shell granted");

        // Real decision: passthrough echoes Grant
        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert!(!decision.shadow);

        // Divergence should be audited (real=grant, shadow=deny)
        let events = audit_log.recent_events(10);
        assert!(events
            .iter()
            .any(|e| e.event_type_name() == "policy_divergence"));
    }
}
