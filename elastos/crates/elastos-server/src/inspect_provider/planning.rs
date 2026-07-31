use std::sync::Arc;

use elastos_runtime::invoke;
use serde_json::{json, Value};

use super::{error, ok, InspectSource};
use crate::provider_resource::{
    build_capability_resource, ensure_generic_wallet_capability, provider_operation_action,
};

fn preview_execution_policy() -> Value {
    json!({
        "schema": "elastos.inspect.execution-policy/v1",
        "mode": "preview_only",
        "can_dispatch": false,
        "can_mutate": false,
        "approval_surface": Value::Null,
    })
}

pub(super) async fn plan(source: &Arc<dyn InspectSource>, request: &Value) -> Value {
    let operation = request
        .get("operation")
        .or_else(|| request.get("provider_operation"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if operation.is_empty() {
        return error("invalid_request", "inspect plan requires operation");
    }

    if let Some(scheme) = request.get("scheme").and_then(Value::as_str) {
        let body = request.get("request").cloned().unwrap_or_else(|| json!({}));
        return match build_capability_resource(scheme, operation, &body) {
            Ok(resource) => {
                let action = provider_operation_action(scheme, operation);
                if let Some(action) = action {
                    if let Err(message) = ensure_generic_wallet_capability(&resource, action) {
                        return error("invalid_request", &message);
                    }
                }
                ok(json!({
                    "schema": "elastos.inspect.gate-preview/v1",
                    "mode": "provider_resource",
                    "provider": scheme,
                    "operation": operation,
                    "capabilities": [{
                        "resource": resource,
                        "actions": action
                            .map(|action| vec![action.to_string()])
                            .unwrap_or_default(),
                    }],
                    "execution": preview_execution_policy(),
                    "dispatch": false,
                }))
            }
            Err(message) => error("invalid_request", &message),
        };
    }

    let id = request
        .get("id")
        .or_else(|| request.get("capsule_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.is_empty() {
        return error("invalid_request", "inspect plan requires id or scheme");
    }
    let Some(entry) = source.inspect_get(id).await else {
        return error("not_found", "inspect target not found");
    };
    let Some(authority) = entry
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.authority.as_ref())
    else {
        return error(
            "no_authority",
            "inspect target has no provider authority metadata",
        );
    };
    match invoke::plan_provider_operation(authority, operation) {
        Ok(plan) => ok(json!({
            "schema": "elastos.inspect.gate-preview/v1",
            "mode": "provider_authority",
            "id": entry.id,
            "provider": entry.name,
            "operation": operation,
            "capabilities": plan.resources.iter().map(|resource| json!({
                "resource": resource,
                "actions": plan.actions.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "audit_events": plan.audit_events,
            "execution": preview_execution_policy(),
            "dispatch": false,
        })),
        Err(invoke::InvokeError::UnknownOperation(_)) => error(
            "unknown_operation",
            "operation is not declared by target authority",
        ),
        Err(invoke::InvokeError::UnknownDeclaredAction(action)) => error(
            "invalid_authority",
            &format!("unknown declared action: {action}"),
        ),
        Err(_) => error("invalid_authority", "invalid authority metadata"),
    }
}
