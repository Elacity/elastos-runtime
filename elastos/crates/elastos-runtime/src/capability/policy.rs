//! Policy control types for the intelligent shell
//!
//! Defines the data types, trait, and built-in verifier for the capability
//! policy evaluation pipeline. A proposer (advisory, possibly LLM-backed)
//! recommends grant/deny; a deterministic verifier makes the authoritative
//! decision.
//!
//! This module is pure types — no behavioral wiring, no LLM dependencies.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::pending::PendingCapabilityRequest;
use super::token::Action;
use crate::primitives::time::SecureTimestamp;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a policy decision (UUID string, like RequestId)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DecisionId(pub String);

impl DecisionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DecisionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DecisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Outcome of a policy evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    Grant,
    Deny,
    Defer,
}

impl fmt::Display for PolicyOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyOutcome::Grant => write!(f, "grant"),
            PolicyOutcome::Deny => write!(f, "deny"),
            PolicyOutcome::Defer => write!(f, "defer"),
        }
    }
}

/// Severity of a verifier check
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    Blocking,
    Advisory,
}

impl fmt::Display for CheckSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckSeverity::Blocking => write!(f, "blocking"),
            CheckSeverity::Advisory => write!(f, "advisory"),
        }
    }
}

/// Type of evidence supporting a decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    RuntimeCheck,
    UserConfirm,
    HistoryMatch,
    PolicyRule,
}

impl fmt::Display for EvidenceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvidenceType::RuntimeCheck => write!(f, "runtime_check"),
            EvidenceType::UserConfirm => write!(f, "user_confirm"),
            EvidenceType::HistoryMatch => write!(f, "history_match"),
            EvidenceType::PolicyRule => write!(f, "policy_rule"),
        }
    }
}

// ---------------------------------------------------------------------------
// Proposer types
// ---------------------------------------------------------------------------

/// Constraints suggested by a proposer (advisory, not yet concrete tokens)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProposedConstraints {
    /// Optional resource scope narrowing (e.g. "localhost://Users/self/Documents/photos/*")
    #[serde(default)]
    pub resource_scope: Option<String>,

    /// Suggested TTL in seconds
    #[serde(default)]
    pub ttl_secs: Option<u64>,

    /// Suggested maximum uses
    #[serde(default)]
    pub max_uses: Option<u32>,

    /// Whether to allow delegation
    #[serde(default)]
    pub delegatable: bool,

    /// Maximum classification level
    #[serde(default)]
    pub max_classification: Option<u8>,
}

/// Advisory recommendation from the proposer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantProposal {
    /// Which request this proposal is for
    pub request_id: String,

    /// Recommended outcome
    pub recommended_outcome: PolicyOutcome,

    /// Suggested constraints if granting
    pub proposed_constraints: ProposedConstraints,

    /// Human-readable rationale
    pub rationale: String,

    /// Confidence score (0.0 - 1.0, clamped)
    pub confidence: f32,

    /// What evidence is still missing
    pub evidence_gaps: Vec<String>,

    /// When this proposal was created
    pub created_at: SecureTimestamp,
}

impl GrantProposal {
    /// Clamp a confidence value to [0.0, 1.0]
    pub fn clamp_confidence(value: f32) -> f32 {
        value.clamp(0.0, 1.0)
    }

    /// Create a new proposal with confidence clamped to [0.0, 1.0]
    pub fn new(
        request_id: String,
        recommended_outcome: PolicyOutcome,
        proposed_constraints: ProposedConstraints,
        rationale: String,
        confidence: f32,
        evidence_gaps: Vec<String>,
    ) -> Self {
        Self {
            request_id,
            recommended_outcome,
            proposed_constraints,
            rationale,
            confidence: Self::clamp_confidence(confidence),
            evidence_gaps,
            created_at: SecureTimestamp::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Verifier types
// ---------------------------------------------------------------------------

/// Single check result from a verifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierCheck {
    /// Check name (e.g. "epoch_valid", "resource_allowed")
    pub name: String,

    /// Whether the check passed
    pub passed: bool,

    /// Human-readable reason
    pub reason: String,

    /// Whether failure blocks the grant
    pub severity: CheckSeverity,
}

/// Authoritative policy decision from the verifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// Unique decision identifier
    pub id: DecisionId,

    /// Which request this decides
    pub request_id: String,

    /// Snapshot of requested resource at decision time
    pub resource: String,

    /// Snapshot of requested action at decision time
    pub action: String,

    /// The authoritative outcome
    pub outcome: PolicyOutcome,

    /// Individual check results
    pub checks: Vec<VerifierCheck>,

    /// Effective constraints (may differ from proposed)
    pub effective_constraints: ProposedConstraints,

    /// Concrete expiry timestamp (resolved from TTL)
    pub effective_expiry: Option<SecureTimestamp>,

    /// Human-readable rationale
    pub rationale: String,

    /// When the decision was made
    pub decided_at: SecureTimestamp,

    /// Whether this decision is shadow-mode (observe only, not enforced)
    pub shadow: bool,
}

// ---------------------------------------------------------------------------
// Evidence and planning types
// ---------------------------------------------------------------------------

/// Evidence record supporting a decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    /// Which decision this evidence supports
    pub decision_id: String,

    /// Type of evidence
    pub evidence_type: EvidenceType,

    /// Source of evidence (e.g. "runtime", "user", "history_store")
    pub source: String,

    /// Evidence content (human-readable)
    pub content: String,

    /// When the evidence was recorded
    pub recorded_at: SecureTimestamp,

    /// How much this evidence shifted confidence
    pub confidence_delta: f32,
}

/// Compiled request intent for structured reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnPlan {
    /// Which request this plan describes
    pub request_id: String,

    /// What the capsule intends to do
    pub intent: String,

    /// What the capsule hopes to achieve
    pub objective: String,

    /// What evidence is still missing
    pub evidence_gaps: Vec<String>,

    /// Recommended action based on current evidence
    pub recommended_action: PolicyOutcome,

    /// When this plan was compiled
    pub compiled_at: SecureTimestamp,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Synchronous policy verifier — must be deterministic and fast.
/// No async, no network, no LLM.
pub trait PolicyVerifier: Send + Sync {
    fn verify(
        &self,
        request: &PendingCapabilityRequest,
        proposal: &GrantProposal,
        shadow: bool,
    ) -> PolicyDecision;
}

// ---------------------------------------------------------------------------
// Built-in verifier: auto-grant (replicates current shell behavior)
// ---------------------------------------------------------------------------

/// Always grants — replicates current shell auto-grant behavior.
pub struct AutoGrantVerifier;

impl PolicyVerifier for AutoGrantVerifier {
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
            outcome: PolicyOutcome::Grant,
            checks: vec![VerifierCheck {
                name: "auto_grant".to_string(),
                passed: true,
                reason: "Auto-grant verifier: unconditional grant".to_string(),
                severity: CheckSeverity::Advisory,
            }],
            effective_constraints: ProposedConstraints::default(),
            effective_expiry: None,
            rationale: "Auto-grant: all requests approved".to_string(),
            decided_at: SecureTimestamp::now(),
            shadow,
        }
    }
}

// ---------------------------------------------------------------------------
// Rules-based verifier (Wedge-4)
// ---------------------------------------------------------------------------

/// A single deterministic policy rule
#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub name: String,
    pub check: RuleCheck,
    pub severity: CheckSeverity,
}

/// The specific check a rule performs
#[derive(Debug, Clone)]
pub enum RuleCheck {
    /// Deny if any path segment of the resource matches a blocked name
    ResourceBlocklist(Vec<String>),
    /// Deny if requested action is in this set
    ActionBlocklist(Vec<Action>),
    /// Deny if proposal confidence is below this threshold
    ConfidenceThreshold(f32),
    /// Warn if proposed TTL exceeds this many seconds
    MaxTtlSecs(u64),
    /// Warn if proposed constraints mark token as delegatable
    NoDelegation,
    /// Deny if resource starts with `applies_to` prefix but NOT any `allowed` prefix
    SchemeAllowlist {
        applies_to: String,
        allowed: Vec<String>,
    },
    /// Warn if grant for matching scheme lacks max_uses or exceeds cap
    MaxUsesForScheme { scheme_prefix: String, max: u32 },
    /// Warn if grant TTL for matching scheme exceeds limit
    MaxTtlForScheme {
        scheme_prefix: String,
        max_secs: u64,
    },
    /// Flag when broad scope should be narrowed
    ScopeNarrowing {
        from_pattern: String,
        to_scope: String,
    },
}

/// Evaluate a single rule against a request/proposal pair.
/// Returns (passed, reason).
fn evaluate_rule(
    rule: &PolicyRule,
    request: &PendingCapabilityRequest,
    proposal: &GrantProposal,
) -> (bool, String) {
    match &rule.check {
        RuleCheck::ResourceBlocklist(blocked_segments) => {
            let res = request.resource.as_str();
            let path_segments: Vec<&str> = res.split('/').collect();
            match blocked_segments
                .iter()
                .find(|blocked| path_segments.contains(&blocked.as_str()))
            {
                Some(s) => (false, format!("resource contains blocked segment: {}", s)),
                None => (true, "no blocklist match".to_string()),
            }
        }
        RuleCheck::ActionBlocklist(actions) => {
            if actions.contains(&request.action) {
                (false, format!("{} action denied by policy", request.action))
            } else {
                (true, format!("{} action allowed", request.action))
            }
        }
        RuleCheck::ConfidenceThreshold(min) => {
            if proposal.confidence < *min {
                (
                    false,
                    format!(
                        "confidence {:.2} below threshold {:.2}",
                        proposal.confidence, min
                    ),
                )
            } else {
                (
                    true,
                    format!("confidence {:.2} meets threshold", proposal.confidence),
                )
            }
        }
        RuleCheck::MaxTtlSecs(max) => match proposal.proposed_constraints.ttl_secs {
            Some(ttl) if ttl > *max => {
                (false, format!("proposed TTL {}s exceeds max {}s", ttl, max))
            }
            _ => (true, "TTL within limits".to_string()),
        },
        RuleCheck::NoDelegation => {
            if proposal.proposed_constraints.delegatable {
                (false, "delegation not permitted by policy".to_string())
            } else {
                (true, "not delegatable".to_string())
            }
        }
        RuleCheck::SchemeAllowlist {
            applies_to,
            allowed,
        } => {
            let res = request.resource.as_str();
            if !res.starts_with(applies_to.as_str()) {
                return (true, "scheme not applicable".to_string());
            }
            // Extract the backend segment: everything between applies_to and the next '/'
            let suffix = &res[applies_to.len()..];
            let backend = suffix.split('/').next().unwrap_or("");
            // Check if any allowed entry's backend segment matches exactly
            let allowed_backends: Vec<&str> = allowed
                .iter()
                .map(|a| {
                    let s = a.strip_prefix(applies_to.as_str()).unwrap_or(a);
                    s.split('/').next().unwrap_or("")
                })
                .collect();
            if allowed_backends.contains(&backend) {
                (true, "backend in allowlist".to_string())
            } else {
                (false, format!("backend '{}' not in allowlist", backend))
            }
        }
        RuleCheck::MaxUsesForScheme { scheme_prefix, max } => {
            let res = request.resource.as_str();
            if !res.starts_with(scheme_prefix.as_str()) {
                return (true, "scheme not applicable".to_string());
            }
            match proposal.proposed_constraints.max_uses {
                None => (
                    false,
                    format!("no max_uses set for {} resource", scheme_prefix),
                ),
                Some(v) if v > *max => (false, format!("max_uses {} exceeds limit {}", v, max)),
                Some(v) => (true, format!("max_uses {} within limit", v)),
            }
        }
        RuleCheck::MaxTtlForScheme {
            scheme_prefix,
            max_secs,
        } => {
            let res = request.resource.as_str();
            if !res.starts_with(scheme_prefix.as_str()) {
                return (true, "scheme not applicable".to_string());
            }
            match proposal.proposed_constraints.ttl_secs {
                Some(ttl) if ttl > *max_secs => (
                    false,
                    format!("TTL {}s exceeds AI limit {}s", ttl, max_secs),
                ),
                _ => (true, "TTL within AI limits".to_string()),
            }
        }
        RuleCheck::ScopeNarrowing {
            from_pattern,
            to_scope,
        } => {
            let res = request.resource.as_str();
            // Check if resource matches the broad pattern
            let broad = super::token::ResourceId::new(from_pattern);
            let actual = super::token::ResourceId::new(res);
            if !actual.matches(&broad) {
                return (true, "pattern not applicable".to_string());
            }
            // Check if already narrow enough
            let narrow = super::token::ResourceId::new(to_scope);
            if actual.matches(&narrow) {
                (true, "scope already narrow".to_string())
            } else {
                (
                    false,
                    format!("broad scope {} should be narrowed to {}", res, to_scope),
                )
            }
        }
    }
}

/// Deterministic rules-based policy verifier.
///
/// Runs a list of `PolicyRule`s against each request. Blocking failures
/// produce a Deny; advisory failures are recorded but don't override the
/// proposal's recommended outcome. The verifier can only tighten — it never
/// loosens a Deny proposal to Grant.
pub struct RulesVerifier {
    rules: Vec<PolicyRule>,
}

impl RulesVerifier {
    pub fn new(rules: Vec<PolicyRule>) -> Self {
        Self { rules }
    }

    pub fn with_defaults() -> Self {
        Self {
            rules: vec![
                PolicyRule {
                    name: "resource_blocklist".into(),
                    check: RuleCheck::ResourceBlocklist(vec![
                        "admin".into(),
                        "config".into(),
                        ".ssh".into(),
                        ".keys".into(),
                    ]),
                    severity: CheckSeverity::Blocking,
                },
                PolicyRule {
                    name: "action_blocklist".into(),
                    check: RuleCheck::ActionBlocklist(vec![Action::Admin]),
                    severity: CheckSeverity::Blocking,
                },
                PolicyRule {
                    name: "confidence_threshold".into(),
                    check: RuleCheck::ConfidenceThreshold(0.5),
                    severity: CheckSeverity::Advisory,
                },
                PolicyRule {
                    name: "max_ttl".into(),
                    check: RuleCheck::MaxTtlSecs(86400),
                    severity: CheckSeverity::Advisory,
                },
                PolicyRule {
                    name: "no_delegation".into(),
                    check: RuleCheck::NoDelegation,
                    severity: CheckSeverity::Advisory,
                },
                // --- AI-specific rules ---
                PolicyRule {
                    name: "ai_scheme_allowlist".into(),
                    check: RuleCheck::SchemeAllowlist {
                        applies_to: "elastos://ai/".into(),
                        allowed: vec![
                            "elastos://ai/local".into(),
                            "elastos://ai/venice".into(),
                            "elastos://ai/codex".into(),
                            "elastos://ai/meta".into(),
                        ],
                    },
                    severity: CheckSeverity::Blocking,
                },
                PolicyRule {
                    name: "ai_max_uses".into(),
                    check: RuleCheck::MaxUsesForScheme {
                        scheme_prefix: "elastos://ai/".into(),
                        max: 100,
                    },
                    severity: CheckSeverity::Advisory,
                },
                PolicyRule {
                    name: "ai_max_ttl".into(),
                    check: RuleCheck::MaxTtlForScheme {
                        scheme_prefix: "elastos://ai/".into(),
                        max_secs: 3600,
                    },
                    severity: CheckSeverity::Advisory,
                },
                PolicyRule {
                    name: "ai_scope_narrowing".into(),
                    check: RuleCheck::ScopeNarrowing {
                        from_pattern: "elastos://ai/*".into(),
                        to_scope: "elastos://ai/local/*".into(),
                    },
                    severity: CheckSeverity::Advisory,
                },
            ],
        }
    }
}

impl PolicyVerifier for RulesVerifier {
    fn verify(
        &self,
        request: &PendingCapabilityRequest,
        proposal: &GrantProposal,
        shadow: bool,
    ) -> PolicyDecision {
        let mut checks = Vec::new();
        let mut blocking_failures: Vec<String> = Vec::new();

        for rule in &self.rules {
            let (passed, reason) = evaluate_rule(rule, request, proposal);
            if !passed && rule.severity == CheckSeverity::Blocking {
                blocking_failures.push(reason.clone());
            }
            checks.push(VerifierCheck {
                name: rule.name.clone(),
                passed,
                reason,
                severity: rule.severity,
            });
        }

        let advisory_failures: Vec<String> = checks
            .iter()
            .filter(|c| !c.passed && c.severity == CheckSeverity::Advisory)
            .map(|c| c.reason.clone())
            .collect();

        // Only echo Grant or Deny from the proposal. Blocking failures or
        // Defer proposals both resolve to Deny (rules never produce Defer,
        // and can only tighten — never loosen).
        let outcome = if !blocking_failures.is_empty()
            || proposal.recommended_outcome != PolicyOutcome::Grant
        {
            PolicyOutcome::Deny
        } else {
            PolicyOutcome::Grant
        };

        let rationale = if !blocking_failures.is_empty() {
            format!("Blocked: {}", blocking_failures.join("; "))
        } else if !advisory_failures.is_empty() {
            format!(
                "Advisory warnings ({}); echoing proposal: {}",
                advisory_failures.join("; "),
                proposal.rationale
            )
        } else {
            format!("All rules passed, echoing proposal: {}", proposal.rationale)
        };

        PolicyDecision {
            id: DecisionId::new(),
            request_id: request.id.to_string(),
            resource: request.resource.to_string(),
            action: request.action.to_string(),
            outcome,
            checks,
            effective_constraints: proposal.proposed_constraints.clone(),
            effective_expiry: None,
            rationale,
            decided_at: SecureTimestamp::now(),
            shadow,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::token::{Action, ResourceId};
    use crate::primitives::time::SecureTimestamp;
    use crate::session::SessionId;

    fn make_test_request() -> PendingCapabilityRequest {
        PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            Action::Read,
            300,
        )
    }

    fn make_test_proposal(request_id: &str) -> GrantProposal {
        GrantProposal::new(
            request_id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints::default(),
            "Test rationale".to_string(),
            0.9,
            vec![],
        )
    }

    #[test]
    fn test_decision_id_uniqueness() {
        let id1 = DecisionId::new();
        let id2 = DecisionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_grant_proposal_serialization() {
        let proposal = GrantProposal::new(
            "req-123".to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                resource_scope: Some("localhost://Users/self/Documents/photos/*".to_string()),
                ttl_secs: Some(3600),
                max_uses: Some(10),
                delegatable: false,
                max_classification: Some(128),
            },
            "User has granted this before".to_string(),
            0.85,
            vec!["no_recent_deny".to_string()],
        );

        let json = serde_json::to_string(&proposal).unwrap();
        let restored: GrantProposal = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.request_id, "req-123");
        assert_eq!(restored.recommended_outcome, PolicyOutcome::Grant);
        assert_eq!(restored.confidence, 0.85);
        assert_eq!(restored.evidence_gaps.len(), 1);
        assert_eq!(
            restored.proposed_constraints.resource_scope,
            Some("localhost://Users/self/Documents/photos/*".to_string())
        );
        assert_eq!(restored.proposed_constraints.ttl_secs, Some(3600));
    }

    #[test]
    fn test_grant_proposal_confidence_clamping() {
        // Above 1.0 → clamped to 1.0
        let high = GrantProposal::new(
            "r1".to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints::default(),
            "test".to_string(),
            1.5,
            vec![],
        );
        assert_eq!(high.confidence, 1.0);

        // Below 0.0 → clamped to 0.0
        let low = GrantProposal::new(
            "r2".to_string(),
            PolicyOutcome::Deny,
            ProposedConstraints::default(),
            "test".to_string(),
            -0.5,
            vec![],
        );
        assert_eq!(low.confidence, 0.0);

        // In range → unchanged
        let mid = GrantProposal::new(
            "r3".to_string(),
            PolicyOutcome::Defer,
            ProposedConstraints::default(),
            "test".to_string(),
            0.75,
            vec![],
        );
        assert_eq!(mid.confidence, 0.75);
    }

    #[test]
    fn test_policy_decision_serialization() {
        let decision = PolicyDecision {
            id: DecisionId::new(),
            request_id: "req-456".to_string(),
            resource: "localhost://Users/self/Documents/docs/*".to_string(),
            action: "read".to_string(),
            outcome: PolicyOutcome::Grant,
            checks: vec![
                VerifierCheck {
                    name: "epoch_valid".to_string(),
                    passed: true,
                    reason: "Current epoch".to_string(),
                    severity: CheckSeverity::Blocking,
                },
                VerifierCheck {
                    name: "resource_allowed".to_string(),
                    passed: true,
                    reason: "No blocklist match".to_string(),
                    severity: CheckSeverity::Advisory,
                },
            ],
            effective_constraints: ProposedConstraints {
                resource_scope: None,
                ttl_secs: Some(600),
                max_uses: None,
                delegatable: false,
                max_classification: None,
            },
            effective_expiry: None,
            rationale: "All checks passed".to_string(),
            decided_at: SecureTimestamp::now(),
            shadow: false,
        };

        let json = serde_json::to_string(&decision).unwrap();
        let restored: PolicyDecision = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.request_id, "req-456");
        assert_eq!(restored.resource, "localhost://Users/self/Documents/docs/*");
        assert_eq!(restored.action, "read");
        assert_eq!(restored.outcome, PolicyOutcome::Grant);
        assert_eq!(restored.checks.len(), 2);
        assert!(!restored.shadow);
    }

    #[test]
    fn test_auto_grant_verifier_always_grants() {
        let verifier = AutoGrantVerifier;
        let request = make_test_request();
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert_eq!(decision.checks.len(), 1);
        assert!(decision.checks[0].passed);
        assert_eq!(decision.checks[0].name, "auto_grant");
        assert!(!decision.shadow);
    }

    #[test]
    fn test_auto_grant_verifier_shadow_flag() {
        let verifier = AutoGrantVerifier;
        let request = make_test_request();
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, true);
        assert!(decision.shadow);
        assert_eq!(decision.outcome, PolicyOutcome::Grant);

        let decision = verifier.verify(&request, &proposal, false);
        assert!(!decision.shadow);
    }

    #[test]
    fn test_evidence_record_serialization() {
        let record = EvidenceRecord {
            decision_id: "dec-789".to_string(),
            evidence_type: EvidenceType::RuntimeCheck,
            source: "runtime".to_string(),
            content: "Epoch is current".to_string(),
            recorded_at: SecureTimestamp::now(),
            confidence_delta: 0.1,
        };

        let json = serde_json::to_string(&record).unwrap();
        let restored: EvidenceRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.decision_id, "dec-789");
        assert_eq!(restored.evidence_type, EvidenceType::RuntimeCheck);
        assert_eq!(restored.confidence_delta, 0.1);
    }

    #[test]
    fn test_verifier_check_blocking_vs_advisory() {
        let blocking = VerifierCheck {
            name: "critical".to_string(),
            passed: false,
            reason: "Failed".to_string(),
            severity: CheckSeverity::Blocking,
        };

        let advisory = VerifierCheck {
            name: "optional".to_string(),
            passed: true,
            reason: "OK".to_string(),
            severity: CheckSeverity::Advisory,
        };

        let json_b = serde_json::to_string(&blocking).unwrap();
        let json_a = serde_json::to_string(&advisory).unwrap();

        assert!(json_b.contains("\"blocking\""));
        assert!(json_a.contains("\"advisory\""));

        let restored_b: VerifierCheck = serde_json::from_str(&json_b).unwrap();
        let restored_a: VerifierCheck = serde_json::from_str(&json_a).unwrap();

        assert_eq!(restored_b.severity, CheckSeverity::Blocking);
        assert_eq!(restored_a.severity, CheckSeverity::Advisory);
    }

    #[test]
    fn test_proposed_constraints_default() {
        let defaults = ProposedConstraints::default();
        assert!(defaults.resource_scope.is_none());
        assert!(defaults.ttl_secs.is_none());
        assert!(defaults.max_uses.is_none());
        assert!(!defaults.delegatable);
        assert!(defaults.max_classification.is_none());
    }

    #[test]
    fn test_turn_plan_serialization() {
        let plan = TurnPlan {
            request_id: "req-abc".to_string(),
            intent: "Read photo files".to_string(),
            objective: "Display photo gallery".to_string(),
            evidence_gaps: vec!["user_history".to_string()],
            recommended_action: PolicyOutcome::Grant,
            compiled_at: SecureTimestamp::now(),
        };

        let json = serde_json::to_string(&plan).unwrap();
        let restored: TurnPlan = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.request_id, "req-abc");
        assert_eq!(restored.intent, "Read photo files");
        assert_eq!(restored.recommended_action, PolicyOutcome::Grant);
        assert_eq!(restored.evidence_gaps.len(), 1);
    }

    #[test]
    fn test_policy_outcome_display() {
        assert_eq!(PolicyOutcome::Grant.to_string(), "grant");
        assert_eq!(PolicyOutcome::Deny.to_string(), "deny");
        assert_eq!(PolicyOutcome::Defer.to_string(), "defer");
    }

    #[test]
    fn test_policy_outcome_serde_roundtrip() {
        let grant_json = serde_json::to_string(&PolicyOutcome::Grant).unwrap();
        assert_eq!(grant_json, "\"grant\"");
        let restored: PolicyOutcome = serde_json::from_str(&grant_json).unwrap();
        assert_eq!(restored, PolicyOutcome::Grant);

        let deny_json = serde_json::to_string(&PolicyOutcome::Deny).unwrap();
        assert_eq!(deny_json, "\"deny\"");
        let restored: PolicyOutcome = serde_json::from_str(&deny_json).unwrap();
        assert_eq!(restored, PolicyOutcome::Deny);

        let defer_json = serde_json::to_string(&PolicyOutcome::Defer).unwrap();
        assert_eq!(defer_json, "\"defer\"");
        let restored: PolicyOutcome = serde_json::from_str(&defer_json).unwrap();
        assert_eq!(restored, PolicyOutcome::Defer);
    }

    #[test]
    fn test_check_severity_serde() {
        let blocking_json = serde_json::to_string(&CheckSeverity::Blocking).unwrap();
        assert_eq!(blocking_json, "\"blocking\"");
        let restored: CheckSeverity = serde_json::from_str(&blocking_json).unwrap();
        assert_eq!(restored, CheckSeverity::Blocking);

        let advisory_json = serde_json::to_string(&CheckSeverity::Advisory).unwrap();
        assert_eq!(advisory_json, "\"advisory\"");
        let restored: CheckSeverity = serde_json::from_str(&advisory_json).unwrap();
        assert_eq!(restored, CheckSeverity::Advisory);
    }

    #[test]
    fn test_evidence_type_serde() {
        let types = [
            (EvidenceType::RuntimeCheck, "\"runtime_check\""),
            (EvidenceType::UserConfirm, "\"user_confirm\""),
            (EvidenceType::HistoryMatch, "\"history_match\""),
            (EvidenceType::PolicyRule, "\"policy_rule\""),
        ];

        for (variant, expected_json) in types {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json);
            let restored: EvidenceType = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, variant);
        }
    }

    // --- RulesVerifier tests (Wedge-4) ---

    fn make_request_with_resource(resource: &str) -> PendingCapabilityRequest {
        PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new(resource),
            Action::Read,
            300,
        )
    }

    fn make_request_with_action(action: Action) -> PendingCapabilityRequest {
        PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/*"),
            action,
            300,
        )
    }

    #[test]
    fn test_rules_verifier_all_pass_echoes_proposal() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert!(decision
            .checks
            .iter()
            .all(|c| c.passed || c.severity == CheckSeverity::Advisory));
    }

    #[test]
    fn test_rules_verifier_resource_blocklist_denies() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("localhost://Users/self/Documents/admin/settings");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        assert!(decision.rationale.contains("admin"));
    }

    #[test]
    fn test_rules_verifier_action_blocklist_denies() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Admin);
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        assert!(decision.rationale.contains("admin"));
    }

    #[test]
    fn test_rules_verifier_low_confidence_advisory() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints::default(),
            "Low confidence".to_string(),
            0.3,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        // Advisory failure doesn't block — still echoes proposal
        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        let confidence_check = decision
            .checks
            .iter()
            .find(|c| c.name == "confidence_threshold")
            .unwrap();
        assert!(!confidence_check.passed);
        assert_eq!(confidence_check.severity, CheckSeverity::Advisory);
    }

    #[test]
    fn test_rules_verifier_max_ttl_advisory() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                ttl_secs: Some(100_000),
                ..Default::default()
            },
            "Long TTL".to_string(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        let ttl_check = decision
            .checks
            .iter()
            .find(|c| c.name == "max_ttl")
            .unwrap();
        assert!(!ttl_check.passed);
        assert_eq!(ttl_check.severity, CheckSeverity::Advisory);
    }

    #[test]
    fn test_rules_verifier_no_delegation_advisory() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                delegatable: true,
                ..Default::default()
            },
            "Delegatable".to_string(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        let deleg_check = decision
            .checks
            .iter()
            .find(|c| c.name == "no_delegation")
            .unwrap();
        assert!(!deleg_check.passed);
        assert_eq!(deleg_check.severity, CheckSeverity::Advisory);
    }

    #[test]
    fn test_rules_verifier_deny_proposal_preserved() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        // Proposal recommends Deny, all rules pass → outcome stays Deny (never loosens)
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Deny,
            ProposedConstraints::default(),
            "User denied".to_string(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_rules_verifier_shadow_flag_propagated() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, true);
        assert!(decision.shadow);

        let decision = verifier.verify(&request, &proposal, false);
        assert!(!decision.shadow);
    }

    #[test]
    fn test_rules_verifier_empty_rules_echoes_proposal() {
        let verifier = RulesVerifier::new(vec![]);
        let request = make_test_request();
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert!(decision.checks.is_empty());
    }

    #[test]
    fn test_rules_verifier_multiple_blocking_failures() {
        let verifier = RulesVerifier::with_defaults();
        // Resource blocklist + action blocklist both fail
        let request = PendingCapabilityRequest::new(
            SessionId::from_string("test-session"),
            ResourceId::new("localhost://Users/self/Documents/admin/data"),
            Action::Admin,
            300,
        );
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        let blocking_failures: Vec<_> = decision
            .checks
            .iter()
            .filter(|c| !c.passed && c.severity == CheckSeverity::Blocking)
            .collect();
        assert_eq!(blocking_failures.len(), 2);
    }

    #[test]
    fn test_rules_verifier_blocklist_catches_terminal_segment() {
        let verifier = RulesVerifier::with_defaults();
        // Resource ends with "admin" (no trailing path)
        let request = make_request_with_resource("localhost://Users/self/Documents/admin");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_rules_verifier_blocklist_catches_nested_segment() {
        let verifier = RulesVerifier::with_defaults();
        // "admin" appears in the middle of the path
        let request = make_request_with_resource("localhost://Users/self/Documents/admin/foo/bar");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_rules_verifier_blocklist_no_false_match() {
        let verifier = RulesVerifier::with_defaults();
        // "administrator" should NOT be blocked by "admin" rule (segment-aware matching)
        let request =
            make_request_with_resource("localhost://Users/self/Documents/administrator/data");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
    }

    #[test]
    fn test_rules_verifier_defer_becomes_deny() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        // Proposal recommends Defer — rules verifier must never produce Defer
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Defer,
            ProposedConstraints::default(),
            "Uncertain".to_string(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        assert_ne!(decision.outcome, PolicyOutcome::Defer);
    }

    #[test]
    fn test_rules_verifier_advisory_failure_rationale_mentions_warnings() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_test_request();
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                delegatable: true,
                ..Default::default()
            },
            "Test".to_string(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Grant);
        assert!(decision.rationale.contains("Advisory warnings"));
        assert!(!decision.rationale.starts_with("All rules passed"));
    }

    // --- AI-specific RuleCheck tests ---

    #[test]
    fn test_scheme_allowlist_passes_allowed_backend() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat_completions");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scheme_allowlist")
            .unwrap();
        assert!(
            check.passed,
            "allowed backend should pass: {}",
            check.reason
        );
    }

    #[test]
    fn test_scheme_allowlist_passes_codex_backend() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/codex/chat_completions");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scheme_allowlist")
            .unwrap();
        assert!(
            check.passed,
            "codex backend should pass allowlist: {}",
            check.reason
        );
    }

    #[test]
    fn test_scheme_allowlist_blocks_unknown_backend() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/rogue/chat");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scheme_allowlist")
            .unwrap();
        assert!(!check.passed);
        assert!(check.reason.contains("not in allowlist"));
    }

    #[test]
    fn test_scheme_allowlist_skips_non_ai_resource() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("localhost://Users/self/Documents/photos/*");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scheme_allowlist")
            .unwrap();
        assert!(
            check.passed,
            "non-AI resource should skip: {}",
            check.reason
        );
    }

    #[test]
    fn test_max_uses_for_scheme_warns_missing() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat");
        // Default proposal has no max_uses
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_uses")
            .unwrap();
        assert!(!check.passed);
        assert!(check.reason.contains("no max_uses"));
    }

    #[test]
    fn test_max_uses_for_scheme_warns_excessive() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat");
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                max_uses: Some(200),
                ..Default::default()
            },
            "test".into(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_uses")
            .unwrap();
        assert!(!check.passed);
        assert!(check.reason.contains("exceeds limit"));
    }

    #[test]
    fn test_max_uses_for_scheme_passes_within_limit() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat");
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                max_uses: Some(50),
                ..Default::default()
            },
            "test".into(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_uses")
            .unwrap();
        assert!(check.passed);
    }

    #[test]
    fn test_max_uses_for_scheme_skips_non_ai() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("localhost://Users/self/Documents/photos/*");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_uses")
            .unwrap();
        assert!(check.passed, "non-AI resource should skip");
    }

    #[test]
    fn test_max_ttl_for_scheme_warns_excessive() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat");
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                ttl_secs: Some(7200),
                ..Default::default()
            },
            "test".into(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_ttl")
            .unwrap();
        assert!(!check.passed);
        assert!(check.reason.contains("exceeds AI limit"));
    }

    #[test]
    fn test_max_ttl_for_scheme_passes_within_limit() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat");
        let proposal = GrantProposal::new(
            request.id.to_string(),
            PolicyOutcome::Grant,
            ProposedConstraints {
                ttl_secs: Some(1800),
                ..Default::default()
            },
            "test".into(),
            0.9,
            vec![],
        );

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_max_ttl")
            .unwrap();
        assert!(check.passed);
    }

    #[test]
    fn test_scope_narrowing_flags_broad_ai_wildcard() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/venice/chat");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scope_narrowing")
            .unwrap();
        assert!(
            !check.passed,
            "broad AI scope should trigger narrowing advisory"
        );
        assert!(check.reason.contains("narrowed"));
    }

    #[test]
    fn test_scope_narrowing_passes_narrow_local() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/local/chat_completions");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scope_narrowing")
            .unwrap();
        assert!(
            check.passed,
            "narrow local scope should pass: {}",
            check.reason
        );
    }

    #[test]
    fn test_scope_narrowing_skips_non_matching() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("localhost://Users/self/Documents/photos/*");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scope_narrowing")
            .unwrap();
        assert!(check.passed, "non-AI resource should skip");
    }

    #[test]
    fn test_defaults_deny_rogue_ai_backend() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/rogue/chat");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(
            decision.outcome,
            PolicyOutcome::Deny,
            "Unknown AI backend should be denied by defaults"
        );
    }

    #[test]
    fn test_scheme_allowlist_rejects_prefix_bypass() {
        // "locality" starts with "local" but is NOT the "local" backend
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_resource("elastos://ai/locality/chat_completions");
        let proposal = make_test_proposal(request.id.as_str());

        let decision = verifier.verify(&request, &proposal, false);

        assert_eq!(
            decision.outcome,
            PolicyOutcome::Deny,
            "locality should not pass the local allowlist"
        );
        let check = decision
            .checks
            .iter()
            .find(|c| c.name == "ai_scheme_allowlist")
            .unwrap();
        assert!(!check.passed);
        assert!(check.reason.contains("locality"));
    }

    // === Denial-path tests: shell refusing each action class ===

    fn make_deny_proposal(request_id: &str) -> GrantProposal {
        GrantProposal::new(
            request_id.to_string(),
            PolicyOutcome::Deny,
            ProposedConstraints::default(),
            "Shell refused this request".to_string(),
            1.0,
            vec![],
        )
    }

    #[test]
    fn test_shell_denies_read_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Read);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_shell_denies_write_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Write);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_shell_denies_execute_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Execute);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_shell_denies_message_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Message);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_shell_denies_delete_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Delete);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_shell_denies_admin_action() {
        let verifier = RulesVerifier::with_defaults();
        let request = make_request_with_action(Action::Admin);
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn test_deny_proposal_overrides_passing_rules() {
        // Even if rules pass, a Deny proposal from the shell must deny
        let verifier = RulesVerifier::with_defaults();
        // This resource is NOT blocked by any rule
        let request = PendingCapabilityRequest::new(
            SessionId::from_string("deny-session"),
            ResourceId::new("localhost://Users/self/Documents/photos/cat.jpg"),
            Action::Read,
            300,
        );
        let proposal = make_deny_proposal(request.id.as_str());
        let decision = verifier.verify(&request, &proposal, false);
        assert_eq!(
            decision.outcome,
            PolicyOutcome::Deny,
            "Shell deny must override rule-level pass"
        );
    }
}
