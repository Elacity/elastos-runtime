//! Metadata-driven invocation planning.
//!
//! This is the read-only "preview" half: validate declared argument shape and
//! reflect the capability/audit gate before any dispatch. It is deliberately
//! transport-agnostic; Carrier, browser gateway, and local provider calls use
//! the plan, not separate policy guesses.

use elastos_common::{
    AffordanceApprovalMode, AffordanceAuditMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    ProviderAuthority,
};
use serde_json::Value;

use crate::capability::Action;

/// Why a preview cannot produce a safe invocation plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeError {
    InputTypeMismatch { expected: String },
    MissingRequiredField(String),
    UnknownOperation(String),
    UnknownDeclaredAction(String),
}

/// Preview for an affordance method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationPlan {
    pub capability_action: Action,
    pub approval: AffordanceApprovalMode,
    pub audit: AffordanceAuditMode,
}

/// Preview for a provider operation declared in manifest authority metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperationPlan {
    pub resources: Vec<String>,
    pub actions: Vec<Action>,
    pub audit_events: Vec<String>,
}

/// Map declared affordance risk to the minimum capability action.
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

/// Minimal JSON-schema check: top-level `type` plus `required` object fields.
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
            _ => true,
        };
        if !ok {
            return Err(InvokeError::InputTypeMismatch {
                expected: expected.to_string(),
            });
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let object = args.as_object();
        for field in required {
            if let Some(name) = field.as_str() {
                if !object
                    .map(|object| object.contains_key(name))
                    .unwrap_or(false)
                {
                    return Err(InvokeError::MissingRequiredField(name.to_string()));
                }
            }
        }
    }
    Ok(())
}

/// Plan an affordance call from its metadata and proposed arguments.
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

fn parse_action(value: &str) -> Result<Action, InvokeError> {
    match value {
        "read" => Ok(Action::Read),
        "write" => Ok(Action::Write),
        "execute" => Ok(Action::Execute),
        "message" => Ok(Action::Message),
        "delete" => Ok(Action::Delete),
        "admin" => Ok(Action::Admin),
        other => Err(InvokeError::UnknownDeclaredAction(other.to_string())),
    }
}

/// Plan a provider operation from manifest `authority` metadata.
pub fn plan_provider_operation(
    authority: &ProviderAuthority,
    operation: &str,
) -> Result<ProviderOperationPlan, InvokeError> {
    let mut resources = Vec::new();
    let mut actions = Vec::new();
    for capability in authority
        .capabilities
        .iter()
        .filter(|capability| capability.operations.iter().any(|op| op == operation))
    {
        if !resources.contains(&capability.resource) {
            resources.push(capability.resource.clone());
        }
        for action in &capability.actions {
            let action = parse_action(action)?;
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
    }
    if resources.is_empty() {
        return Err(InvokeError::UnknownOperation(operation.to_string()));
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
    fn affordance_plan_validates_required_fields_before_preview() {
        let affordance = affordance(json!({
            "id": "send",
            "risk": "write",
            "approval": "user",
            "audit": "event",
            "input_schema": { "type": "object", "required": ["to", "body"] }
        }));
        assert_eq!(
            plan(&affordance, &json!({"to": "alice"})).unwrap_err(),
            InvokeError::MissingRequiredField("body".to_string())
        );
        let ok = plan(&affordance, &json!({"to": "alice", "body": "hi"})).unwrap();
        assert_eq!(ok.capability_action, Action::Write);
        assert_eq!(ok.approval, AffordanceApprovalMode::User);
    }

    #[test]
    fn provider_operation_plan_reflects_authority_union() {
        let authority = authority(json!({
            "reason": "release content keys",
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["read"], "operations": ["release"] },
                { "resource": "elastos://decrypt/*", "actions": ["execute", "admin"], "operations": ["release"] }
            ],
            "audit_events": ["key.release.denied", "key.release.granted"]
        }));
        let plan = plan_provider_operation(&authority, "release").unwrap();
        assert_eq!(
            plan.resources,
            vec![
                "elastos://key/*".to_string(),
                "elastos://decrypt/*".to_string()
            ]
        );
        assert_eq!(
            plan.actions,
            vec![Action::Read, Action::Execute, Action::Admin]
        );
        assert!(plan
            .audit_events
            .iter()
            .any(|event| event == "key.release.denied"));
    }

    #[test]
    fn provider_operation_plan_fails_closed_on_unknown_action() {
        let authority = authority(json!({
            "reason": "x",
            "capabilities": [
                { "resource": "elastos://key/*", "actions": ["teleport"], "operations": ["release"] }
            ],
            "audit_events": ["key.release.denied"]
        }));
        assert_eq!(
            plan_provider_operation(&authority, "release").unwrap_err(),
            InvokeError::UnknownDeclaredAction("teleport".to_string())
        );
    }
}
