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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn affordance(value: serde_json::Value) -> CapsuleAffordanceDescriptor {
        serde_json::from_value(value).expect("affordance descriptor")
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
            InvokeError::InputTypeMismatch { expected: "object".to_string() }
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
}
