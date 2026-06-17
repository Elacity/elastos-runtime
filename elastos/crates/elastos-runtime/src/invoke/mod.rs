//! Metadata-driven invocation planning (prototype).
//!
//! Given a capsule's typed affordance metadata, derive how a call must be
//! validated and gated *before* any dispatch:
//!
//! 1. validate the arguments against the affordance's declared `input_schema`;
//! 2. derive the policy gate — the capability [`Action`] it requires, whether
//!    it needs approval, and how it must be audited — from `risk`/`approval`/
//!    `audit`.
//!
//! The metadata *drives* both. This is the reflective kernel behind Elastos's
//! Component Assembly Runtime (CAR) idea — "metadata-driven reflection" — and
//! the bridge from inspection (read the contract) to invocation (act on it).
//!
//! Pure and transport-agnostic by design: argument marshalling, dispatch, and
//! the location-agnostic Carrier transport are deliberately out of scope here
//! (that architecture is to be planned, not assumed). This is the decision core
//! a future invoker would call, mirroring [`crate::inspect`].

use elastos_common::{
    AffordanceApprovalMode, AffordanceAuditMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    ProviderAuthority,
};
use serde_json::Value;

use crate::capability::token::Action;

/// Why a proposed invocation is rejected before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeError {
    /// Arguments did not match the declared input `type`.
    InputTypeMismatch { expected: String },
    /// A required input field was missing.
    MissingRequiredField(String),
    /// The operation is not declared by any of the provider's capability blocks.
    UnknownOperation(String),
    /// The manifest declares an action string the capability layer does not know.
    /// Surfaced (not silently dropped) so the gate is never under-stated.
    UnknownDeclaredAction(String),
}

/// The validated plan for an invocation: the capability action it requires,
/// whether it needs approval, and how it must be audited. Derived entirely from
/// the affordance metadata — a dispatcher MUST enforce all three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationPlan {
    pub capability_action: Action,
    pub approval: AffordanceApprovalMode,
    pub audit: AffordanceAuditMode,
}

/// Map an affordance risk class to the capability action it requires. Read-only
/// affordances need `Read`; anything that mutates or actuates needs a stronger
/// action so the capability layer gates it correctly (fail-closed by class).
pub fn required_action(risk: &AffordanceRisk) -> Action {
    match risk {
        AffordanceRisk::Read => Action::Read,
        AffordanceRisk::Write => Action::Write,
        AffordanceRisk::Launch | AffordanceRisk::Actuator => Action::Execute,
        AffordanceRisk::Payment | AffordanceRisk::Rights | AffordanceRisk::Privileged => {
            Action::Admin
        }
    }
}

/// Minimal, metadata-driven validation of `args` against an `input_schema`.
///
/// Prototype scope: checks the top-level JSON `type` and any `required` fields —
/// the subset that demonstrates schema-driven marshalling. A full JSON Schema
/// validator is intentionally out of scope. An absent schema means "untyped":
/// nothing to check.
pub fn validate_input(input_schema: Option<&Value>, args: &Value) -> Result<(), InvokeError> {
    let Some(schema) = input_schema else {
        return Ok(());
    };

    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let ok = match expected {
            "object" => args.is_object(),
            "array" => args.is_array(),
            "string" => args.is_string(),
            "number" => args.is_number(),
            "integer" => args.is_i64() || args.is_u64(),
            "boolean" => args.is_boolean(),
            "null" => args.is_null(),
            _ => true, // unknown type keyword: don't reject in the prototype
        };
        if !ok {
            return Err(InvokeError::InputTypeMismatch {
                expected: expected.to_string(),
            });
        }
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let obj = args.as_object();
        for field in required {
            if let Some(name) = field.as_str() {
                let present = obj.map(|o| o.contains_key(name)).unwrap_or(false);
                if !present {
                    return Err(InvokeError::MissingRequiredField(name.to_string()));
                }
            }
        }
    }

    Ok(())
}

/// Plan an invocation from the affordance metadata and proposed arguments:
/// validate the input shape, then derive the capability/approval/audit gate.
/// Returns the plan a dispatcher must enforce; it does not itself dispatch.
pub fn plan(
    affordance: &CapsuleAffordanceDescriptor,
    args: &Value,
) -> Result<InvocationPlan, InvokeError> {
    validate_input(affordance.input_schema.as_ref(), args)?;
    Ok(InvocationPlan {
        capability_action: required_action(&affordance.risk),
        approval: affordance.approval.clone(),
        audit: affordance.audit.clone(),
    })
}

/// The capability gate a provider *operation* would require, derived from the
/// capsule's self-describing `authority` metadata — the resource it touches, the
/// action(s) a caller's capability must cover, and the audit events it emits.
///
/// This is the provider-side twin of [`InvocationPlan`]: where `plan` reflects an
/// `interfaces[].methods` affordance, this reflects an `authority.capabilities[]`
/// operation (how DDRM providers — key release, decrypt, rights, chain — declare
/// their powers). It dispatches nothing; it answers "what authority would this
/// ask for?" straight from the manifest, so the preview can never under-state a
/// gate the runtime would later enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperationPlan {
    /// Every capability resource URI the operation acts on (e.g. `elastos://key/*`).
    /// Surfaced as the union across *all* capability blocks that declare the
    /// operation — never just the first — so a manifest that splits an operation
    /// across blocks cannot hide a resource the call also requires.
    pub resources: Vec<String>,
    /// Every action a caller's capability must cover. The union across all
    /// matching blocks, surfaced whole (not collapsed to one), so nothing the
    /// manifest demands is hidden from the reviewer. Fail-closed: an unrecognised
    /// action keyword in *any* matching block is an error, not a silent drop.
    pub actions: Vec<Action>,
    /// The audit events this provider declares it emits.
    pub audit_events: Vec<String>,
}

/// Parse a manifest-declared action string into a capability [`Action`],
/// fail-closed: an unrecognised keyword is an error, never a silent no-op.
fn parse_action(s: &str) -> Result<Action, InvokeError> {
    match s {
        "read" => Ok(Action::Read),
        "write" => Ok(Action::Write),
        "execute" => Ok(Action::Execute),
        "message" => Ok(Action::Message),
        "delete" => Ok(Action::Delete),
        "admin" => Ok(Action::Admin),
        other => Err(InvokeError::UnknownDeclaredAction(other.to_string())),
    }
}

/// Preview the capability gate a provider `operation` would require, by reflecting
/// the capsule's `authority` metadata. Aggregates *every* capability block that
/// declares the operation and returns the union of their resources + required
/// actions, plus the authority's audit events. Dispatches nothing.
///
/// Fail-closed by construction: surfacing the union (not the first match) means
/// the preview can never under-state the authority a call needs, even if the
/// manifest splits one operation across several blocks.
pub fn plan_provider_operation(
    authority: &ProviderAuthority,
    operation: &str,
) -> Result<ProviderOperationPlan, InvokeError> {
    let matching: Vec<&_> = authority
        .capabilities
        .iter()
        .filter(|cap| cap.operations.iter().any(|op| op == operation))
        .collect();

    if matching.is_empty() {
        return Err(InvokeError::UnknownOperation(operation.to_string()));
    }

    let mut resources: Vec<String> = Vec::new();
    let mut actions: Vec<Action> = Vec::new();
    for cap in matching {
        if !resources.contains(&cap.resource) {
            resources.push(cap.resource.clone());
        }
        for a in &cap.actions {
            let action = parse_action(a)?; // unknown keyword in any block → error
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
    }

    Ok(ProviderOperationPlan {
        resources,
        actions,
        audit_events: authority.audit_events.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn affordance(value: serde_json::Value) -> CapsuleAffordanceDescriptor {
        serde_json::from_value(value).expect("affordance descriptor")
    }

    fn authority(value: serde_json::Value) -> ProviderAuthority {
        serde_json::from_value(value).expect("provider authority")
    }

    #[test]
    fn risk_maps_to_capability_action() {
        assert_eq!(required_action(&AffordanceRisk::Read), Action::Read);
        assert_eq!(required_action(&AffordanceRisk::Write), Action::Write);
        assert_eq!(required_action(&AffordanceRisk::Launch), Action::Execute);
        assert_eq!(required_action(&AffordanceRisk::Actuator), Action::Execute);
        assert_eq!(required_action(&AffordanceRisk::Payment), Action::Admin);
        assert_eq!(required_action(&AffordanceRisk::Rights), Action::Admin);
        assert_eq!(required_action(&AffordanceRisk::Privileged), Action::Admin);
    }

    #[test]
    fn plan_derives_gate_for_read_affordance() {
        let a = affordance(json!({
            "id": "history", "risk": "read", "approval": "none", "audit": "summary"
        }));
        let plan = plan(&a, &json!({})).unwrap();
        assert_eq!(plan.capability_action, Action::Read);
        assert_eq!(plan.approval, AffordanceApprovalMode::None);
        assert_eq!(plan.audit, AffordanceAuditMode::Summary);
    }

    #[test]
    fn plan_derives_user_approval_for_payment() {
        let a = affordance(json!({
            "id": "pay", "risk": "payment", "approval": "user", "audit": "full"
        }));
        let plan = plan(&a, &json!({ "amount": 10 })).unwrap();
        assert_eq!(plan.capability_action, Action::Admin);
        assert_eq!(plan.approval, AffordanceApprovalMode::User);
        assert_eq!(plan.audit, AffordanceAuditMode::Full);
    }

    #[test]
    fn input_schema_required_field_is_enforced() {
        let a = affordance(json!({
            "id": "send", "risk": "write", "approval": "user", "audit": "event",
            "input_schema": { "type": "object", "required": ["to", "body"] }
        }));
        // Missing "body".
        let err = plan(&a, &json!({ "to": "alice" })).unwrap_err();
        assert_eq!(err, InvokeError::MissingRequiredField("body".to_string()));
        // Complete args pass and yield the write gate.
        let ok = plan(&a, &json!({ "to": "alice", "body": "hi" })).unwrap();
        assert_eq!(ok.capability_action, Action::Write);
    }

    #[test]
    fn input_schema_type_mismatch_is_rejected() {
        let a = affordance(json!({
            "id": "send", "risk": "write", "approval": "none", "audit": "none",
            "input_schema": { "type": "object" }
        }));
        let err = plan(&a, &json!("not-an-object")).unwrap_err();
        assert_eq!(
            err,
            InvokeError::InputTypeMismatch {
                expected: "object".to_string()
            }
        );
    }

    #[test]
    fn untyped_affordance_accepts_any_args() {
        let a = affordance(json!({
            "id": "ping", "risk": "read", "approval": "none", "audit": "none"
        }));
        assert!(plan(&a, &json!({ "anything": [1, 2, 3] })).is_ok());
        assert!(plan(&a, &json!("scalar")).is_ok());
    }

    #[test]
    fn provider_op_plan_reflects_the_authority_block() {
        // Mirrors DDRM's key-provider: status is Read, release is Execute, on the
        // same elastos://key/* resource, with declared audit events.
        let auth = authority(json!({
            "reason": "release content keys to authorized renderers",
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["read"], "operations": ["status"] },
                { "resource": "elastos://key/*", "actions": ["execute"], "operations": ["release"] }
            ],
            "audit_events": ["key.release.denied", "key.release.granted"]
        }));

        let release = plan_provider_operation(&auth, "release").unwrap();
        assert_eq!(release.resources, vec!["elastos://key/*".to_string()]);
        assert_eq!(release.actions, vec![Action::Execute]);
        assert!(release
            .audit_events
            .iter()
            .any(|e| e == "key.release.denied"));

        let status = plan_provider_operation(&auth, "status").unwrap();
        assert_eq!(status.actions, vec![Action::Read]);
    }

    #[test]
    fn provider_op_plan_aggregates_split_blocks_fail_closed() {
        // Hardening: an operation declared across MULTIPLE capability blocks must
        // surface the UNION of resources and actions — never just the first match
        // — so a split-privilege manifest cannot trick the preview into
        // under-stating the authority a call actually requires.
        let auth = authority(json!({
            "reason": "x", "audit_events": ["a"],
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["read"], "operations": ["release"] },
                { "resource": "elastos://decrypt/*", "actions": ["execute", "admin"],
                  "operations": ["release", "render"] }
            ]
        }));
        let plan = plan_provider_operation(&auth, "release").unwrap();
        // Both resources are surfaced (union, deduped, order-preserving).
        assert_eq!(
            plan.resources,
            vec![
                "elastos://key/*".to_string(),
                "elastos://decrypt/*".to_string()
            ]
        );
        // The full action set across both blocks.
        assert_eq!(
            plan.actions,
            vec![Action::Read, Action::Execute, Action::Admin]
        );
    }

    #[test]
    fn provider_op_plan_fails_closed_when_any_matching_block_has_unknown_action() {
        // If ANY matching block declares an unrecognised action, the whole preview
        // errors — we never quietly report only the blocks we understood.
        let auth = authority(json!({
            "reason": "x", "audit_events": ["a"],
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["read"], "operations": ["release"] },
                { "resource": "elastos://key/*", "actions": ["teleport"], "operations": ["release"] }
            ]
        }));
        assert_eq!(
            plan_provider_operation(&auth, "release").unwrap_err(),
            InvokeError::UnknownDeclaredAction("teleport".to_string())
        );
    }

    #[test]
    fn provider_op_plan_rejects_unknown_operation() {
        let auth = authority(json!({
            "reason": "x", "audit_events": ["a"],
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["read"], "operations": ["status"] }
            ]
        }));
        assert_eq!(
            plan_provider_operation(&auth, "release").unwrap_err(),
            InvokeError::UnknownOperation("release".to_string())
        );
    }

    #[test]
    fn provider_op_plan_fails_closed_on_unknown_action() {
        // A manifest action keyword the capability layer does not know must error,
        // never be silently dropped (which would under-state the gate).
        let auth = authority(json!({
            "reason": "x", "audit_events": ["a"],
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["teleport"], "operations": ["release"] }
            ]
        }));
        assert_eq!(
            plan_provider_operation(&auth, "release").unwrap_err(),
            InvokeError::UnknownDeclaredAction("teleport".to_string())
        );
    }

    #[test]
    fn provider_op_plan_surfaces_full_action_set() {
        let auth = authority(json!({
            "reason": "x", "audit_events": ["a"],
            "capabilities": [
                { "resource": "elastos://chain/*", "actions": ["execute", "admin"],
                  "operations": ["broadcast_transaction"] }
            ]
        }));
        let plan = plan_provider_operation(&auth, "broadcast_transaction").unwrap();
        assert_eq!(plan.actions, vec![Action::Execute, Action::Admin]);
    }
}
