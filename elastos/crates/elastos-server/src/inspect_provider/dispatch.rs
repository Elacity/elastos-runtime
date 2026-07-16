use std::sync::{Arc, Weak};

use elastos_common::CapsuleManifest;
use elastos_runtime::invoke;
use elastos_runtime::provider::{
    ProviderInvocation, ProviderInvocationTransport, ProviderRegistry, ProviderTransfer,
};
use serde_json::{json, Value};

use super::{error, ok, InspectSource};

fn approved_execution_policy() -> Value {
    json!({
        "schema": "elastos.inspect.execution-policy/v1",
        "mode": "approved_dispatch",
        "can_dispatch": true,
        "can_mutate": true,
        "approval_surface": "inbox",
    })
}

fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
    manifest
        .provides
        .as_ref()?
        .strip_prefix("elastos://")
        .and_then(|rest| rest.split('/').next())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn inspect_hidden_runtime_metadata_field(request: &Value) -> Option<&str> {
    request
        .as_object()?
        .keys()
        .find(|key| {
            key.starts_with("_runtime")
                || matches!(key.as_str(), "connect_ticket" | "carrier_route" | "carrier")
        })
        .map(String::as_str)
}

pub(super) async fn dispatch_approved(
    source: &Arc<dyn InspectSource>,
    registry: &Weak<ProviderRegistry>,
    request: &Value,
) -> Value {
    let id = request
        .get("id")
        .or_else(|| request.get("capsule_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.is_empty() {
        return error("invalid_request", "inspect dispatch requires id");
    }
    let operation = request
        .get("operation")
        .or_else(|| request.get("provider_operation"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if operation.is_empty() {
        return error("invalid_request", "inspect dispatch requires operation");
    }
    let Some(entry) = source.inspect_get(id).await else {
        return error("not_found", "inspect target not found");
    };
    let Some(manifest) = entry.manifest.as_ref() else {
        return error(
            "invalid_target",
            "inspect dispatch target is not manifest-backed",
        );
    };
    let Some(authority) = manifest.authority.as_ref() else {
        return error(
            "no_authority",
            "inspect target has no provider authority metadata",
        );
    };
    let Some(target) = provided_scheme(manifest) else {
        return error(
            "invalid_target",
            "inspect dispatch target has no provider scheme",
        );
    };
    let plan = match invoke::plan_provider_operation(authority, operation) {
        Ok(plan) => plan,
        Err(invoke::InvokeError::UnknownOperation(_)) => {
            return error(
                "unknown_operation",
                "operation is not declared by target authority",
            )
        }
        Err(invoke::InvokeError::UnknownDeclaredAction(action)) => {
            return error(
                "invalid_authority",
                &format!("unknown declared action: {action}"),
            )
        }
        Err(_) => return error("invalid_authority", "invalid authority metadata"),
    };
    let mut provider_request = request.get("request").cloned().unwrap_or_else(|| json!({}));
    if let Some(field) = inspect_hidden_runtime_metadata_field(&provider_request) {
        return error(
            "invalid_request",
            &format!("inspect dispatch request must not predeclare Runtime metadata field {field}"),
        );
    }
    let Some(provider_object) = provider_request.as_object_mut() else {
        return error(
            "invalid_request",
            "inspect dispatch request must be a JSON object",
        );
    };
    provider_object.insert("op".to_string(), Value::String(operation.to_string()));

    let Some(registry) = registry.upgrade() else {
        return error(
            "runtime_unavailable",
            "inspect dispatch registry unavailable",
        );
    };
    match registry
        .invoke_provider(ProviderInvocation {
            source: "inspect".to_string(),
            target: target.clone(),
            op: operation.to_string(),
            request: provider_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
    {
        Ok(provider_response) => ok(json!({
            "schema": "elastos.inspect.dispatch-result/v1",
            "mode": "provider_authority",
            "id": entry.id,
            "provider": entry.name,
            "target": target,
            "operation": operation,
            "capabilities": plan.resources.iter().map(|resource| json!({
                "resource": resource,
                "actions": plan.actions.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "audit_events": plan.audit_events,
            "execution": approved_execution_policy(),
            "provider_response": provider_response,
        })),
        Err(err) => error("dispatch_failed", &err.to_string()),
    }
}
