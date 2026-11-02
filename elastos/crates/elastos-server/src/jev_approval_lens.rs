//! Jev Approval Lens records: sanitized shadow records of Assistant hosted
//! HTTP requests.
//!
//! The durable envelope keeps a sanitized consequence, factual relationships,
//! blocked `auto_approve`, a stable request id, a human decision, an actual
//! outcome, and a private 0600 record. Every record is written with
//! `recommendation=unavailable`.
//!
//! Approval Lens is hidden in 0.8.0: no evaluator is ever asked and an
//! evaluator choice saved by an earlier release is ignored. Inbox decisions
//! on cards an earlier release created are still recorded, and an approval
//! recorded then can still be ended from System.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::api::HostedModelOfferHint;
use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const JEV_RECORD_SCHEMA: &str = "elastos.jev.approval-lens.shadow/v1";
pub const JEV_MODE_SHADOW: &str = "shadow";
pub const HOSTED_HTTP_APPROVE_PREFIX: &str = "hosted-http-approve:";
pub const HOSTED_HTTP_DENY_PREFIX: &str = "hosted-http-deny:";
const JEV_ROOT: &str = "jev-approval-lens";
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 280;

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
    pub confidence: Option<u8>,
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

pub fn approved_connection(data_dir: &Path, offer_id: &str) -> bool {
    hosted_http_request_id(offer_id)
        .ok()
        .and_then(|id| load_record(data_dir, &id).ok())
        .is_some_and(|record| {
            record.human_decision.as_deref() == Some("approve")
                && relationship_object(&record, "offer") == Some(offer_id)
        })
}

pub fn end_connection_approval(data_dir: &Path, offer_id: &str) -> anyhow::Result<()> {
    let request_id = hosted_http_request_id(offer_id)?;
    let record = load_record(data_dir, &request_id)?;
    anyhow::ensure!(
        relationship_object(&record, "offer") == Some(offer_id)
            && record.human_decision.as_deref() == Some("approve"),
        "hosted connection has no active approval"
    );
    record_human_decision(data_dir, &request_id, "defer")?;
    Ok(())
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
        confidence: None,
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
        assert_eq!(recorded.recommendation.confidence, None);
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
}
