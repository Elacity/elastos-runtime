use super::*;

#[path = "gateway_inspect_actions/binding.rs"]
mod binding;
#[path = "gateway_inspect_actions/store.rs"]
mod store;

use binding::{
    inspect_action_request_binding, inspect_action_request_id, inspect_action_request_nonce,
};
use store::{
    read_inspect_action_record, write_inspect_action_record, InspectActionRequestRecord,
    INSPECT_ACTION_SCHEMA,
};

pub(super) async fn gateway_inspect_action_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request: &serde_json::Value,
) -> Response {
    match create_inspect_action_request(state, context, request).await {
        Ok(record) => Json(serde_json::json!({
            "schema": INSPECT_ACTION_SCHEMA,
            "status": "pending",
            "request_id": record.request_id,
            "id": record.id,
            "operation": record.operation,
            "plan": record.plan,
            "request_binding": record.request_binding,
        }))
        .into_response(),
        Err(err) => gateway_provider_error_response("inspect", err),
    }
}

pub(super) fn append_inspect_action_notifications(
    notifications: &mut HomeNotificationsSummary,
    requests: Vec<serde_json::Value>,
) {
    for request in requests {
        let request_id = request
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let target = request
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("inspect target");
        let operation = request
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("provider operation");
        let created_at = request
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_else(now_ts);
        let request_hash = request
            .pointer("/request_binding/sha256")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let request_hash_short = request_hash
            .get(..12)
            .filter(|value| !value.is_empty())
            .unwrap_or("unbound");
        let gate_summary = inspect_action_gate_summary(request.get("plan"));
        let body = match gate_summary {
            Some(summary) => format!(
                "System requests approval to run {operation} on {target} with request {request_hash_short}. {summary}"
            ),
            None => format!(
                "System requests approval to run {operation} on {target} with request {request_hash_short}."
            ),
        };
        notifications.unread_count += 1;
        notifications.attention_count += 1;
        notifications.entries.push(HomeNotificationEntrySummary {
            id: format!("inspect-action-request:{request_id}"),
            source_app: SYSTEM_CAPSULE_ID.to_string(),
            kind: "inspect_action_request".to_string(),
            title: "Runtime action request".to_string(),
            body,
            action_ref: Some(HomeNotificationActionSummary {
                app: INBOX_CAPSULE_ID.to_string(),
                action_id: format!("inspect-approve-request:{request_id}"),
            }),
            severity: "attention".to_string(),
            read: false,
            created_at,
        });
    }
}

fn inspect_action_gate_summary(plan: Option<&serde_json::Value>) -> Option<String> {
    let plan = plan?;
    let mut parts = Vec::new();
    let capabilities = plan
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|capability| {
                    let resource = capability
                        .get("resource")
                        .and_then(serde_json::Value::as_str)?
                        .trim();
                    if resource.is_empty() {
                        return None;
                    }
                    let actions = capability
                        .get("actions")
                        .and_then(serde_json::Value::as_array)
                        .map(|actions| {
                            actions
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .take(3)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if actions.is_empty() {
                        Some(format!("Capability {resource}"))
                    } else {
                        Some(format!("Capability {resource}: {}", actions.join(", ")))
                    }
                })
                .take(2)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    parts.extend(capabilities);
    let audits = plan
        .get("audit_events")
        .and_then(serde_json::Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .take(3)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !audits.is_empty() {
        parts.push(format!("Audit {}", audits.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("Gate preview: {}.", parts.join(". ")))
}

pub(super) fn pending_inspect_action_requests(
    data_dir: &FsPath,
    principal_id: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    store::pending_inspect_action_requests(data_dir, principal_id)
}

pub(super) async fn approve_inspect_action_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> anyhow::Result<String> {
    let mut record = read_inspect_action_record(&state.data_dir, request_id)?;
    if record.status != "pending" {
        anyhow::bail!("Inspector action request is not pending");
    }
    if record.principal_id != context.principal_id {
        anyhow::bail!("Inspector action request belongs to a different principal");
    }
    let current_request_binding = inspect_action_request_binding(&record.request);
    if let Some(stored_binding) = &record.request_binding {
        if stored_binding != &current_request_binding {
            record.status = "stale".to_string();
            record.error =
                Some("Inspector action request body changed before Inbox approval".to_string());
            record.result = Some(serde_json::json!({
                "status": "error",
                "code": "request_binding_changed",
                "expected": stored_binding,
                "actual": current_request_binding,
            }));
            record.updated_at = now_ts();
            write_inspect_action_record(&state.data_dir, &record)?;
            append_provider_effect_audit(
                &state.data_dir,
                ProviderEffectAuditInput {
                    capsule_id: INBOX_CAPSULE_ID,
                    event_type: "inspect.action.stale",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id,
                    result: "stale",
                    reason: "Inspector action request body changed before Inbox approval",
                },
            )?;
            anyhow::bail!("Inspector action request body changed before Inbox approval");
        }
    } else {
        record.request_binding = Some(current_request_binding);
    }
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("inspect provider unavailable"))?;
    let current_plan = registry
        .send_raw(
            "inspect",
            &serde_json::json!({
                "op": "plan",
                "id": record.id,
                "operation": record.operation,
            }),
        )
        .await?;
    if current_plan
        .get("status")
        .and_then(serde_json::Value::as_str)
        != Some("ok")
        || current_plan.get("data") != Some(&record.plan)
    {
        let message =
            "Inspector action request is no longer pending because its authority plan changed";
        record.status = "stale".to_string();
        record.error = Some(message.to_string());
        record.result = Some(current_plan);
        record.updated_at = now_ts();
        write_inspect_action_record(&state.data_dir, &record)?;
        append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: INBOX_CAPSULE_ID,
                event_type: "inspect.action.stale",
                principal_id: &context.principal_id,
                session_id: &context.session_id,
                request_id,
                result: "stale",
                reason: "Inspector action authority changed before Inbox approval",
            },
        )?;
        anyhow::bail!(message);
    }
    let response = registry
        .send_raw(
            "inspect",
            &serde_json::json!({
                "op": "dispatch_approved",
                "id": record.id,
                "operation": record.operation,
                "request": record.request,
            }),
        )
        .await?;
    if response.get("status").and_then(serde_json::Value::as_str) == Some("ok") {
        record.status = "completed".to_string();
        record.result = Some(response.clone());
        record.updated_at = now_ts();
        write_inspect_action_record(&state.data_dir, &record)?;
        append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: INBOX_CAPSULE_ID,
                event_type: "inspect.action.completed",
                principal_id: &context.principal_id,
                session_id: &context.session_id,
                request_id,
                result: "completed",
                reason: "Approved Inspector action through Inbox",
            },
        )?;
        return Ok("Approved and dispatched Inspector action.".to_string());
    }
    let message = response
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Inspector action dispatch failed")
        .to_string();
    record.status = "failed".to_string();
    record.error = Some(message.clone());
    record.result = Some(response);
    record.updated_at = now_ts();
    write_inspect_action_record(&state.data_dir, &record)?;
    append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: INBOX_CAPSULE_ID,
            event_type: "inspect.action.failed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "failed",
            reason: "Approved Inspector action dispatch failed through Inbox",
        },
    )?;
    anyhow::bail!(message);
}

pub(super) fn deny_inspect_action_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> anyhow::Result<String> {
    let mut record = read_inspect_action_record(&state.data_dir, request_id)?;
    if record.status != "pending" {
        anyhow::bail!("Inspector action request is not pending");
    }
    if record.principal_id != context.principal_id {
        anyhow::bail!("Inspector action request belongs to a different principal");
    }
    record.status = "denied".to_string();
    record.updated_at = now_ts();
    write_inspect_action_record(&state.data_dir, &record)?;
    append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: INBOX_CAPSULE_ID,
            event_type: "inspect.action.denied",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "denied",
            reason: "Denied Inspector action through Inbox",
        },
    )?;
    Ok("Denied Inspector action.".to_string())
}

async fn create_inspect_action_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request: &serde_json::Value,
) -> anyhow::Result<InspectActionRequestRecord> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("inspect provider unavailable"))?;
    let id = request
        .get("id")
        .or_else(|| request.get("capsule_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("inspect action request requires id"))?;
    let operation = request
        .get("operation")
        .or_else(|| request.get("provider_operation"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("inspect action request requires operation"))?;
    let provider_request = request
        .get("request")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !provider_request.is_object() {
        anyhow::bail!("inspect action request body must be a JSON object");
    }
    if provider_request.get("_runtime_invocation").is_some()
        || provider_request.get("_runtime_transfer").is_some()
    {
        anyhow::bail!("inspect action request must not predeclare runtime metadata");
    }
    if let Some(field) = provider_proxy_runtime_metadata_field(&provider_request) {
        anyhow::bail!("inspect action request must not predeclare Runtime metadata field {field}");
    }
    let plan = registry
        .send_raw(
            "inspect",
            &serde_json::json!({
                "op": "plan",
                "id": id,
                "operation": operation,
            }),
        )
        .await?;
    if plan.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        anyhow::bail!(
            "{}",
            plan.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("inspect action planning failed")
        );
    }
    let now = now_ts();
    let nonce = inspect_action_request_nonce();
    let request_id = inspect_action_request_id(id, operation, now, &nonce);
    let request_binding = inspect_action_request_binding(&provider_request);
    let record = InspectActionRequestRecord {
        schema: INSPECT_ACTION_SCHEMA.to_string(),
        request_id,
        principal_id: context.principal_id.clone(),
        session_id: context.session_id.clone(),
        id: id.to_string(),
        operation: operation.to_string(),
        request: provider_request,
        plan: plan["data"].clone(),
        request_binding: Some(request_binding),
        status: "pending".to_string(),
        created_at: now,
        updated_at: now,
        result: None,
        error: None,
    };
    write_inspect_action_record(&state.data_dir, &record)?;
    append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: SYSTEM_CAPSULE_ID,
            event_type: "inspect.action.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &record.request_id,
            result: "pending",
            reason: "System requested Inspector action approval",
        },
    )?;
    Ok(record)
}
