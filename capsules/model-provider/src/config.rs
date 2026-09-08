use crate::contract::{
    validate_bounded_trimmed, HostedOfferDisclosure, OfferPolicySummary, OfferSummary,
    HOSTED_PLACEMENT, HOSTED_SELECTION_PINNED, MODEL_POLICY_SCHEMA, SINGLE_DISPATCH_NO_RETRY,
    UPSTREAM_FALLBACK_OPERATOR_ASSERTED_DISABLED,
};
use anyhow::Result;
use elastos_model_contract::model_input_hash;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read as _;
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
pub const MAX_HOSTED_PROVIDER_LABEL_BYTES: usize = 128;
pub const MAX_HOSTED_POLICY_REF_BYTES: usize = 2 * 1024;
pub const MAX_BACKEND_COST_BYTES: usize = 128;
pub const MAX_BACKEND_COST_UNIT_BYTES: usize = 64;
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
pub const MAX_LOCAL_LLAMA_CONTEXT_SIZE: u32 = 32_768;
pub const MAX_LOCAL_LLAMA_PARALLEL: u32 = 64;
pub const MAX_LOCAL_LLAMA_THREADS: u32 = 512;
pub const MAX_LOCAL_LLAMA_GPU_LAYERS: u32 = 4_096;
pub const MAX_LOCAL_LLAMA_HEALTH_TIMEOUT_MS: u64 = 120_000;
pub const MAX_LOCAL_LLAMA_SHUTDOWN_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeProviderConfig {
    #[serde(default)]
    pub base_path: String,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub encryption_key: String,
    #[serde(default)]
    pub extra: Value,
}

impl BridgeProviderConfig {
    pub fn validate(&self) -> Result<()> {
        let _ = self.read_only;
        if !self.base_path.is_empty() {
            validate_bounded_trimmed(&self.base_path, "base_path", MAX_BASE_PATH_BYTES)?;
            if !Path::new(&self.base_path).is_absolute() {
                anyhow::bail!("base_path must be an absolute path");
            }
        }
        for allowed_path in &self.allowed_paths {
            validate_bounded_trimmed(allowed_path, "allowed_paths", MAX_BASE_PATH_BYTES)?;
        }
        if !self.encryption_key.is_empty() {
            validate_bounded_trimmed(&self.encryption_key, "encryption_key", MAX_SECRET_BYTES)?;
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
    pub fn validate(&self, bridge: &BridgeProviderConfig) -> Result<()> {
        if let Some(provider_id) = self.provider_id.as_deref() {
            validate_bounded_trimmed(provider_id, "provider_id", MAX_PROVIDER_ID_BYTES)?;
        }
        if let Some(journal_dir) = self.journal_dir.as_deref() {
            validate_bounded_trimmed(journal_dir, "journal_dir", MAX_JOURNAL_DIR_BYTES)?;
            if !Path::new(journal_dir).is_absolute() {
                anyhow::bail!("journal_dir must be an absolute path");
            }
        }
        if bridge.base_path.is_empty() && self.journal_dir.is_none() {
            anyhow::bail!("model provider init requires base_path or journal_dir");
        }
        if self.offers.len() > MAX_OFFER_COUNT {
            anyhow::bail!("model provider offers exceed {}", MAX_OFFER_COUNT);
        }
        let mut offer_ids = BTreeSet::new();
        for offer in &self.offers {
            offer.validate()?;
            offer.validate_local_artifacts(bridge)?;
            if !offer_ids.insert(offer.id.as_str()) {
                anyhow::bail!("duplicate model offer id in provider config");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalArtifactConfig {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalLlamaSettings {
    pub context_size: u32,
    pub parallel: u32,
    pub threads: u32,
    pub batch_threads: u32,
    pub gpu_layers: u32,
    pub health_timeout_ms: u64,
    pub shutdown_timeout_ms: u64,
    pub enable_thinking: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostedDisclosureConfig {
    pub backend_provider_label: String,
    pub selection_mode: String,
    pub privacy_policy_ref: String,
    pub terms_ref: String,
    pub upstream_routing_fallback_assertion: String,
}

impl HostedDisclosureConfig {
    fn validate(&self) -> Result<()> {
        validate_bounded_trimmed(
            &self.backend_provider_label,
            "hosted backend_provider_label",
            MAX_HOSTED_PROVIDER_LABEL_BYTES,
        )?;
        if self.selection_mode != HOSTED_SELECTION_PINNED {
            anyhow::bail!("hosted selection_mode must be pinned");
        }
        validate_bounded_trimmed(
            &self.privacy_policy_ref,
            "hosted privacy_policy_ref",
            MAX_HOSTED_POLICY_REF_BYTES,
        )?;
        validate_bounded_trimmed(
            &self.terms_ref,
            "hosted terms_ref",
            MAX_HOSTED_POLICY_REF_BYTES,
        )?;
        if self.upstream_routing_fallback_assertion != UPSTREAM_FALLBACK_OPERATOR_ASSERTED_DISABLED
        {
            anyhow::bail!(
                "hosted upstream_routing_fallback_assertion must be operator_asserted_disabled"
            );
        }
        Ok(())
    }

    fn summary(&self, requested_selector: &str) -> HostedOfferDisclosure {
        HostedOfferDisclosure {
            placement: HOSTED_PLACEMENT.to_string(),
            backend_provider_label: self.backend_provider_label.clone(),
            selection_mode: self.selection_mode.clone(),
            requested_selector: requested_selector.to_string(),
            privacy_policy_ref: self.privacy_policy_ref.clone(),
            terms_ref: self.terms_ref.clone(),
            provider_request_policy: SINGLE_DISPATCH_NO_RETRY.to_string(),
            upstream_routing_fallback_assertion: self.upstream_routing_fallback_assertion.clone(),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_hosted_disclosure() -> HostedDisclosureConfig {
    HostedDisclosureConfig {
        backend_provider_label: "Fixture Provider".to_string(),
        selection_mode: HOSTED_SELECTION_PINNED.to_string(),
        privacy_policy_ref: "fixture:privacy:v1".to_string(),
        terms_ref: "fixture:terms:v1".to_string(),
        upstream_routing_fallback_assertion: UPSTREAM_FALLBACK_OPERATOR_ASSERTED_DISABLED
            .to_string(),
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
        hosted: HostedDisclosureConfig,
    },
    OpenAiResponsesText {
        api_url: String,
        #[serde(default)]
        api_key: Option<String>,
        model: String,
        hosted: HostedDisclosureConfig,
    },
    LocalLlamaCppText {
        engine: LocalArtifactConfig,
        model: LocalArtifactConfig,
        settings: LocalLlamaSettings,
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
                hosted,
            }
            | Self::OpenAiResponsesText {
                api_url,
                api_key,
                model,
                hosted,
            } => {
                validate_url(api_url, "openai adapter api_url")?;
                if let Some(api_key) = api_key.as_deref() {
                    validate_bounded_trimmed(api_key, "openai adapter api_key", MAX_SECRET_BYTES)?;
                }
                validate_bounded_trimmed(model, "openai adapter model", MAX_MODEL_BYTES)?;
                hosted.validate()?;
            }
            Self::LocalLlamaCppText {
                engine,
                model,
                settings,
            } => {
                validate_local_artifact_config(engine, "local llama engine")?;
                validate_local_artifact_config(model, "local llama model")?;
                settings.validate()?;
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
        matches!(
            self,
            Self::OpenAiCompatibleText { .. }
                | Self::OpenAiResponsesText { .. }
                | Self::LocalLlamaCppText { .. }
        )
    }
}

impl LocalLlamaSettings {
    fn validate(&self) -> Result<()> {
        validate_nonzero_bounded(
            self.context_size,
            MAX_LOCAL_LLAMA_CONTEXT_SIZE,
            "local llama context_size",
        )?;
        validate_nonzero_bounded(
            self.parallel,
            MAX_LOCAL_LLAMA_PARALLEL,
            "local llama parallel",
        )?;
        validate_nonzero_bounded(self.threads, MAX_LOCAL_LLAMA_THREADS, "local llama threads")?;
        validate_nonzero_bounded(
            self.batch_threads,
            MAX_LOCAL_LLAMA_THREADS,
            "local llama batch_threads",
        )?;
        if self.gpu_layers > MAX_LOCAL_LLAMA_GPU_LAYERS {
            anyhow::bail!("local llama gpu_layers must be in 0..={MAX_LOCAL_LLAMA_GPU_LAYERS}");
        }
        validate_nonzero_bounded(
            self.health_timeout_ms,
            MAX_LOCAL_LLAMA_HEALTH_TIMEOUT_MS,
            "local llama health_timeout_ms",
        )?;
        validate_nonzero_bounded(
            self.shutdown_timeout_ms,
            MAX_LOCAL_LLAMA_SHUTDOWN_TIMEOUT_MS,
            "local llama shutdown_timeout_ms",
        )?;
        Ok(())
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
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
        let hosted = match &self.adapter {
            AdapterConfig::OpenAiCompatibleText { model, hosted, .. }
            | AdapterConfig::OpenAiResponsesText { model, hosted, .. } => {
                Some(hosted.summary(model))
            }
            _ => None,
        };
        OfferSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            operation: self.operation.clone(),
            input_modalities: self.input_modalities.clone(),
            output_modalities: self.output_modalities.clone(),
            stream_output: self.adapter.stream_output(),
            policy: self.policy.summary(),
            hosted,
        }
    }

    pub fn execution_binding_hash(&self) -> Result<String> {
        let adapter = match &self.adapter {
            AdapterConfig::OpenAiCompatibleText { api_url, model, .. } => json!({
                "kind": "open_ai_compatible_text",
                "api_url": api_url,
                "model": model,
            }),
            AdapterConfig::OpenAiResponsesText { api_url, model, .. } => json!({
                "kind": "open_ai_responses_text",
                "api_url": api_url,
                "model": model,
            }),
            AdapterConfig::LocalLlamaCppText {
                engine,
                model,
                settings,
            } => json!({
                "kind": "local_llama_cpp_text",
                "engine_sha256": engine.sha256,
                "model_sha256": model.sha256,
                "settings": settings,
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
        Ok(model_input_hash(&json!({
            "adapter": adapter,
            "offer": self.summary(),
        }))?)
    }

    fn validate_canonical_modalities(&self) -> Result<()> {
        match &self.adapter {
            AdapterConfig::OpenAiCompatibleText { .. }
            | AdapterConfig::OpenAiResponsesText { .. }
            | AdapterConfig::LocalLlamaCppText { .. } => {
                if self.operation != "text.generate" {
                    anyhow::bail!("text generation offers require operation text.generate");
                }
                validate_exact_modalities(
                    &self.input_modalities,
                    &["text/plain"],
                    "text generation input_modalities",
                )?;
                validate_exact_modalities(
                    &self.output_modalities,
                    &["text/plain"],
                    "text generation output_modalities",
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

    fn validate_local_artifacts(&self, bridge: &BridgeProviderConfig) -> Result<()> {
        let AdapterConfig::LocalLlamaCppText { engine, model, .. } = &self.adapter else {
            return Ok(());
        };
        validate_local_artifact(bridge, engine, true, "local llama engine")?;
        validate_local_artifact(bridge, model, false, "local llama model")
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

fn validate_nonzero_bounded<T>(value: T, max: T, label: &str) -> Result<()>
where
    T: Copy + Ord + From<u8> + std::fmt::Display,
{
    if value < T::from(1) || value > max {
        anyhow::bail!("{label} must be in 1..={max}");
    }
    Ok(())
}

fn validate_local_artifact_config(config: &LocalArtifactConfig, label: &str) -> Result<()> {
    validate_bounded_trimmed(&config.path, label, MAX_BASE_PATH_BYTES)?;
    if !Path::new(&config.path).is_absolute() {
        anyhow::bail!("{label} path must be absolute");
    }
    let digest = config.sha256.strip_prefix("sha256:").unwrap_or_default();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("{label} sha256 must be canonical lowercase sha256 hex");
    }
    Ok(())
}

pub(crate) fn validate_local_artifact(
    bridge: &BridgeProviderConfig,
    artifact: &LocalArtifactConfig,
    executable: bool,
    label: &str,
) -> Result<()> {
    validate_local_artifact_config(artifact, label)?;
    validate_bounded_trimmed(&bridge.base_path, "base_path", MAX_BASE_PATH_BYTES)?;
    let base = Path::new(&bridge.base_path);
    if !base.is_absolute() {
        anyhow::bail!("base_path must be an absolute path");
    }
    let canonical_base =
        fs::canonicalize(base).map_err(|_| anyhow::anyhow!("base_path is unavailable"))?;
    if canonical_base != base {
        anyhow::bail!("base_path must not contain symlinks");
    }
    let path = Path::new(&artifact.path);
    let canonical =
        fs::canonicalize(path).map_err(|_| anyhow::anyhow!("{label} is unavailable"))?;
    if canonical != path {
        anyhow::bail!("{label} path must not contain symlinks");
    }
    let allowed = if bridge.allowed_paths.is_empty() {
        canonical.starts_with(&canonical_base)
    } else {
        bridge.allowed_paths.iter().any(|relative| {
            let relative = Path::new(relative);
            relative.is_relative()
                && relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                && fs::canonicalize(canonical_base.join(relative))
                    .map(|root| root.starts_with(&canonical_base) && canonical.starts_with(root))
                    .unwrap_or(false)
        })
    };
    if !allowed {
        anyhow::bail!("{label} is outside Runtime-admitted paths");
    }
    validate_local_artifact_metadata(artifact, executable)
}

pub(crate) fn revalidate_local_artifact(
    artifact: &LocalArtifactConfig,
    executable: bool,
    deadline: std::time::Instant,
) -> Result<()> {
    let label = if executable {
        "local llama engine"
    } else {
        "local llama model"
    };
    if std::time::Instant::now() >= deadline {
        anyhow::bail!("{label} verification deadline expired");
    }
    let path = Path::new(&artifact.path);
    let canonical =
        fs::canonicalize(path).map_err(|_| anyhow::anyhow!("{label} is unavailable"))?;
    if canonical != path {
        anyhow::bail!("{label} path must not contain symlinks");
    }
    validate_local_artifact_metadata(artifact, executable)?;
    let expected = artifact.sha256.strip_prefix("sha256:").unwrap_or_default();
    let mut file = fs::File::open(path).map_err(|_| anyhow::anyhow!("{label} is unavailable"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("{label} verification deadline expired");
        }
        let read = file
            .read(&mut buffer)
            .map_err(|_| anyhow::anyhow!("{label} could not be verified"))?;
        if read == 0 {
            break;
        }
        use sha2::Digest as _;
        hasher.update(&buffer[..read]);
    }
    use sha2::Digest as _;
    if format!("{:x}", hasher.finalize()) != expected {
        anyhow::bail!("{label} checksum does not match operator config");
    }
    Ok(())
}

fn validate_local_artifact_metadata(
    artifact: &LocalArtifactConfig,
    executable: bool,
) -> Result<()> {
    let label = if executable {
        "local llama engine"
    } else {
        "local llama model"
    };
    let path = Path::new(&artifact.path);
    let metadata =
        fs::symlink_metadata(path).map_err(|_| anyhow::anyhow!("{label} is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        anyhow::bail!("{label} must be a non-empty regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || mode & 0o022 != 0
        {
            anyhow::bail!("{label} must be an owner-controlled single-link file");
        }
        if executable && mode & 0o111 == 0 {
            anyhow::bail!("{label} must be executable");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt as _};

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
                hosted: test_hosted_disclosure(),
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

    fn local_llama_offer(engine: &Path, model: &Path) -> ConfiguredOffer {
        ConfiguredOffer {
            adapter: AdapterConfig::LocalLlamaCppText {
                engine: LocalArtifactConfig {
                    path: engine.to_string_lossy().into_owned(),
                    sha256:
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_string(),
                },
                model: LocalArtifactConfig {
                    path: model.to_string_lossy().into_owned(),
                    sha256:
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .to_string(),
                },
                settings: LocalLlamaSettings {
                    context_size: 4_096,
                    parallel: 1,
                    threads: 2,
                    batch_threads: 2,
                    gpu_layers: 0,
                    health_timeout_ms: 1_000,
                    shutdown_timeout_ms: 100,
                    enable_thinking: false,
                },
            },
            ..base_offer()
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_llama_init_admits_metadata_without_hashing_large_artifacts() {
        let root = crate::test_support::temp_root_path("model-provider-config", "local-llama");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let bin = root.join("bin");
        let models = root.join("models");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&models).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let engine = bin.join("llama-server");
        let model = models.join("model.gguf");
        fs::write(&engine, b"engine bytes").unwrap();
        fs::write(&model, b"model bytes do not match configured digest").unwrap();
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&model, fs::Permissions::from_mode(0o600)).unwrap();
        let extra = ProviderInitExtra {
            provider_id: Some("model-provider".to_string()),
            journal_dir: Some(root.join("journal").to_string_lossy().into_owned()),
            offers: vec![local_llama_offer(&engine, &model)],
        };
        let bridge = BridgeProviderConfig {
            base_path: root.to_string_lossy().into_owned(),
            allowed_paths: Vec::new(),
            ..Default::default()
        };

        extra.validate(&bridge).unwrap();

        let outside = crate::test_support::temp_root_path("model-provider-config", "outside");
        fs::create_dir_all(&outside).unwrap();
        let outside = fs::canonicalize(outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
        let outside_model = outside.join("model.gguf");
        fs::write(&outside_model, b"outside").unwrap();
        fs::set_permissions(&outside_model, fs::Permissions::from_mode(0o600)).unwrap();
        let mut outside_extra = extra.clone();
        if let AdapterConfig::LocalLlamaCppText { model, .. } = &mut outside_extra.offers[0].adapter
        {
            model.path = outside_model.to_string_lossy().into_owned();
        }
        assert!(outside_extra.validate(&bridge).is_err());

        let linked_allowed = root.join("linked-models");
        symlink(&outside, &linked_allowed).unwrap();
        let linked_allowed_bridge = BridgeProviderConfig {
            base_path: root.to_string_lossy().into_owned(),
            allowed_paths: vec!["linked-models".to_string()],
            ..Default::default()
        };
        assert!(outside_extra.validate(&linked_allowed_bridge).is_err());

        let linked_engine = bin.join("linked-server");
        symlink(&engine, &linked_engine).unwrap();
        let mut linked_extra = extra;
        if let AdapterConfig::LocalLlamaCppText { engine, .. } = &mut linked_extra.offers[0].adapter
        {
            engine.path = linked_engine.to_string_lossy().into_owned();
        }
        assert!(linked_extra.validate(&bridge).is_err());
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
        let bridge = BridgeProviderConfig {
            base_path: "/tmp/base".to_string(),
            ..Default::default()
        };
        assert!(config.validate(&bridge).is_err());

        let bridge = BridgeProviderConfig {
            base_path: "/tmp/base".to_string(),
            allowed_paths: Vec::new(),
            read_only: false,
            encryption_key: String::new(),
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
                        "model": "gpt-test",
                        "hosted": {
                            "backend_provider_label": "Fixture Provider",
                            "selection_mode": "pinned",
                            "privacy_policy_ref": "fixture:privacy:v1",
                            "terms_ref": "fixture:terms:v1",
                            "upstream_routing_fallback_assertion": "operator_asserted_disabled"
                        }
                    }
                }]
            }),
        };
        let extra = serde_json::from_value::<ProviderInitExtra>(bridge.extra.clone()).unwrap();
        assert!(extra.validate(&bridge).is_err());
    }

    #[test]
    fn bridge_provider_config_accepts_runtime_init_envelope_fields() {
        let bridge: BridgeProviderConfig = serde_json::from_value(json!({
            "base_path": "/tmp/model-provider",
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": "",
            "extra": {
                "provider_id": "model-provider",
                "journal_dir": "/tmp/model-provider/journal",
                "offers": []
            }
        }))
        .unwrap();

        bridge.validate().unwrap();
        let extra = serde_json::from_value::<ProviderInitExtra>(bridge.extra.clone()).unwrap();
        extra.validate(&bridge).unwrap();
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
        assert_eq!(
            serde_json::to_value(&openai_summary).unwrap()["hosted"],
            json!({
                "placement": "hosted",
                "backend_provider_label": "Fixture Provider",
                "selection_mode": "pinned",
                "requested_selector": "gpt-test",
                "privacy_policy_ref": "fixture:privacy:v1",
                "terms_ref": "fixture:terms:v1",
                "provider_request_policy": "single_dispatch_no_retry",
                "upstream_routing_fallback_assertion": "operator_asserted_disabled",
            })
        );
        let openai_json = serde_json::to_string(&openai_summary).unwrap();
        assert!(!openai_json.contains("example.test"));
        assert!(!openai_json.contains("secret-a"));
        assert!(!openai_json.contains("token-a"));

        let artifact_summary = artifact_offer("video.generate").summary();
        assert!(!artifact_summary.stream_output);
        assert!(artifact_summary.hosted.is_none());
        let artifact_json = serde_json::to_string(&artifact_summary).unwrap();
        assert!(!artifact_json.contains("jobs.example.test"));
        assert!(!artifact_json.contains("token-a"));
    }

    #[test]
    fn responses_adapter_has_distinct_binding_and_shared_hosted_disclosure() {
        let endpoint = "https://example.test/v1/text";
        let model = "model-test";
        let mut chat = base_offer();
        if let AdapterConfig::OpenAiCompatibleText {
            api_url,
            model: configured_model,
            ..
        } = &mut chat.adapter
        {
            *api_url = endpoint.to_string();
            *configured_model = model.to_string();
        }
        let mut responses = chat.clone();
        responses.adapter = AdapterConfig::OpenAiResponsesText {
            api_url: endpoint.to_string(),
            api_key: Some("sentinel-responses-key".to_string()),
            model: model.to_string(),
            hosted: test_hosted_disclosure(),
        };

        responses.validate().unwrap();
        let responses_hash = responses.execution_binding_hash().unwrap();
        assert_ne!(responses_hash, chat.execution_binding_hash().unwrap());
        let mut rotated = responses.clone();
        if let AdapterConfig::OpenAiResponsesText { api_key, .. } = &mut rotated.adapter {
            *api_key = Some("rotated-responses-key".to_string());
        }
        assert_eq!(responses_hash, rotated.execution_binding_hash().unwrap());
        assert_eq!(responses.summary(), chat.summary());
        let public = serde_json::to_string(&responses.summary()).unwrap();
        assert!(!public.contains("example.test"));
        assert!(!public.contains("sentinel-responses-key"));
    }

    #[test]
    fn hosted_disclosure_requires_pinned_selection_and_operator_fallback_assertion() {
        let mut offer = base_offer();
        if let AdapterConfig::OpenAiCompatibleText { hosted, .. } = &mut offer.adapter {
            hosted.selection_mode = "provider_auto".to_string();
        }
        assert!(offer.validate().is_err());

        if let AdapterConfig::OpenAiCompatibleText { hosted, .. } = &mut offer.adapter {
            hosted.selection_mode = HOSTED_SELECTION_PINNED.to_string();
            hosted.upstream_routing_fallback_assertion = "disabled".to_string();
        }
        assert!(offer.validate().is_err());

        let mut offer = base_offer();
        if let AdapterConfig::OpenAiCompatibleText { hosted, .. } = &mut offer.adapter {
            hosted.backend_provider_label = "Fixture\nProvider".to_string();
        }
        assert!(offer.validate().is_err());
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
