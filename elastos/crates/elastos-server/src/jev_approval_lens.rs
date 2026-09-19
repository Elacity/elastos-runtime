//! First Jev Approval Lens slice: shadow mode on Assistant hosted HTTP.
//!
//! Runtime owns the sanitized consequence record and factual relationships.
//! Deterministic policy filters auto-approval before Jev speaks. Jev returns a
//! typed recommendation. Inbox or operator configuration remains the authority
//! for the human decision. This slice never auto-approves.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::api::HostedModelOfferHint;

pub const JEV_RECORD_SCHEMA: &str = "elastos.jev.approval-lens.shadow/v1";
pub const JEV_MODE_SHADOW: &str = "shadow";
const JEV_ROOT: &str = "jev-approval-lens";
const MAX_REQUEST_ID_BYTES: usize = 128;

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

pub fn record_assistant_hosted_http_shadow(
    data_dir: &Path,
    context: &AssistantHostedHttpContext<'_>,
) -> anyhow::Result<JevShadowRecord> {
    let request_id = bounded_request_id(context.request_id)?;
    let policy = deterministic_policy();
    let recommendation = jev_recommendation(&context.hint, &policy);
    let record = JevShadowRecord {
        schema: JEV_RECORD_SCHEMA.to_string(),
        request_id: request_id.to_string(),
        mode: JEV_MODE_SHADOW.to_string(),
        consequence: SanitizedConsequence {
            kind: "assistant_hosted_http".to_string(),
            source_app: "assistant".to_string(),
            title: "Assistant hosted model HTTP".to_string(),
            resource_class: "external_http".to_string(),
        },
        policy,
        recommendation,
        relationships: vec![
            relationship("principal", context.principal_id),
            relationship("session", context.session_id),
            relationship("capsule", context.capsule_id),
            relationship("grant", context.grant_id),
            relationship("offer", context.offer_id),
            relationship("provider_label", &context.hint.provider_label),
            relationship("requested_selector", &context.hint.requested_selector),
            relationship("privacy_policy_ref", &context.hint.privacy_policy_ref),
            relationship("fallback", &context.hint.fallback),
        ],
        human_decision: None,
        actual_outcome: None,
    };
    write_record(data_dir, &record)?;
    Ok(record)
}

#[allow(dead_code)]
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
        .with_context(|| format!("Jev shadow record missing at {}", path.display()))?;
    let record: JevShadowRecord = serde_json::from_slice(&bytes)?;
    if record.schema != JEV_RECORD_SCHEMA {
        anyhow::bail!("unsupported Jev shadow schema");
    }
    if record.mode != JEV_MODE_SHADOW {
        anyhow::bail!("Jev lens mode must stay shadow in this slice");
    }
    Ok(record)
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

fn jev_recommendation(hint: &HostedModelOfferHint, policy: &PolicyFilter) -> JevRecommendation {
    let _ = hint;
    let _ = policy;
    JevRecommendation {
        recommendation: "review".to_string(),
        risk: "medium".to_string(),
        confidence: 80,
        needs_human_review: true,
    }
}

fn relationship(kind: &str, object_id: &str) -> FactualRelationship {
    FactualRelationship {
        kind: kind.to_string(),
        object_id: object_id.trim().to_string(),
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
        assert_eq!(recorded.recommendation.recommendation, "review");
        assert!(recorded.recommendation.needs_human_review);
        assert!(recorded
            .policy
            .blocked_choices
            .iter()
            .any(|choice| choice == "auto_approve"));
        assert!(recorded.human_decision.is_none());
        record_human_decision(tmp.path(), "req-hosted-1", "approve").unwrap();
        record_actual_outcome(tmp.path(), "req-hosted-1", "completed").unwrap();
        let loaded = load_record(tmp.path(), "req-hosted-1").unwrap();
        assert_eq!(loaded.human_decision.as_deref(), Some("approve"));
        assert_eq!(loaded.actual_outcome.as_deref(), Some("completed"));
        assert_eq!(loaded.request_id, "req-hosted-1");
        assert!(loaded
            .relationships
            .iter()
            .any(|rel| rel.kind == "offer" && rel.object_id == "offer-hosted"));
        let encoded = serde_json::to_string(&loaded).unwrap();
        assert!(!encoded.contains("https://"));
        assert!(!encoded.contains("api_key"));
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
        let err = record_human_decision(tmp.path(), "req-hosted-2", "auto_approve")
            .unwrap_err()
            .to_string();
        assert!(err.contains("blocked"));
        assert!(load_record(tmp.path(), "req-hosted-2")
            .unwrap()
            .human_decision
            .is_none());
    }
}
