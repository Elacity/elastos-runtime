//! Jev Approval Lens: sanitized shadow records plus one Home Inbox path.
//!
//! The durable envelope keeps a sanitized consequence, factual relationships,
//! blocked `auto_approve`, a stable request id, a human decision, an actual
//! outcome, and a private 0600 record. Skeleton writes still persist
//! `recommendation=unavailable` when no named Jev instance exists.
//!
//! When an owner has configured a named Jev instance, first Assistant hosted
//! HTTP to another hosted instance creates an Inbox card, calls Jev through
//! that instance, and keeps the person as the authority. Advice cannot grant
//! permission. Auto-approval stays disabled.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use elastos_model_contract::{
    model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding, RUNTIME_ACCESS_BINDING_SCHEMA,
    RUNTIME_CREATE_BINDING_SCHEMA,
};
use elastos_runtime::provider::ProviderRegistry;
use serde::{Deserialize, Serialize};

use crate::api::HostedModelOfferHint;

pub const JEV_RECORD_SCHEMA: &str = "elastos.jev.approval-lens.shadow/v1";
pub const JEV_MODE_SHADOW: &str = "shadow";
pub const HOSTED_HTTP_APPROVE_PREFIX: &str = "hosted-http-approve:";
pub const HOSTED_HTTP_DENY_PREFIX: &str = "hosted-http-deny:";
pub const HOSTED_HTTP_REVIEW_MESSAGE: &str = "This hosted connection needs your review in Inbox.";
pub const HOSTED_HTTP_DENIED_MESSAGE: &str =
    "This hosted connection was denied. Choose another connection or review Inbox.";
const JEV_ROOT: &str = "jev-approval-lens";
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 280;
const EVALUATOR_TIMEOUT: Duration = Duration::from_secs(8);
const EVALUATOR_CAPSULE_ID: &str = "assistant";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedHttpGate {
    Proceed,
    NeedsReview,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SanitizedConsequence {
    pub kind: String,
    pub source_app: String,
    pub title: String,
    pub resource_class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolicyFilter {
    pub allowed_choices: Vec<String>,
    pub blocked_choices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JevRecommendation {
    pub recommendation: String,
    #[serde(default)]
    pub reason: String,
    pub risk: String,
    pub confidence: u8,
    pub needs_human_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FactualRelationship {
    pub kind: String,
    pub object_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JevShadowRecord {
    pub schema: String,
    pub request_id: String,
    pub mode: String,
    pub consequence: SanitizedConsequence,
    pub policy: PolicyFilter,
    pub recommendation: JevRecommendation,
    pub relationships: Vec<FactualRelationship>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_outcome: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AssistantHostedHttpContext<'a> {
    pub request_id: &'a str,
    pub principal_id: &'a str,
    pub session_id: &'a str,
    pub capsule_id: &'a str,
    pub grant_id: &'a str,
    pub offer_id: &'a str,
    pub hint: HostedModelOfferHint,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypedJevReply {
    recommendation: String,
    reason: String,
    risk: String,
    confidence: u8,
}

pub fn record_assistant_hosted_http_shadow(
    data_dir: &Path,
    context: &AssistantHostedHttpContext<'_>,
) -> anyhow::Result<JevShadowRecord> {
    let request_id = bounded_request_id(context.request_id)?;
    let policy = deterministic_policy();
    let record = JevShadowRecord {
        schema: JEV_RECORD_SCHEMA.to_string(),
        request_id: request_id.to_string(),
        mode: JEV_MODE_SHADOW.to_string(),
        consequence: sanitized_consequence(context),
        policy,
        recommendation: unavailable_recommendation("Not reported"),
        relationships: relationships(context),
        human_decision: None,
        actual_outcome: None,
    };
    write_record(data_dir, &record)?;
    Ok(record)
}

pub fn record_human_decision(
    data_dir: &Path,
    request_id: &str,
    decision: &str,
) -> anyhow::Result<JevShadowRecord> {
    let mut record = load_record(data_dir, request_id)?;
    let decision = bounded_choice(decision, "human_decision")?;
    if record
        .policy
        .blocked_choices
        .iter()
        .any(|choice| choice == decision)
    {
        anyhow::bail!("Jev shadow policy blocked human choice {decision}");
    }
    if !record
        .policy
        .allowed_choices
        .iter()
        .any(|choice| choice == decision)
    {
        anyhow::bail!("Jev shadow policy does not allow human choice {decision}");
    }
    record.human_decision = Some(decision.to_string());
    write_record(data_dir, &record)?;
    Ok(record)
}

pub fn record_actual_outcome(
    data_dir: &Path,
    request_id: &str,
    outcome: &str,
) -> anyhow::Result<JevShadowRecord> {
    let mut record = load_record(data_dir, request_id)?;
    record.actual_outcome = Some(bounded_choice(outcome, "actual_outcome")?.to_string());
    write_record(data_dir, &record)?;
    Ok(record)
}

pub fn load_record(data_dir: &Path, request_id: &str) -> anyhow::Result<JevShadowRecord> {
    let path = record_path(data_dir, request_id)?;
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read Jev shadow record {}", path.display()))?;
    serde_json::from_slice(&bytes).context("Jev shadow record is not valid JSON")
}

pub fn hosted_http_request_id(offer_id: &str) -> anyhow::Result<String> {
    let offer_id = bounded_request_id(offer_id)?;
    let request_id = format!("hosted-http:{}", offer_id.replace(':', "_"));
    bounded_request_id(&request_id)?;
    Ok(request_id)
}

pub fn outcome_request_id(data_dir: &Path, offer_id: &str, fallback: &str) -> String {
    if let Ok(request_id) = hosted_http_request_id(offer_id) {
        if load_record(data_dir, &request_id).is_ok() {
            return request_id;
        }
    }
    fallback.to_string()
}

pub fn inbox_card_copy(record: &JevShadowRecord) -> (String, String) {
    let title = format!(
        "Assistant requests {}",
        if record.consequence.title.trim().is_empty() {
            "hosted HTTP"
        } else {
            record.consequence.title.trim()
        }
    );
    let processor = relationship_object(record, "provider_label").unwrap_or("Not reported");
    let recommendation = record.recommendation.recommendation.trim();
    let reason = if record.recommendation.reason.trim().is_empty() {
        "Not reported"
    } else {
        record.recommendation.reason.trim()
    };
    let body = format!(
        "Action: send a prompt through a hosted model connection\n\
Affected resource: {}\n\
Processor: {}\n\
Relationships: Assistant uses this hosted connection. The owner remains the authority.\n\
Jev recommendation: {}\n\
Reason: {}\n\
Risk: {}\n\
Confidence: {}\n\
You remain the authority.",
        record.consequence.title,
        processor,
        recommendation,
        reason,
        record.recommendation.risk,
        record.recommendation.confidence
    );
    (title, body)
}

pub async fn prepare_assistant_hosted_http(
    data_dir: &Path,
    registry: &ProviderRegistry,
    context: &AssistantHostedHttpContext<'_>,
) -> HostedHttpGate {
    let Some((jev_offer_id, _)) = crate::api::named_jev_hosted_offer(data_dir) else {
        let _ = record_assistant_hosted_http_shadow(data_dir, context);
        return HostedHttpGate::Proceed;
    };
    if context.offer_id == jev_offer_id {
        let _ = record_assistant_hosted_http_shadow(data_dir, context);
        return HostedHttpGate::Proceed;
    }
    let Ok(request_id) = hosted_http_request_id(context.offer_id) else {
        let _ = record_assistant_hosted_http_shadow(data_dir, context);
        return HostedHttpGate::Proceed;
    };
    if let Ok(existing) = load_record(data_dir, &request_id) {
        match existing.human_decision.as_deref() {
            Some("approve") => return HostedHttpGate::Proceed,
            Some("deny") => return HostedHttpGate::Denied,
            _ => {
                let _ = upsert_inbox_card(data_dir, &existing);
                return HostedHttpGate::NeedsReview;
            }
        }
    }
    let eval_context = AssistantHostedHttpContext {
        request_id: &request_id,
        principal_id: context.principal_id,
        session_id: context.session_id,
        capsule_id: context.capsule_id,
        grant_id: context.grant_id,
        offer_id: context.offer_id,
        hint: context.hint.clone(),
    };
    let recommendation = evaluate_named_jev(data_dir, registry, &jev_offer_id, &eval_context).await;
    let record = JevShadowRecord {
        schema: JEV_RECORD_SCHEMA.to_string(),
        request_id: request_id.clone(),
        mode: JEV_MODE_SHADOW.to_string(),
        consequence: sanitized_consequence(&eval_context),
        policy: deterministic_policy(),
        recommendation,
        relationships: relationships(&eval_context),
        human_decision: None,
        actual_outcome: None,
    };
    if write_record(data_dir, &record).is_err() {
        let _ = upsert_inbox_card(data_dir, &record);
        return HostedHttpGate::NeedsReview;
    }
    let _ = upsert_inbox_card(data_dir, &record);
    HostedHttpGate::NeedsReview
}

pub fn parse_typed_jev_reply(text: &str) -> Result<JevRecommendation, String> {
    let json_text = extract_json_object(text).ok_or_else(|| "Jev reply is not JSON".to_string())?;
    let reply: TypedJevReply = serde_json::from_str(json_text)
        .map_err(|_| "Jev reply is not a typed recommendation".to_string())?;
    let recommendation = reply.recommendation.trim();
    if !matches!(recommendation, "approve" | "deny" | "defer") {
        return Err("Jev recommendation is not an allowed choice".to_string());
    }
    let risk = reply.risk.trim();
    if !matches!(risk, "low" | "medium" | "high" | "unknown") {
        return Err("Jev risk is not an allowed choice".to_string());
    }
    if reply.confidence > 100 {
        return Err("Jev confidence is out of range".to_string());
    }
    Ok(JevRecommendation {
        recommendation: recommendation.to_string(),
        reason: sanitize_reason(&reply.reason),
        risk: risk.to_string(),
        confidence: reply.confidence,
        needs_human_review: true,
    })
}

async fn evaluate_named_jev(
    data_dir: &Path,
    registry: &ProviderRegistry,
    jev_offer_id: &str,
    context: &AssistantHostedHttpContext<'_>,
) -> JevRecommendation {
    if context.offer_id == jev_offer_id {
        return unavailable_recommendation("Jev does not evaluate its own request");
    }
    let prompt = evaluator_prompt(context);
    if secret_in_text(data_dir, &prompt).is_some() {
        return unavailable_recommendation("Jev request omitted a secret");
    }
    let input = serde_json::json!({
        "schema": "elastos.model.input.text/v1",
        "prompt": prompt
    });
    let eval_request_id = format!("jev-eval:{}:{}", context.request_id, unique_eval_token());
    let Ok(input_hash) = model_input_hash(&input) else {
        return unavailable_recommendation("Jev request could not be bound");
    };
    let binding = RuntimeCreateBinding {
        schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
        principal_id: context.principal_id.to_string(),
        session_id: context.session_id.to_string(),
        capsule_id: EVALUATOR_CAPSULE_ID.to_string(),
        grant_id: context.grant_id.to_string(),
        request_id: eval_request_id,
        offer_id: jev_offer_id.to_string(),
        operation: "text.generate".to_string(),
        input_hash,
    };
    if binding
        .validate(jev_offer_id, "text.generate", &input)
        .is_err()
    {
        return unavailable_recommendation("Jev request could not be bound");
    }
    let request = serde_json::json!({
        "op": "runs_create",
        "offer_id": jev_offer_id,
        "operation": "text.generate",
        "input": input,
        "runtime_binding": binding,
    });
    let result = tokio::time::timeout(EVALUATOR_TIMEOUT, async {
        let created = registry.send_raw("model", &request).await;
        match created {
            Ok(response) => Ok(wait_for_jev_output(registry, context, response).await),
            Err(err) => Err(err),
        }
    })
    .await;
    match result {
        Err(_) => unavailable_recommendation("Jev did not reply in time"),
        Ok(Err(_)) => unavailable_recommendation("Jev instance is unavailable"),
        Ok(Ok(response)) => recommendation_from_provider_response(data_dir, &response),
    }
}

fn jev_output_ready(response: &serde_json::Value) -> bool {
    if provider_output_text(response).is_some_and(|text| !text.is_empty()) {
        return true;
    }
    matches!(
        response
            .pointer("/data/status")
            .or_else(|| response.pointer("/data/terminal/status"))
            .and_then(serde_json::Value::as_str),
        Some("completed" | "failed" | "cancelled")
    )
}

async fn wait_for_jev_output(
    registry: &ProviderRegistry,
    context: &AssistantHostedHttpContext<'_>,
    created: serde_json::Value,
) -> serde_json::Value {
    if created.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return created;
    }
    if jev_output_ready(&created) {
        return created;
    }
    let Some(run_id) = created
        .pointer("/data/run_id")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return created;
    };
    let mut poll = 0u32;
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        poll = poll.saturating_add(1);
        let binding = RuntimeAccessBinding {
            schema: RUNTIME_ACCESS_BINDING_SCHEMA.to_string(),
            principal_id: context.principal_id.to_string(),
            session_id: context.session_id.to_string(),
            capsule_id: EVALUATOR_CAPSULE_ID.to_string(),
            grant_id: context.grant_id.to_string(),
            request_id: format!("jev-eval-get-{poll}"),
            run_id: run_id.clone(),
        };
        if binding.validate(&run_id).is_err() {
            return created;
        }
        let request = serde_json::json!({
            "op": "runs_get",
            "run_id": run_id,
            "runtime_binding": binding,
        });
        match registry.send_raw("model", &request).await {
            Ok(response)
                if jev_output_ready(&response)
                    || response.get("status").and_then(serde_json::Value::as_str) != Some("ok") =>
            {
                return response;
            }
            Ok(_) => {}
            Err(_) => return created,
        }
    }
}

fn recommendation_from_provider_response(
    data_dir: &Path,
    response: &serde_json::Value,
) -> JevRecommendation {
    if response.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return unavailable_recommendation("Jev instance is unavailable");
    }
    if matches!(
        response
            .pointer("/data/status")
            .and_then(serde_json::Value::as_str),
        Some("failed" | "cancelled")
    ) {
        return unavailable_recommendation("Jev instance is unavailable");
    }
    let text = provider_output_text(response).unwrap_or("");
    if secret_in_text(data_dir, text).is_some() {
        return unavailable_recommendation("Jev reply contained a secret");
    }
    match parse_typed_jev_reply(text) {
        Ok(recommendation) => recommendation,
        Err(_) => unavailable_recommendation("Jev reply was not a typed recommendation"),
    }
}

fn provider_output_text(response: &serde_json::Value) -> Option<&str> {
    response
        .pointer("/data/terminal/output/text")
        .or_else(|| response.pointer("/data/output/text"))
        .or_else(|| response.pointer("/data/text"))
        .and_then(serde_json::Value::as_str)
}

fn evaluator_prompt(context: &AssistantHostedHttpContext<'_>) -> String {
    format!(
        "Return JSON only with keys recommendation, reason, risk, confidence.\n\
recommendation is approve, deny, or defer.\n\
risk is low, medium, high, or unknown.\n\
confidence is an integer from 0 to 100.\n\
Action: send a prompt through a hosted model connection\n\
Affected resource: {}\n\
Processor: {}\n\
The person remains the authority.",
        context.hint.offer_title, context.hint.provider_label
    )
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&text[start..=end])
}

fn sanitize_reason(value: &str) -> String {
    let trimmed = value
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .collect::<String>();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return "Not reported".to_string();
    }
    if trimmed.len() > MAX_REASON_BYTES {
        return trimmed.chars().take(MAX_REASON_BYTES).collect();
    }
    trimmed.to_string()
}

fn secret_in_text(data_dir: &Path, text: &str) -> Option<()> {
    if text.contains("sk-or") || text.contains("sk-vnz") || text.contains("api_key") {
        return Some(());
    }
    let secrets_dir = data_dir
        .join("providers")
        .join("model-provider")
        .join("secrets");
    let Ok(entries) = fs::read_dir(secrets_dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(secret) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let secret = secret.trim();
        if !secret.is_empty() && text.contains(secret) {
            return Some(());
        }
    }
    None
}

fn upsert_inbox_card(data_dir: &Path, record: &JevShadowRecord) -> anyhow::Result<()> {
    let (title, body) = inbox_card_copy(record);
    if secret_in_text(data_dir, &title).is_some() || secret_in_text(data_dir, &body).is_some() {
        anyhow::bail!("Jev Inbox copy omitted a secret");
    }
    crate::notifications::upsert_external_http_request(
        data_dir,
        &record.request_id,
        "assistant",
        &title,
        &body,
        &format!("{HOSTED_HTTP_APPROVE_PREFIX}{}", record.request_id),
        now_ts(),
    )
}

fn sanitized_consequence(context: &AssistantHostedHttpContext<'_>) -> SanitizedConsequence {
    let title = if context.hint.offer_title.trim().is_empty() {
        "hosted HTTP".to_string()
    } else {
        context.hint.offer_title.trim().to_string()
    };
    SanitizedConsequence {
        kind: "assistant_hosted_http".to_string(),
        source_app: "assistant".to_string(),
        title,
        resource_class: "external_http".to_string(),
    }
}

fn relationships(context: &AssistantHostedHttpContext<'_>) -> Vec<FactualRelationship> {
    vec![
        relationship("principal", context.principal_id),
        relationship("session", context.session_id),
        relationship("capsule", context.capsule_id),
        relationship("grant", context.grant_id),
        relationship("offer", context.offer_id),
        relationship("offer_title", &context.hint.offer_title),
        relationship("provider_label", &context.hint.provider_label),
        relationship("requested_selector", &context.hint.requested_selector),
        relationship("privacy_policy_ref", &context.hint.privacy_policy_ref),
        relationship("fallback", &context.hint.fallback),
    ]
}

fn relationship_object<'a>(record: &'a JevShadowRecord, kind: &str) -> Option<&'a str> {
    record
        .relationships
        .iter()
        .find(|item| item.kind == kind)
        .map(|item| item.object_id.as_str())
}

fn unavailable_recommendation(reason: &str) -> JevRecommendation {
    JevRecommendation {
        recommendation: "unavailable".to_string(),
        reason: sanitize_reason(reason),
        risk: "unknown".to_string(),
        confidence: 0,
        needs_human_review: true,
    }
}

fn deterministic_policy() -> PolicyFilter {
    PolicyFilter {
        allowed_choices: vec![
            "approve".to_string(),
            "deny".to_string(),
            "defer".to_string(),
        ],
        blocked_choices: vec!["auto_approve".to_string()],
    }
}

fn relationship(kind: &str, object_id: &str) -> FactualRelationship {
    FactualRelationship {
        kind: kind.to_string(),
        object_id: object_id.to_string(),
    }
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn unique_eval_token() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!(
        "{}-{}",
        millis,
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn bounded_request_id(request_id: &str) -> anyhow::Result<&str> {
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
        anyhow::bail!("Jev request id is missing or too large");
    }
    if request_id
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')))
    {
        anyhow::bail!("Jev request id contains an unsupported character");
    }
    Ok(request_id)
}

fn bounded_choice<'a>(value: &'a str, label: &str) -> anyhow::Result<&'a str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        anyhow::bail!("{label} is missing or too large");
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        anyhow::bail!("{label} contains an unsupported character");
    }
    Ok(value)
}

fn record_path(data_dir: &Path, request_id: &str) -> anyhow::Result<PathBuf> {
    let request_id = bounded_request_id(request_id)?;
    let file_name = request_id.replace(':', "_");
    Ok(data_dir.join(JEV_ROOT).join(format!("{file_name}.json")))
}

fn write_record(data_dir: &Path, record: &JevShadowRecord) -> anyhow::Result<()> {
    let path = record_path(data_dir, &record.request_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Jev shadow path missing parent"))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(parent)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(parent, permissions)?;
    }
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jev")
    ));
    let encoded = serde_json::to_vec_pretty(record)?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&tmp)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&tmp, permissions)?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint() -> HostedModelOfferHint {
        HostedModelOfferHint {
            offer_title: "Venice".to_string(),
            provider_label: "Fixture Provider".to_string(),
            requested_selector: "gpt-test".to_string(),
            privacy_policy_ref: "fixture:privacy:v1".to_string(),
            fallback: "operator_asserted_disabled".to_string(),
        }
    }

    #[test]
    fn shadow_record_keeps_one_request_id_for_recommendation_decision_and_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let context = AssistantHostedHttpContext {
            request_id: "req-hosted-1",
            principal_id: "person:local:admin",
            session_id: "auth:session",
            capsule_id: "assistant",
            grant_id: "grant:home",
            offer_id: "offer-hosted",
            hint: hint(),
        };
        let recorded = record_assistant_hosted_http_shadow(tmp.path(), &context).unwrap();
        assert_eq!(recorded.schema, JEV_RECORD_SCHEMA);
        assert_eq!(recorded.mode, JEV_MODE_SHADOW);
        assert_eq!(recorded.recommendation.recommendation, "unavailable");
        assert_eq!(recorded.recommendation.reason, "Not reported");
        assert_eq!(recorded.recommendation.risk, "unknown");
        assert_eq!(recorded.recommendation.confidence, 0);
        assert!(recorded.recommendation.needs_human_review);
        assert!(recorded
            .policy
            .blocked_choices
            .iter()
            .any(|choice| choice == "auto_approve"));
        assert!(recorded.human_decision.is_none());
        record_human_decision(tmp.path(), "req-hosted-1", "approve").unwrap();
        record_actual_outcome(tmp.path(), "req-hosted-1", "accepted").unwrap();
        let loaded = load_record(tmp.path(), "req-hosted-1").unwrap();
        assert_eq!(loaded.request_id, "req-hosted-1");
        assert_eq!(loaded.human_decision.as_deref(), Some("approve"));
        assert_eq!(loaded.actual_outcome.as_deref(), Some("accepted"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let path = record_path(tmp.path(), "req-hosted-1").unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn shadow_policy_refuses_auto_approve() {
        let tmp = tempfile::tempdir().unwrap();
        let context = AssistantHostedHttpContext {
            request_id: "req-hosted-2",
            principal_id: "person:local:admin",
            session_id: "auth:session",
            capsule_id: "assistant",
            grant_id: "grant:home",
            offer_id: "offer-hosted",
            hint: hint(),
        };
        record_assistant_hosted_http_shadow(tmp.path(), &context).unwrap();
        let err = record_human_decision(tmp.path(), "req-hosted-2", "auto_approve").unwrap_err();
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn typed_reply_accepts_approve_and_keeps_human_review() {
        let parsed = parse_typed_jev_reply(
            "{\"recommendation\":\"approve\",\"reason\":\"Matches the named connection.\",\"risk\":\"low\",\"confidence\":80}",
        )
        .unwrap();
        assert_eq!(parsed.recommendation, "approve");
        assert_eq!(parsed.risk, "low");
        assert_eq!(parsed.confidence, 80);
        assert!(parsed.needs_human_review);
    }

    #[test]
    fn typed_reply_rejects_auto_approve_and_unknown_fields() {
        assert!(parse_typed_jev_reply(
            "{\"recommendation\":\"approve\",\"reason\":\"x\",\"risk\":\"low\",\"confidence\":1,\"auto_approve\":true}"
        )
        .is_err());
        assert!(parse_typed_jev_reply(
            "{\"recommendation\":\"auto_approve\",\"reason\":\"x\",\"risk\":\"low\",\"confidence\":1}"
        )
        .is_err());
        assert!(parse_typed_jev_reply("not-json").is_err());
    }

    #[test]
    fn inbox_copy_omits_urls_and_keeps_owner_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let context = AssistantHostedHttpContext {
            request_id: "hosted-http:model_venice",
            principal_id: "person:local:admin",
            session_id: "auth:session",
            capsule_id: "assistant",
            grant_id: "grant:home",
            offer_id: "model:venice",
            hint: hint(),
        };
        let recorded = record_assistant_hosted_http_shadow(tmp.path(), &context).unwrap();
        let (title, body) = inbox_card_copy(&recorded);
        assert!(title.contains("Venice"));
        assert!(body.contains("Action: send a prompt through a hosted model connection"));
        assert!(body.contains("Affected resource: Venice"));
        assert!(body.contains("Jev recommendation: unavailable"));
        assert!(body.contains("You remain the authority."));
        assert!(!body.contains("https://"));
        assert!(!body.contains("api_key"));
    }
}
