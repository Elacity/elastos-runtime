use crate::contract::{
    model_input_hash, validate_trimmed, OfferPolicySummary, OfferSummary, MODEL_POLICY_SCHEMA,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use url::Url;

pub const MAX_INIT_EXTRA_BYTES: usize = 256 * 1024;
pub const MAX_BASE_PATH_BYTES: usize = 4 * 1024;
pub const MAX_JOURNAL_DIR_BYTES: usize = 4 * 1024;
pub const MAX_PROVIDER_ID_BYTES: usize = 128;
pub const MAX_OFFER_COUNT: usize = 64;
pub const MAX_OFFER_ID_BYTES: usize = 128;
pub const MAX_OFFER_TITLE_BYTES: usize = 160;
pub const MAX_OPERATION_BYTES: usize = 128;
pub const MAX_MODALITIES_PER_OFFER: usize = 8;
pub const MAX_MODALITY_BYTES: usize = 64;
pub const MAX_URL_BYTES: usize = 2 * 1024;
pub const MAX_SECRET_BYTES: usize = 4 * 1024;
pub const MAX_MODEL_BYTES: usize = 128;
pub const MAX_POLL_INTERVAL_MS: u64 = 300_000;
pub const MAX_CONCURRENCY_LIMIT: u32 = 64;
pub const MAX_INPUT_BYTES_LIMIT: u64 = 16 * 1024 * 1024;
pub const MAX_INLINE_OUTPUT_BYTES_LIMIT: u64 = 128 * 1024;
pub const MAX_EVENT_BYTES_LIMIT: u64 = 192 * 1024;
pub const MAX_RUN_EVENT_COUNT_LIMIT: usize = 256;
pub const MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT: u64 = 512 * 1024;
pub const MAX_RUN_EVENTS_PAGE_COUNT_LIMIT: usize = 64;
pub const MAX_RUN_EVENTS_PAGE_BYTES_LIMIT: u64 = 224 * 1024;
pub const MAX_RUNTIME_MS_LIMIT: u64 = 3_600_000;
pub const MAX_RETENTION_SECS: u64 = 604_800;
pub const MAX_CANCEL_SETTLEMENT_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeProviderConfig {
    #[serde(default)]
    pub base_path: String,
    #[serde(default)]
    pub extra: Value,
}

impl BridgeProviderConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.base_path.is_empty() {
            validate_bounded_trimmed(&self.base_path, "base_path", MAX_BASE_PATH_BYTES)?;
            if !Path::new(&self.base_path).is_absolute() {
                anyhow::bail!("base_path must be an absolute path");
            }
        }
        let extra_bytes = serde_json::to_vec(&self.extra)?;
        if extra_bytes.len() > MAX_INIT_EXTRA_BYTES {
            anyhow::bail!(
                "model provider init extra exceeds {} bytes",
                MAX_INIT_EXTRA_BYTES
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInitExtra {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub journal_dir: Option<String>,
    #[serde(default)]
    pub offers: Vec<ConfiguredOffer>,
}

impl ProviderInitExtra {
    pub fn validate(&self, base_path: &str) -> Result<()> {
        if let Some(provider_id) = self.provider_id.as_deref() {
            validate_bounded_trimmed(provider_id, "provider_id", MAX_PROVIDER_ID_BYTES)?;
        }
        if let Some(journal_dir) = self.journal_dir.as_deref() {
            validate_bounded_trimmed(journal_dir, "journal_dir", MAX_JOURNAL_DIR_BYTES)?;
            if !Path::new(journal_dir).is_absolute() {
                anyhow::bail!("journal_dir must be an absolute path");
            }
        }
        if base_path.is_empty() && self.journal_dir.is_none() {
            anyhow::bail!("model provider init requires base_path or journal_dir");
        }
        if self.offers.len() > MAX_OFFER_COUNT {
            anyhow::bail!("model provider offers exceed {}", MAX_OFFER_COUNT);
        }
        let mut offer_ids = BTreeSet::new();
        for offer in &self.offers {
            offer.validate()?;
            if !offer_ids.insert(offer.id.as_str()) {
                anyhow::bail!("duplicate model offer id in provider config");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterConfig {
    OpenAiCompatibleText {
        api_url: String,
        #[serde(default)]
        api_key: Option<String>,
        model: String,
    },
    HttpJobArtifact {
        create_url: String,
        status_url: String,
        #[serde(default)]
        cancel_url: Option<String>,
        #[serde(default)]
        bearer_token: Option<String>,
        poll_interval_ms: u64,
    },
}

impl AdapterConfig {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::OpenAiCompatibleText {
                api_url,
                api_key,
                model,
            } => {
                validate_url(api_url, "openai adapter api_url")?;
                if let Some(api_key) = api_key.as_deref() {
                    validate_bounded_trimmed(api_key, "openai adapter api_key", MAX_SECRET_BYTES)?;
                }
                validate_bounded_trimmed(model, "openai adapter model", MAX_MODEL_BYTES)?;
            }
            Self::HttpJobArtifact {
                create_url,
                status_url,
                cancel_url,
                bearer_token,
                poll_interval_ms,
            } => {
                validate_url(create_url, "http job adapter create_url")?;
                validate_url(status_url, "http job adapter status_url")?;
                if let Some(cancel_url) = cancel_url.as_deref() {
                    validate_url(cancel_url, "http job adapter cancel_url")?;
                }
                if let Some(bearer_token) = bearer_token.as_deref() {
                    validate_bounded_trimmed(
                        bearer_token,
                        "http job adapter bearer_token",
                        MAX_SECRET_BYTES,
                    )?;
                }
                if *poll_interval_ms == 0 || *poll_interval_ms > MAX_POLL_INTERVAL_MS {
                    anyhow::bail!(
                        "http job adapter poll_interval_ms must be in 1..={MAX_POLL_INTERVAL_MS}"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn stream_output(&self) -> bool {
        matches!(self, Self::OpenAiCompatibleText { .. })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfferPolicy {
    pub concurrency_limit: u32,
    pub input_bytes_limit: u64,
    pub inline_output_bytes_limit: u64,
    pub event_bytes_limit: u64,
    pub runtime_ms_limit: u64,
    pub retention_secs: u64,
    pub cancel_settlement_timeout_ms: u64,
}

impl OfferPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.concurrency_limit == 0 || self.concurrency_limit > MAX_CONCURRENCY_LIMIT {
            anyhow::bail!("policy concurrency_limit must be in 1..={MAX_CONCURRENCY_LIMIT}");
        }
        for (value, max, label) in [
            (
                self.input_bytes_limit,
                MAX_INPUT_BYTES_LIMIT,
                "input_bytes_limit",
            ),
            (
                self.inline_output_bytes_limit,
                MAX_INLINE_OUTPUT_BYTES_LIMIT,
                "inline_output_bytes_limit",
            ),
            (
                self.event_bytes_limit,
                MAX_EVENT_BYTES_LIMIT,
                "event_bytes_limit",
            ),
            (
                self.runtime_ms_limit,
                MAX_RUNTIME_MS_LIMIT,
                "runtime_ms_limit",
            ),
            (self.retention_secs, MAX_RETENTION_SECS, "retention_secs"),
            (
                self.cancel_settlement_timeout_ms,
                MAX_CANCEL_SETTLEMENT_TIMEOUT_MS,
                "cancel_settlement_timeout_ms",
            ),
        ] {
            if value == 0 || value > max {
                anyhow::bail!("{label} must be in 1..={max}");
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> OfferPolicySummary {
        OfferPolicySummary {
            schema: MODEL_POLICY_SCHEMA.to_string(),
            concurrency_limit: self.concurrency_limit,
            input_bytes_limit: self.input_bytes_limit,
            inline_output_bytes_limit: self.inline_output_bytes_limit,
            event_bytes_limit: self.event_bytes_limit,
            runtime_ms_limit: self.runtime_ms_limit,
            retention_secs: self.retention_secs,
            cancel_settlement_timeout_ms: self.cancel_settlement_timeout_ms,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredOffer {
    pub id: String,
    pub title: String,
    pub operation: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub policy: OfferPolicy,
    pub adapter: AdapterConfig,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl ConfiguredOffer {
    pub fn validate(&self) -> Result<()> {
        validate_bounded_trimmed(&self.id, "offer id", MAX_OFFER_ID_BYTES)?;
        validate_bounded_trimmed(&self.title, "offer title", MAX_OFFER_TITLE_BYTES)?;
        validate_bounded_trimmed(&self.operation, "offer operation", MAX_OPERATION_BYTES)?;
        for modality in &self.input_modalities {
            validate_bounded_trimmed(modality, "offer input modality", MAX_MODALITY_BYTES)?;
        }
        for modality in &self.output_modalities {
            validate_bounded_trimmed(modality, "offer output modality", MAX_MODALITY_BYTES)?;
        }
        self.validate_canonical_modalities()?;
        self.policy.validate()?;
        self.adapter.validate()?;
        Ok(())
    }

    pub fn summary(&self) -> OfferSummary {
        OfferSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            operation: self.operation.clone(),
            input_modalities: self.input_modalities.clone(),
            output_modalities: self.output_modalities.clone(),
            stream_output: self.adapter.stream_output(),
            policy: self.policy.summary(),
        }
    }

    pub fn execution_binding_hash(&self) -> Result<String> {
        let adapter = match &self.adapter {
            AdapterConfig::OpenAiCompatibleText { api_url, model, .. } => json!({
                "kind": "open_ai_compatible_text",
                "api_url": api_url,
                "model": model,
            }),
            AdapterConfig::HttpJobArtifact {
                create_url,
                status_url,
                cancel_url,
                poll_interval_ms,
                ..
            } => json!({
                "kind": "http_job_artifact",
                "create_url": create_url,
                "status_url": status_url,
                "cancel_url": cancel_url,
                "poll_interval_ms": poll_interval_ms,
            }),
        };
        model_input_hash(&json!({
            "adapter": adapter,
            "offer": self.summary(),
        }))
    }

    fn validate_canonical_modalities(&self) -> Result<()> {
        match &self.adapter {
            AdapterConfig::OpenAiCompatibleText { .. } => {
                if self.operation != "text.generate" {
                    anyhow::bail!("openai compatible text offers require operation text.generate");
                }
                validate_exact_modalities(
                    &self.input_modalities,
                    &["text/plain"],
                    "openai compatible text input_modalities",
                )?;
                validate_exact_modalities(
                    &self.output_modalities,
                    &["text/plain"],
                    "openai compatible text output_modalities",
                )?;
            }
            AdapterConfig::HttpJobArtifact { .. } => {
                if !matches!(self.operation.as_str(), "image.generate" | "video.generate") {
                    anyhow::bail!(
                        "http job artifact offers require operation image.generate or video.generate"
                    );
                }
                validate_exact_modalities(
                    &self.input_modalities,
                    &["application/json"],
                    "http job artifact input_modalities",
                )?;
                validate_exact_modalities(
                    &self.output_modalities,
                    &["application/json"],
                    "http job artifact output_modalities",
                )?;
            }
        }
        Ok(())
    }
}

pub fn journal_root(base_path: &str, configured: Option<&str>) -> Result<PathBuf> {
    if let Some(configured) = configured {
        validate_bounded_trimmed(configured, "journal_dir", MAX_JOURNAL_DIR_BYTES)?;
        if !Path::new(configured).is_absolute() {
            anyhow::bail!("journal_dir must be an absolute path");
        }
        return Ok(PathBuf::from(configured));
    }
    if base_path.is_empty() {
        anyhow::bail!("model provider init requires base_path or journal_dir");
    }
    validate_bounded_trimmed(base_path, "base_path", MAX_BASE_PATH_BYTES)?;
    if !Path::new(base_path).is_absolute() {
        anyhow::bail!("base_path must be an absolute path");
    }
    Ok(Path::new(base_path)
        .join("providers")
        .join("model-provider"))
}

fn validate_url(value: &str, label: &str) -> Result<()> {
    validate_bounded_trimmed(value, label, MAX_URL_BYTES)?;
    let parsed = Url::parse(value).map_err(|_| anyhow::anyhow!("{label} must be a valid URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => anyhow::bail!("{label} must use http or https"),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("{label} must not include userinfo");
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("{label} must include a host");
    }
    if parsed.fragment().is_some() {
        anyhow::bail!("{label} must not include a fragment");
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

fn validate_exact_modalities(actual: &[String], expected: &[&str], label: &str) -> Result<()> {
    if actual.len() != expected.len() {
        anyhow::bail!("{label} must be exactly {:?}", expected);
    }
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        if actual != expected {
            anyhow::bail!("{label} must be exactly {:?}", expected);
        }
    }
    Ok(())
}

fn validate_bounded_trimmed(value: &str, label: &str, max_bytes: usize) -> Result<()> {
    validate_trimmed(value, label)?;
    if value.len() > max_bytes {
        anyhow::bail!("{label} exceeds {max_bytes} bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_offer() -> ConfiguredOffer {
        ConfiguredOffer {
            id: "offer".to_string(),
            title: "Offer".to_string(),
            operation: "text.generate".to_string(),
            input_modalities: vec!["text/plain".to_string()],
            output_modalities: vec!["text/plain".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 1,
                input_bytes_limit: 1024,
                inline_output_bytes_limit: 1024,
                event_bytes_limit: 1024,
                runtime_ms_limit: 1000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 1000,
            },
            adapter: AdapterConfig::OpenAiCompatibleText {
                api_url: "https://example.test/v1/chat/completions".to_string(),
                api_key: Some("secret-a".to_string()),
                model: "gpt-test".to_string(),
            },
            enabled: true,
        }
    }

    fn artifact_offer(operation: &str) -> ConfiguredOffer {
        ConfiguredOffer {
            operation: operation.to_string(),
            input_modalities: vec!["application/json".to_string()],
            output_modalities: vec!["application/json".to_string()],
            adapter: AdapterConfig::HttpJobArtifact {
                create_url: "https://jobs.example.test/create".to_string(),
                status_url: "https://jobs.example.test/status".to_string(),
                cancel_url: Some("https://jobs.example.test/cancel".to_string()),
                bearer_token: Some("token-a".to_string()),
                poll_interval_ms: 1_000,
            },
            ..base_offer()
        }
    }

    #[test]
    fn journal_root_requires_absolute_operator_path() {
        assert!(journal_root("", None).is_err());
        assert!(journal_root("relative/path", None).is_err());
        assert!(journal_root("", Some("relative/path")).is_err());
        assert_eq!(
            journal_root("/var/lib/elastos", None).unwrap(),
            Path::new("/var/lib/elastos")
                .join("providers")
                .join("model-provider")
        );
        assert_eq!(
            journal_root("", Some("/var/lib/model-provider")).unwrap(),
            PathBuf::from("/var/lib/model-provider")
        );
    }

    #[test]
    fn adapter_config_rejects_unsafe_urls_and_secret_length() {
        let config = ProviderInitExtra {
            provider_id: None,
            journal_dir: Some("/tmp/model-provider".to_string()),
            offers: vec![ConfiguredOffer {
                adapter: AdapterConfig::HttpJobArtifact {
                    create_url: "https://example.test/create".to_string(),
                    status_url: "https://example.test/status".to_string(),
                    cancel_url: Some("https://example.test/cancel".to_string()),
                    bearer_token: Some("x".repeat(MAX_SECRET_BYTES + 1)),
                    poll_interval_ms: 1000,
                },
                ..base_offer()
            }],
        };
        assert!(config.validate("/tmp/base").is_err());

        let bridge = BridgeProviderConfig {
            base_path: "/tmp/base".to_string(),
            extra: json!({
                "journal_dir": "/tmp/model-provider",
                "offers": [{
                    "id": "offer",
                    "title": "Offer",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "policy": {
                        "concurrency_limit": 1,
                        "input_bytes_limit": 1024,
                        "inline_output_bytes_limit": 1024,
                        "event_bytes_limit": 1024,
                        "runtime_ms_limit": 1000,
                        "retention_secs": 60,
                        "cancel_settlement_timeout_ms": 1000
                    },
                    "adapter": {
                        "kind": "open_ai_compatible_text",
                        "api_url": "https://user@example.test/v1/chat#frag",
                        "model": "gpt-test"
                    }
                }]
            }),
        };
        let extra = serde_json::from_value::<ProviderInitExtra>(bridge.extra).unwrap();
        assert!(extra.validate(&bridge.base_path).is_err());
    }

    #[test]
    fn execution_binding_hash_changes_only_for_semantic_execution_inputs() {
        let openai = base_offer();
        let openai_hash = openai.execution_binding_hash().unwrap();

        let mut openai_api_url = openai.clone();
        if let AdapterConfig::OpenAiCompatibleText { api_url, .. } = &mut openai_api_url.adapter {
            *api_url = "https://example.test/v2/chat/completions".to_string();
        }
        assert_ne!(
            openai_hash,
            openai_api_url.execution_binding_hash().unwrap()
        );

        let mut openai_model = openai.clone();
        if let AdapterConfig::OpenAiCompatibleText { model, .. } = &mut openai_model.adapter {
            *model = "gpt-next".to_string();
        }
        assert_ne!(openai_hash, openai_model.execution_binding_hash().unwrap());

        let mut openai_key = openai.clone();
        if let AdapterConfig::OpenAiCompatibleText { api_key, .. } = &mut openai_key.adapter {
            *api_key = Some("secret-b".to_string());
        }
        assert_eq!(openai_hash, openai_key.execution_binding_hash().unwrap());

        let http_job = artifact_offer("image.generate");
        let http_job_hash = http_job.execution_binding_hash().unwrap();

        let mut http_job_create = http_job.clone();
        if let AdapterConfig::HttpJobArtifact { create_url, .. } = &mut http_job_create.adapter {
            *create_url = "https://jobs.example.test/create-v2".to_string();
        }
        assert_ne!(
            http_job_hash,
            http_job_create.execution_binding_hash().unwrap()
        );

        let mut http_job_status = http_job.clone();
        if let AdapterConfig::HttpJobArtifact { status_url, .. } = &mut http_job_status.adapter {
            *status_url = "https://jobs.example.test/status-v2".to_string();
        }
        assert_ne!(
            http_job_hash,
            http_job_status.execution_binding_hash().unwrap()
        );

        let mut http_job_cancel = http_job.clone();
        if let AdapterConfig::HttpJobArtifact { cancel_url, .. } = &mut http_job_cancel.adapter {
            *cancel_url = None;
        }
        assert_ne!(
            http_job_hash,
            http_job_cancel.execution_binding_hash().unwrap()
        );

        let mut http_job_poll = http_job.clone();
        if let AdapterConfig::HttpJobArtifact {
            poll_interval_ms, ..
        } = &mut http_job_poll.adapter
        {
            *poll_interval_ms = 2_000;
        }
        assert_ne!(
            http_job_hash,
            http_job_poll.execution_binding_hash().unwrap()
        );

        let mut http_job_token = http_job.clone();
        if let AdapterConfig::HttpJobArtifact { bearer_token, .. } = &mut http_job_token.adapter {
            *bearer_token = Some("token-b".to_string());
        }
        assert_eq!(
            http_job_hash,
            http_job_token.execution_binding_hash().unwrap()
        );

        let mut summary_change = openai.clone();
        summary_change.title = "Offer renamed".to_string();
        assert_ne!(
            openai_hash,
            summary_change.execution_binding_hash().unwrap()
        );

        let mut policy_change = openai;
        policy_change.policy.retention_secs = 61;
        assert_ne!(openai_hash, policy_change.execution_binding_hash().unwrap());
    }

    #[test]
    fn configured_offer_requires_canonical_modalities_and_adapter_pairs() {
        base_offer().validate().unwrap();
        artifact_offer("image.generate").validate().unwrap();
        artifact_offer("video.generate").validate().unwrap();

        let mut legacy_text = base_offer();
        legacy_text.input_modalities = vec!["text".to_string()];
        assert!(legacy_text.validate().is_err());

        let mut extra_text_modality = base_offer();
        extra_text_modality.input_modalities =
            vec!["text/plain".to_string(), "application/json".to_string()];
        assert!(extra_text_modality.validate().is_err());

        let mut wrong_text_operation = base_offer();
        wrong_text_operation.operation = "image.generate".to_string();
        assert!(wrong_text_operation.validate().is_err());

        let wrong_artifact_operation = artifact_offer("text.generate");
        assert!(wrong_artifact_operation.validate().is_err());

        let mut swapped_artifact_modalities = artifact_offer("image.generate");
        swapped_artifact_modalities.input_modalities = vec!["text/plain".to_string()];
        assert!(swapped_artifact_modalities.validate().is_err());

        let mut mismatched_adapter = base_offer();
        mismatched_adapter.adapter = AdapterConfig::HttpJobArtifact {
            create_url: "https://jobs.example.test/create".to_string(),
            status_url: "https://jobs.example.test/status".to_string(),
            cancel_url: Some("https://jobs.example.test/cancel".to_string()),
            bearer_token: Some("token-a".to_string()),
            poll_interval_ms: 1_000,
        };
        assert!(mismatched_adapter.validate().is_err());
    }

    #[test]
    fn offer_summary_redacts_adapter_secrets_and_reports_streaming_truthfully() {
        let openai_summary = base_offer().summary();
        assert!(openai_summary.stream_output);
        let openai_json = serde_json::to_string(&openai_summary).unwrap();
        assert!(!openai_json.contains("example.test"));
        assert!(!openai_json.contains("secret-a"));
        assert!(!openai_json.contains("token-a"));

        let artifact_summary = artifact_offer("video.generate").summary();
        assert!(!artifact_summary.stream_output);
        let artifact_json = serde_json::to_string(&artifact_summary).unwrap();
        assert!(!artifact_json.contains("jobs.example.test"));
        assert!(!artifact_json.contains("token-a"));
    }

    #[test]
    fn event_limit_fits_within_page_budget() {
        let event_limit = std::hint::black_box(MAX_EVENT_BYTES_LIMIT);
        let page_limit = std::hint::black_box(MAX_RUN_EVENTS_PAGE_BYTES_LIMIT);
        assert!(event_limit <= page_limit);
    }

    #[test]
    fn inline_output_limit_fits_terminal_event_budget() {
        let inline_output_limit = std::hint::black_box(MAX_INLINE_OUTPUT_BYTES_LIMIT);
        let event_limit = std::hint::black_box(MAX_EVENT_BYTES_LIMIT);
        assert!(inline_output_limit <= event_limit);
    }
}
