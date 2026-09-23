//! Runtime-owned model configuration shared by startup and admitted-content activation.
use anyhow::Context as _;
use elastos_runtime::provider;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read as _, Write as _};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MODEL_PROVIDER_ID: &str = "model-provider";
const MODEL_PROVIDER_CONFIG_FILE_NAME: &str = "config.json";
const MODEL_PROVIDER_CONFIG_MAX_BYTES: usize = 256 * 1024;
const MODEL_PROVIDER_VALIDATE_FIXTURES_FILE_NAME: &str = "validate-fixtures.json";
const MODEL_PROVIDER_VALIDATE_FIXTURES_MAX_BYTES: usize = 8 * 1024;
const OPENROUTER_DECISIONS_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const HOSTED_ADAPTER_KIND: &str = "open_ai_compatible_text";
const MODEL_PROVIDER_SECRETS_DIR_NAME: &str = "secrets";
const HOSTED_DISPLAY_NAME_MAX_BYTES: usize = 80;
const HOSTED_SECRET_REF_PREFIX: &str = "runtime:model-provider:";
static MODEL_PROVIDER_CONFIG_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Loopback HTTP(S) URL accepted only from owner-scoped validate-fixtures.json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopbackHttpUrl(String);

impl LoopbackHttpUrl {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedValidateFixtures {
    pub openrouter_models_url: LoopbackHttpUrl,
    pub venice_rate_limits_url: LoopbackHttpUrl,
    pub venice_models_url: LoopbackHttpUrl,
    pub openrouter_chat_url: Option<LoopbackHttpUrl>,
    pub venice_chat_url: Option<LoopbackHttpUrl>,
    pub loopback_ca_pem: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateFixturesFile {
    openrouter_models_url: String,
    venice_rate_limits_url: String,
    venice_models_url: String,
    #[serde(default)]
    openrouter_chat_url: Option<String>,
    #[serde(default)]
    venice_chat_url: Option<String>,
    #[serde(default)]
    loopback_ca_pem: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostedAiProvider {
    OpenRouter,
    Venice,
}

impl HostedAiProvider {
    pub(crate) const ALL: [Self; 2] = [Self::OpenRouter, Self::Venice];

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim() {
            "openrouter" => Ok(Self::OpenRouter),
            "venice" => Ok(Self::Venice),
            _ => Err(anyhow::anyhow!("invalid AI provider")),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Venice => "venice",
        }
    }

    pub(crate) fn offer_id(self) -> &'static str {
        match self {
            Self::OpenRouter => "model:openrouter",
            Self::Venice => "model:venice",
        }
    }

    pub(crate) fn chat_url(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/api/v1/chat/completions",
            Self::Venice => "https://api.venice.ai/api/v1/chat/completions",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::Venice => "Venice",
        }
    }

    fn privacy_policy_ref(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/privacy",
            Self::Venice => "https://docs.venice.ai/overview/privacy",
        }
    }

    fn terms_ref(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/terms",
            Self::Venice => "https://venice.ai/legal/tos",
        }
    }

    pub(crate) fn from_offer_id(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.offer_id() == value.trim())
    }

    pub(crate) fn from_api_url(value: &str) -> Option<Self> {
        if value.trim() == OPENROUTER_DECISIONS_URL {
            return Some(Self::OpenRouter);
        }
        Self::ALL
            .into_iter()
            .find(|provider| provider.chat_url() == value.trim())
    }

    pub(crate) fn share_terms_ack(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter-5.1-5.2+model",
            Self::Venice => "venice-7.3+model",
        }
    }

    pub(crate) fn share_payer(self) -> &'static str {
        let _ = self;
        "this Home"
    }

    pub(crate) fn share_terms_summary(self) -> &'static str {
        match self {
            Self::OpenRouter => {
                "OpenRouter Terms 5.1-5.2 plus the selected model terms. This Home pays. OpenRouter receives prompts."
            }
            Self::Venice => {
                "Venice TOS 7.3 End User API terms apply to the selected model. Section 7.1 is personal use and is not a Share grant. This Home pays. Venice receives prompts."
            }
        }
    }
}

const HOSTED_SHARE_PAYER: &str = "this Home";
const HOSTED_SHARE_TERMS_ACK_MAX_BYTES: usize = 64;
const HOSTED_SHARE_PROCESSOR_MAX_BYTES: usize = 64;
const HOSTED_SHARE_MODEL_MAX_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelProviderOperatorConfigFile {
    offers: Vec<serde_json::Value>,
}

fn model_provider_root_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("providers").join(MODEL_PROVIDER_ID)
}

fn model_provider_config_path(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join(MODEL_PROVIDER_CONFIG_FILE_NAME)
}

fn model_provider_validate_fixtures_path(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join(MODEL_PROVIDER_VALIDATE_FIXTURES_FILE_NAME)
}

fn model_provider_secrets_dir(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join(MODEL_PROVIDER_SECRETS_DIR_NAME)
}

fn hosted_secret_file_name(offer_id: &str) -> anyhow::Result<String> {
    let offer_id = offer_id.trim();
    if offer_id.is_empty()
        || offer_id.bytes().any(|byte| {
            !byte.is_ascii_alphanumeric() && byte != b':' && byte != b'-' && byte != b'_'
        })
    {
        anyhow::bail!("invalid hosted instance id");
    }
    Ok(offer_id.replace(':', "_"))
}

fn new_hosted_instance_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("model:hosted-{hex}")
}

fn hosted_secret_ref(offer_id: &str) -> String {
    format!("{HOSTED_SECRET_REF_PREFIX}{offer_id}")
}

fn hosted_provider_from_offer(offer: &serde_json::Value) -> Option<HostedAiProvider> {
    if let Some(provider) = offer
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(HostedAiProvider::from_offer_id)
    {
        return Some(provider);
    }
    offer
        .pointer("/adapter/api_url")
        .and_then(serde_json::Value::as_str)
        .and_then(HostedAiProvider::from_api_url)
}

fn offer_id_of(offer: &serde_json::Value) -> Option<&str> {
    offer
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_hosted_instance_offer(offer: &serde_json::Value) -> bool {
    hosted_provider_from_offer(offer).is_some()
        || offer
            .get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id.starts_with("model:hosted-"))
}

/// Parse a validate-fixture URL. Accept only http(s), host 127.0.0.1, an explicit
/// numeric port, and a path. Reject DNS, IPv6, localhost, credentials, and
/// fragments.
pub(crate) fn parse_loopback_http_url(raw: &str) -> anyhow::Result<LoopbackHttpUrl> {
    let trimmed = raw.trim();
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| anyhow::anyhow!("hosted validate fixture URL is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("hosted validate fixture URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("hosted validate fixture URL must omit credentials");
    }
    match parsed.host() {
        Some(url::Host::Ipv4(addr)) if addr.octets() == [127, 0, 0, 1] => {}
        _ => anyhow::bail!("hosted validate fixture URL must use host 127.0.0.1"),
    }
    let Some(port) = parsed.port() else {
        anyhow::bail!("hosted validate fixture URL must include a numeric port");
    };
    if port == 0 {
        anyhow::bail!("hosted validate fixture URL must include a numeric port");
    }
    if parsed.fragment().is_some() {
        anyhow::bail!("hosted validate fixture URL must omit a fragment");
    }
    let path = parsed.path();
    if !path.starts_with('/') || path.len() < 2 {
        anyhow::bail!("hosted validate fixture URL must include a path");
    }
    Ok(LoopbackHttpUrl(parsed.as_str().to_string()))
}

fn optional_loopback_http_url(raw: Option<&str>) -> anyhow::Result<Option<LoopbackHttpUrl>> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) => parse_loopback_http_url(value).map(Some),
    }
}

fn load_hosted_validate_fixtures_unlocked(
    data_dir: &Path,
) -> anyhow::Result<Option<HostedValidateFixtures>> {
    let path = model_provider_validate_fixtures_path(data_dir);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to inspect model-provider validate fixtures {}",
                    path.display()
                )
            })
        }
    };
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    let bytes = read_model_provider_private_file(
        &path,
        &metadata,
        MODEL_PROVIDER_VALIDATE_FIXTURES_MAX_BYTES,
        "model-provider validate fixtures",
    )?;
    let file: ValidateFixturesFile = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("model-provider validate fixtures are invalid"))?;
    Ok(Some(HostedValidateFixtures {
        openrouter_models_url: parse_loopback_http_url(&file.openrouter_models_url)?,
        venice_rate_limits_url: parse_loopback_http_url(&file.venice_rate_limits_url)?,
        venice_models_url: parse_loopback_http_url(&file.venice_models_url)?,
        openrouter_chat_url: optional_loopback_http_url(file.openrouter_chat_url.as_deref())?,
        venice_chat_url: optional_loopback_http_url(file.venice_chat_url.as_deref())?,
        loopback_ca_pem: file.loopback_ca_pem,
    }))
}

/// Load owner-scoped validate fixtures. An absent file keeps the public HTTPS pins.
pub(crate) fn load_hosted_validate_fixtures(
    data_dir: &Path,
) -> anyhow::Result<Option<HostedValidateFixtures>> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    load_hosted_validate_fixtures_unlocked(data_dir)
}

fn model_provider_journal_dir(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join("journal")
}

pub fn model_provider_bridge_config(
    data_dir: &Path,
) -> anyhow::Result<provider::BridgeProviderConfig> {
    // Resolve Runtime's data-root alias once for both initial Init and refresh.
    // The provider still requires canonical paths for every local artifact.
    let canonical_data_dir =
        fs::canonicalize(data_dir).context("model-provider Runtime root is unavailable")?;
    let data_dir = canonical_data_dir.as_path();
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let loaded = load_model_provider_operator_offers(data_dir)?;
    if offers_need_secret_migration(&loaded) {
        persist_model_provider_operator_offers(data_dir, loaded.clone())?;
    }
    let fixtures = load_hosted_validate_fixtures_unlocked(data_dir)?;
    let offers: Vec<serde_json::Value> = load_model_provider_operator_offers(data_dir)?
        .into_iter()
        .map(|offer| materialize_provider_offer(offer, fixtures.as_ref()))
        .map(strip_runtime_share_fields)
        .collect();
    Ok(provider::BridgeProviderConfig {
        base_path: data_dir.to_string_lossy().into_owned(),
        extra: serde_json::json!({
            "provider_id": MODEL_PROVIDER_ID,
            "journal_dir": model_provider_journal_dir(data_dir).to_string_lossy().into_owned(),
            "offers": offers,
        }),
        ..Default::default()
    })
}

/// Return verified Init config with the existing inventory worker guard.
/// Startup retains the guard through provider spawn, Init and registration.
pub async fn model_provider_config(
    data_dir: &Path,
    registry: &provider::ProviderRegistry,
) -> anyhow::Result<(provider::BridgeProviderConfig, Option<fs::File>)> {
    let config = model_provider_bridge_config(data_dir)?;
    #[cfg(unix)]
    {
        let mut config = config;
        let worker =
            super::append_admitted_model_startup_offers(data_dir, registry, &mut config).await?;
        Ok((config, worker))
    }
    #[cfg(not(unix))]
    {
        let _ = registry;
        Ok((config, None))
    }
}

pub(crate) fn load_model_provider_operator_offers(
    data_dir: &Path,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let config_path = model_provider_config_path(data_dir);
    let metadata = match fs::symlink_metadata(&config_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to inspect model-provider operator config {}",
                    config_path.display()
                )
            })
        }
    };
    let config_root = model_provider_root_dir(data_dir);
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(&config_root, "model-provider config root")?;
    let bytes = read_model_provider_private_file(
        &config_path,
        &metadata,
        MODEL_PROVIDER_CONFIG_MAX_BYTES,
        "model-provider operator config",
    )?;
    let raw = String::from_utf8(bytes)
        .context("model-provider operator config must be valid UTF-8 JSON")?;
    let config: ModelProviderOperatorConfigFile = serde_json::from_str(&raw)
        .context("model-provider operator config must contain only the top-level offers key")?;
    Ok(config.offers)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedModelOfferHint {
    pub offer_title: String,
    pub provider_label: String,
    pub requested_selector: String,
    pub privacy_policy_ref: String,
    pub fallback: String,
}

/// Best-effort hosted-offer facts for Assistant selection and the Jev shadow lens.
/// This function returns no URLs or credentials.
pub(crate) fn hosted_model_offer_hint(
    data_dir: &Path,
    offer_id: &str,
) -> Option<HostedModelOfferHint> {
    let offer_id = offer_id.trim();
    if offer_id.is_empty() {
        return None;
    }
    let offers = load_model_provider_operator_offers(data_dir).ok()?;
    hosted_hint_from_offers(&offers, offer_id)
}

fn hosted_hint_from_offers(
    offers: &[serde_json::Value],
    offer_id: &str,
) -> Option<HostedModelOfferHint> {
    let offer = offers
        .iter()
        .find(|offer| offer.get("id").and_then(serde_json::Value::as_str) == Some(offer_id))?;
    let adapter = offer.get("adapter")?;
    let kind = adapter.get("kind").and_then(serde_json::Value::as_str)?;
    if kind != "open_ai_compatible_text"
        && kind != "openai_compatible_text"
        && kind != "open_ai_responses_text"
        && kind != "openai_responses_text"
        && kind != "open_router_decisions"
    {
        return None;
    }
    let hosted = adapter.get("hosted")?;
    let provider_label = hosted
        .get("backend_provider_label")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if provider_label.is_empty() {
        return None;
    }
    let offer_title = offer
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&provider_label)
        .to_string();
    Some(HostedModelOfferHint {
        offer_title,
        provider_label,
        requested_selector: adapter
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        privacy_policy_ref: hosted
            .get("privacy_policy_ref")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        fallback: hosted
            .get("upstream_routing_fallback_assertion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

/// Resolve one saved decision instance without using its display name as authority.
pub(super) fn decision_hosted_offer(data_dir: &Path, id: &str) -> Option<HostedModelOfferHint> {
    let offers = load_model_provider_operator_offers(data_dir).ok()?;
    let offer = offers
        .iter()
        .find(|offer| offer.get("id").and_then(serde_json::Value::as_str) == Some(id))?;
    if !operator_offer_enabled(offer)
        || !operator_offer_has_key(offer)
        || offer.get("operation").and_then(serde_json::Value::as_str)
            != Some(elastos_model_contract::decisions::OPERATION)
        || offer
            .pointer("/adapter/kind")
            .and_then(serde_json::Value::as_str)
            != Some("open_router_decisions")
        || !offer
            .pointer("/adapter/model")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|model| model.starts_with("typesafe/jev-"))
    {
        return None;
    }
    hosted_hint_from_offers(std::slice::from_ref(offer), id)
}

fn approval_lens_selection_path(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join("approval-lens.json")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalLensSelection {
    offer_id: String,
}

pub(crate) fn approval_lens_has_selection(data_dir: &Path) -> bool {
    // A failed policy read or migration keeps human review required.
    approval_lens_offer_id(data_dir)
        .map(|id| id.is_some())
        .unwrap_or(true)
}

fn store_approval_lens_selection(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    write_model_provider_config_atomic(
        &approval_lens_selection_path(data_dir),
        &serde_json::to_vec(&ApprovalLensSelection {
            offer_id: id.to_string(),
        })?,
    )
}

pub(super) fn select_approval_lens(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    anyhow::ensure!(
        decision_hosted_offer(data_dir, id).is_some(),
        "configured decision model required"
    );
    store_approval_lens_selection(data_dir, id)
}

/// Read policy identity separately from availability. First legacy selection is
/// migrated while holding the same lock as explicit selection and model edits.
pub(super) fn approval_lens_offer_id(data_dir: &Path) -> anyhow::Result<Option<String>> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let path = approval_lens_selection_path(data_dir);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            validate_model_provider_private_directory(
                &data_dir.join("providers"),
                "model-provider config parent",
            )?;
            validate_model_provider_private_directory(
                &model_provider_root_dir(data_dir),
                "model-provider config root",
            )?;
            let bytes = read_model_provider_private_file(
                &path,
                &metadata,
                4096,
                "Approval Lens selection",
            )?;
            let selection: ApprovalLensSelection = serde_json::from_slice(&bytes)?;
            anyhow::ensure!(
                !selection.offer_id.is_empty() && selection.offer_id.len() <= 256,
                "invalid evaluator selection"
            );
            Ok(Some(selection.offer_id))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let offers = load_model_provider_operator_offers(data_dir)?;
            let id = offers.iter().find_map(|offer| {
                let id = offer.get("id")?.as_str()?;
                (offer
                    .get("title")?
                    .as_str()?
                    .trim()
                    .eq_ignore_ascii_case("jev")
                    && decision_hosted_offer(data_dir, id).is_some())
                .then(|| id.to_string())
            });
            if let Some(id) = &id {
                store_approval_lens_selection(data_dir, id)?;
            }
            Ok(id)
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn named_jev_hosted_offer(data_dir: &Path) -> Option<(String, HostedModelOfferHint)> {
    let id = approval_lens_offer_id(data_dir).ok()??;
    Some((id.clone(), decision_hosted_offer(data_dir, &id)?))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AiProviderConnection {
    pub operation: String,
    pub id: String,
    pub name: String,
    pub provider: String,
    pub connected: bool,
    pub processor_label: String,
    pub processor_kind: String,
    pub selected_model: Option<String>,
    pub key_present: bool,
    pub share_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AiProviderStatus {
    pub storage_home: String,
    pub connections: Vec<AiProviderConnection>,
}

fn is_same_hosted_instance(offer: &serde_json::Value, offer_id: &str) -> bool {
    offer_id_of(offer) == Some(offer_id.trim())
}

fn hosted_offer(
    provider: HostedAiProvider,
    offer_id: &str,
    name: &str,
    model: &str,
    privacy: Option<&str>,
) -> serde_json::Value {
    let decision = provider == HostedAiProvider::OpenRouter && model.starts_with("typesafe/jev-");
    let mut hosted = serde_json::json!({
        "backend_provider_label": provider.label(),
        "selection_mode": "pinned",
        "privacy_policy_ref": provider.privacy_policy_ref(),
        "terms_ref": provider.terms_ref(),
        "upstream_routing_fallback_assertion": "operator_asserted_disabled"
    });
    if let Some(privacy) = privacy.map(str::trim).filter(|value| !value.is_empty()) {
        hosted["model_privacy"] = serde_json::Value::String(privacy.to_string());
    }
    serde_json::json!({
        "id": offer_id,
        "title": name,
        "operation": if decision { elastos_model_contract::decisions::OPERATION } else { "text.generate" },
        "input_modalities": [if decision { "application/json" } else { "text/plain" }],
        "output_modalities": [if decision { "application/json" } else { "text/plain" }],
        "enabled": true,
        "policy": {
            "concurrency_limit": 1,
            "input_bytes_limit": 32768,
            "inline_output_bytes_limit": 65536,
            "event_bytes_limit": 66560,
            "runtime_ms_limit": if decision { 7000 } else { 120000 },
            "retention_secs": 3600,
            "cancel_settlement_timeout_ms": 15000
        },
        "adapter": {
            "kind": if decision { "open_router_decisions" } else { HOSTED_ADAPTER_KIND },
            "api_url": if decision { OPENROUTER_DECISIONS_URL } else { provider.chat_url() },
            "model": model,
            "secret_ref": hosted_secret_ref(offer_id),
            "hosted": hosted
        }
    })
}

fn connection_from_offer(offer: &serde_json::Value) -> Option<AiProviderConnection> {
    let provider = hosted_provider_from_offer(offer)?;
    let id = offer_id_of(offer)?.to_string();
    let name = offer
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| provider.label())
        .to_string();
    let adapter = offer.get("adapter");
    let selected_model = adapter
        .and_then(|value| value.get("model"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let key_present = operator_offer_has_key(offer);
    let privacy = adapter
        .and_then(|value| value.get("hosted"))
        .and_then(|value| value.get("model_privacy"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let connected =
        offer.get("enabled").and_then(serde_json::Value::as_bool) != Some(false) && key_present;
    Some(AiProviderConnection {
        operation: offer
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text.generate")
            .to_string(),
        id,
        name,
        provider: provider.as_str().to_string(),
        connected,
        processor_label: provider.label().to_string(),
        processor_kind: "external".to_string(),
        selected_model,
        key_present,
        share_enabled: hosted_share_enabled(offer),
        privacy,
    })
}

fn status_from_offers(offers: &[serde_json::Value]) -> AiProviderStatus {
    AiProviderStatus {
        storage_home: "this Home".to_string(),
        connections: offers.iter().filter_map(connection_from_offer).collect(),
    }
}

fn write_model_provider_config_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("model-provider config has no parent"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("model-provider config has no file name"))?;
    let temp = parent.join(format!(
        ".{file_name}.{:016x}.tmp",
        rand::thread_rng().next_u64()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&temp)
            .with_context(|| format!("failed to stage model-provider config {}", temp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path).with_context(|| {
            format!("failed to replace model-provider config {}", path.display())
        })?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn strip_runtime_share_fields(mut offer: serde_json::Value) -> serde_json::Value {
    if let Some(object) = offer.as_object_mut() {
        object.remove("share");
    }
    offer
}

pub(super) fn operator_offer_has_key(offer: &serde_json::Value) -> bool {
    let inline = offer
        .pointer("/adapter/api_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let referenced = offer
        .pointer("/adapter/secret_ref")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    inline || referenced
}

fn operator_offer_enabled(offer: &serde_json::Value) -> bool {
    offer.get("enabled").and_then(serde_json::Value::as_bool) != Some(false)
}

fn listed_or_operator_is_hosted(offer: &serde_json::Value) -> bool {
    let hosted = offer
        .get("hosted")
        .or_else(|| offer.pointer("/adapter/hosted"));
    hosted.is_some_and(|value| !value.is_null())
}

fn hosted_share_enabled(offer: &serde_json::Value) -> bool {
    offer
        .get("share")
        .and_then(|share| share.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

pub(crate) fn offer_is_shareable(offer: &serde_json::Value) -> bool {
    if !listed_or_operator_is_hosted(offer) {
        return true;
    }
    offer.get("operation").and_then(serde_json::Value::as_str) == Some("text.generate")
        && hosted_share_enabled(offer)
        && operator_offer_enabled(offer)
        && (operator_offer_has_key(offer)
            || offer
                .get("key_present")
                .and_then(serde_json::Value::as_bool)
                == Some(true))
}

fn existing_hosted_share(
    offers: &[serde_json::Value],
    offer_id: &str,
) -> Option<serde_json::Value> {
    offers
        .iter()
        .find(|offer| is_same_hosted_instance(offer, offer_id))
        .and_then(|offer| offer.get("share"))
        .filter(|share| share.is_object())
        .cloned()
}

fn preserve_hosted_share(
    mut offer: serde_json::Value,
    provider: HostedAiProvider,
    model: &str,
    share: Option<serde_json::Value>,
) -> serde_json::Value {
    if offer.get("operation").and_then(serde_json::Value::as_str) != Some("text.generate") {
        return offer;
    }
    let Some(mut share) = share else {
        return offer;
    };
    if let Some(object) = share.as_object_mut() {
        object.insert(
            "processor".to_string(),
            serde_json::Value::String(provider.label().to_string()),
        );
        object.insert(
            "payer".to_string(),
            serde_json::Value::String(HOSTED_SHARE_PAYER.to_string()),
        );
        object.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    }
    offer["share"] = share;
    offer
}

fn hosted_secret_path(data_dir: &Path, offer_id: &str) -> anyhow::Result<PathBuf> {
    Ok(model_provider_secrets_dir(data_dir).join(hosted_secret_file_name(offer_id)?))
}

fn write_hosted_secret(data_dir: &Path, offer_id: &str, api_key: &str) -> anyhow::Result<()> {
    let dir = model_provider_secrets_dir(data_dir);
    crate::auth::create_owner_only_dir_all(data_dir, &dir)?;
    let path = hosted_secret_path(data_dir, offer_id)?;
    write_model_provider_config_atomic(&path, api_key.as_bytes())
}

pub(super) fn read_hosted_secret(
    data_dir: &Path,
    offer_id: &str,
) -> anyhow::Result<Option<String>> {
    let path = hosted_secret_path(data_dir, offer_id)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect hosted secret {}", path.display()))
        }
    };
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    validate_model_provider_private_directory(
        &model_provider_secrets_dir(data_dir),
        "model-provider secrets",
    )?;
    let bytes =
        read_model_provider_private_file(&path, &metadata, 8 * 1024, "hosted model secret")?;
    let secret = String::from_utf8(bytes).context("hosted model secret must be UTF-8")?;
    let secret = secret.trim();
    if secret.is_empty() {
        return Ok(None);
    }
    Ok(Some(secret.to_string()))
}

pub(super) fn read_hosted_egress_grants(data_dir: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let path = model_provider_root_dir(data_dir).join("egress-grants.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to inspect hosted egress grants"),
    };
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    read_model_provider_private_file(&path, &metadata, 1024 * 1024, "hosted egress grants")
        .map(Some)
}

#[cfg(target_os = "macos")]
pub(super) fn write_hosted_egress_grants(data_dir: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() <= 1024 * 1024,
        "hosted egress grants exceed limit"
    );
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    write_model_provider_config_atomic(
        &model_provider_root_dir(data_dir).join("egress-grants.json"),
        bytes,
    )
}

#[cfg(target_os = "macos")]
pub(super) fn read_hosted_egress_decisions(data_dir: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let path = model_provider_root_dir(data_dir).join("egress-decisions.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to inspect hosted egress decisions"),
    };
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    read_model_provider_private_file(&path, &metadata, 4 * 1024 * 1024, "hosted egress decisions")
        .map(Some)
}

#[cfg(target_os = "macos")]
pub(super) fn write_hosted_egress_decisions(data_dir: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "hosted egress decisions exceed limit"
    );
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    write_model_provider_config_atomic(
        &model_provider_root_dir(data_dir).join("egress-decisions.json"),
        bytes,
    )
}

#[cfg(target_os = "macos")]
fn hosted_egress_history_dir(data_dir: &Path) -> PathBuf {
    model_provider_root_dir(data_dir).join("egress-decision-history")
}

#[cfg(target_os = "macos")]
pub(super) fn archive_hosted_egress_decision(
    data_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        file_name.len() <= 96
            && file_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
            && file_name.ends_with(".json")
            && bytes.len() <= 64 * 1024,
        "invalid hosted egress history entry"
    );
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    let directory = hosted_egress_history_dir(data_dir);
    if !directory.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
            fs::File::open(model_provider_root_dir(data_dir))?.sync_all()?;
        }
    }
    validate_model_provider_private_directory(&directory, "hosted egress history")?;
    let path = directory.join(file_name);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        let previous = read_model_provider_private_file(
            &path,
            &metadata,
            64 * 1024,
            "hosted egress history entry",
        )?;
        anyhow::ensure!(
            serde_json::from_slice::<serde_json::Value>(&previous)?
                == serde_json::from_slice::<serde_json::Value>(bytes)?,
            "hosted egress history entry changed"
        );
        fs::File::open(&directory)?.sync_all()?;
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let name = std::ffi::CString::new(directory.as_os_str().as_bytes())?;
        let mut volume = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        anyhow::ensure!(
            unsafe { libc::statvfs(name.as_ptr(), volume.as_mut_ptr()) } == 0,
            "failed to inspect hosted egress history volume"
        );
        let volume = unsafe { volume.assume_init() };
        anyhow::ensure!(
            u128::from(volume.f_bavail) * 10 >= u128::from(volume.f_blocks),
            "hosted egress history disk reserve reached"
        );
    }
    let stage = directory.join(format!(
        ".{file_name}.{:016x}.tmp",
        rand::thread_rng().next_u64()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&stage)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&stage, &path)?;
        fs::File::open(&directory)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&stage);
    result
}

#[cfg(target_os = "macos")]
pub(super) fn recent_hosted_egress_history(
    data_dir: &Path,
    limit: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let directory = hosted_egress_history_dir(data_dir);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    validate_model_provider_private_directory(&directory, "hosted egress history")?;
    let mut names = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    names.retain(|name| {
        name.to_str()
            .is_some_and(|name| name.ends_with(".json") && !name.starts_with('.'))
    });
    names.sort();
    let mut records = Vec::new();
    for name in names.into_iter().rev().take(limit) {
        let path = directory.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        records.push(read_model_provider_private_file(
            &path,
            &metadata,
            64 * 1024,
            "hosted egress history entry",
        )?);
    }
    Ok(records)
}

pub(super) fn read_hosted_job_bindings(data_dir: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let path = model_provider_root_dir(data_dir).join("egress-job-bindings.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to inspect hosted job bindings"),
    };
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    read_model_provider_private_file(&path, &metadata, 4 * 1024 * 1024, "hosted job bindings")
        .map(Some)
}

pub(super) fn write_hosted_job_bindings(data_dir: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "hosted job bindings exceed limit"
    );
    validate_model_provider_private_directory(
        &data_dir.join("providers"),
        "model-provider config parent",
    )?;
    validate_model_provider_private_directory(
        &model_provider_root_dir(data_dir),
        "model-provider config root",
    )?;
    write_model_provider_config_atomic(
        &model_provider_root_dir(data_dir).join("egress-job-bindings.json"),
        bytes,
    )
}

/// Called only after System admin authorization. Blank edits retain the key of
/// this existing provider instance; this helper never returns a key to Home.
pub(crate) fn hosted_key_for_save(
    data_dir: &Path,
    provider: HostedAiProvider,
    instance_id: Option<&str>,
    supplied: &str,
) -> anyhow::Result<String> {
    if !supplied.trim().is_empty() {
        return Ok(supplied.trim().to_string());
    }
    let id = instance_id.ok_or_else(|| anyhow::anyhow!("hosted connection is required"))?;
    validate_hosted_instance_id(id)?;
    let offers = load_model_provider_operator_offers(data_dir)?;
    let existing = offers
        .iter()
        .find(|offer| is_same_hosted_instance(offer, id))
        .ok_or_else(|| anyhow::anyhow!("hosted connection is required"))?;
    anyhow::ensure!(
        hosted_provider_from_offer(existing) == Some(provider),
        "hosted instance provider does not match"
    );
    read_hosted_secret(data_dir, id)?
        .ok_or_else(|| anyhow::anyhow!("hosted connection is required"))
}

fn delete_hosted_secret(data_dir: &Path, offer_id: &str) -> anyhow::Result<()> {
    let path = hosted_secret_path(data_dir, offer_id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove hosted secret {}", path.display()))
        }
    }
}

fn take_adapter_api_key(offer: &mut serde_json::Value) -> Option<String> {
    let adapter = offer.get_mut("adapter")?.as_object_mut()?;
    let key = adapter
        .remove("api_key")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)?;
    Some(key)
}

fn store_operator_offer_secret(
    data_dir: &Path,
    mut offer: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if !is_hosted_instance_offer(&offer) {
        return Ok(offer);
    }
    let Some(id) = offer_id_of(&offer).map(ToOwned::to_owned) else {
        return Ok(offer);
    };
    if let Some(api_key) = take_adapter_api_key(&mut offer) {
        write_hosted_secret(data_dir, &id, &api_key)?;
    }
    if let Some(adapter) = offer
        .get_mut("adapter")
        .and_then(serde_json::Value::as_object_mut)
    {
        adapter.insert(
            "secret_ref".to_string(),
            serde_json::Value::String(hosted_secret_ref(&id)),
        );
        adapter.remove("api_key");
    }
    Ok(offer)
}

fn apply_fixture_chat_url(
    fixtures: Option<&HostedValidateFixtures>,
    offer: &mut serde_json::Value,
) {
    let Some(fixtures) = fixtures else {
        return;
    };
    let Some(provider) = hosted_provider_from_offer(offer) else {
        return;
    };
    let rewrite = match provider {
        HostedAiProvider::OpenRouter => fixtures.openrouter_chat_url.as_ref(),
        HostedAiProvider::Venice => fixtures.venice_chat_url.as_ref(),
    };
    let Some(url) = rewrite else {
        return;
    };
    if let Some(adapter) = offer
        .get_mut("adapter")
        .and_then(serde_json::Value::as_object_mut)
    {
        adapter.insert(
            "api_url".to_string(),
            serde_json::Value::String(url.as_str().to_string()),
        );
    }
}

fn materialize_provider_offer(
    mut offer: serde_json::Value,
    fixtures: Option<&HostedValidateFixtures>,
) -> serde_json::Value {
    if let Some(adapter) = offer
        .get_mut("adapter")
        .and_then(serde_json::Value::as_object_mut)
    {
        adapter.remove("secret_ref");
        // Runtime keeps hosted credentials; the native provider receives only
        // a named offer and requests each external effect through Runtime.
        adapter.remove("api_key");
        adapter.remove("bearer_token");
    }
    apply_fixture_chat_url(fixtures, &mut offer);
    offer
}

fn offers_need_secret_migration(offers: &[serde_json::Value]) -> bool {
    offers.iter().any(|offer| {
        offer
            .pointer("/adapter/api_key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    })
}

fn normalize_display_name(value: &str, fallback: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    let name = if trimmed.is_empty() {
        fallback.trim()
    } else {
        trimmed
    };
    if name.is_empty()
        || name.len() > HOSTED_DISPLAY_NAME_MAX_BYTES
        || name.contains('\n')
        || name.contains('\r')
    {
        anyhow::bail!("invalid hosted instance name");
    }
    Ok(name.to_string())
}

fn validate_hosted_instance_id(offer_id: &str) -> anyhow::Result<()> {
    let offer_id = offer_id.trim();
    if HostedAiProvider::from_offer_id(offer_id).is_some() {
        return Ok(());
    }
    let Some(hex) = offer_id.strip_prefix("model:hosted-") else {
        anyhow::bail!("invalid hosted instance id");
    };
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid hosted instance id");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedModelShareCard {
    pub offer_id: String,
    pub name: String,
    pub processor: String,
    pub model: String,
    pub payer: String,
    pub terms_ack: String,
    pub terms_summary: String,
    pub share_enabled: bool,
}

pub(crate) fn hosted_model_share_cards(
    data_dir: &Path,
) -> anyhow::Result<Vec<HostedModelShareCard>> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let offers = load_model_provider_operator_offers(data_dir)?;
    Ok(offers
        .iter()
        .filter_map(|offer| {
            let connection = connection_from_offer(offer)?;
            if !connection.connected || connection.operation != "text.generate" {
                return None;
            }
            let provider = hosted_provider_from_offer(offer)?;
            let model = connection.selected_model.clone()?;
            Some(HostedModelShareCard {
                offer_id: connection.id,
                name: connection.name,
                processor: provider.label().to_string(),
                model,
                payer: provider.share_payer().to_string(),
                terms_ack: provider.share_terms_ack().to_string(),
                terms_summary: provider.share_terms_summary().to_string(),
                share_enabled: hosted_share_enabled(offer),
            })
        })
        .collect())
}

pub(crate) fn any_hosted_model_shared(data_dir: &Path) -> bool {
    hosted_model_share_cards(data_dir)
        .ok()
        .is_some_and(|cards| cards.iter().any(|card| card.share_enabled))
}

pub(crate) fn operator_has_hosted_offer(data_dir: &Path, offer_id: &str) -> bool {
    load_model_provider_operator_offers(data_dir)
        .ok()
        .is_some_and(|offers| {
            offers.iter().any(|offer| {
                is_same_hosted_instance(offer, offer_id) && is_hosted_instance_offer(offer)
            })
        })
}

pub(crate) fn set_hosted_offer_share(
    data_dir: &Path,
    offer_id: &str,
    enabled: bool,
    terms_ack: Option<&str>,
) -> anyhow::Result<AiProviderStatus> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    validate_hosted_instance_id(offer_id)?;
    let mut offers = load_model_provider_operator_offers(data_dir)?;
    let Some(index) = offers
        .iter()
        .position(|offer| is_same_hosted_instance(offer, offer_id))
    else {
        anyhow::bail!("hosted connection is required");
    };
    let connection = connection_from_offer(&offers[index])
        .ok_or_else(|| anyhow::anyhow!("hosted connection is required"))?;
    let provider = hosted_provider_from_offer(&offers[index])
        .ok_or_else(|| anyhow::anyhow!("hosted connection is required"))?;
    if !connection.connected {
        anyhow::bail!("hosted connection is required");
    }
    if enabled {
        anyhow::ensure!(
            connection.operation == "text.generate",
            "only text model connections can be shared as Assistant services"
        );
        let ack = terms_ack.map(str::trim).unwrap_or_default();
        if ack.is_empty() {
            anyhow::bail!("share terms acknowledgment is required");
        }
        if ack != provider.share_terms_ack()
            || ack.len() > HOSTED_SHARE_TERMS_ACK_MAX_BYTES
            || provider.label().len() > HOSTED_SHARE_PROCESSOR_MAX_BYTES
        {
            anyhow::bail!("invalid share terms acknowledgment");
        }
        let model = connection.selected_model.clone().unwrap_or_default();
        if model.is_empty() || model.len() > HOSTED_SHARE_MODEL_MAX_BYTES {
            anyhow::bail!("hosted connection is required");
        }
        offers[index]["share"] = serde_json::json!({
            "enabled": true,
            "terms_ack": ack,
            "processor": provider.label(),
            "payer": HOSTED_SHARE_PAYER,
            "model": model
        });
    } else if let Some(object) = offers[index].as_object_mut() {
        object.remove("share");
    }
    persist_model_provider_operator_offers(data_dir, offers)?;
    Ok(status_from_offers(&load_model_provider_operator_offers(
        data_dir,
    )?))
}

fn persist_model_provider_operator_offers(
    data_dir: &Path,
    offers: Vec<serde_json::Value>,
) -> anyhow::Result<()> {
    let root = model_provider_root_dir(data_dir);
    crate::auth::create_owner_only_dir_all(data_dir, &root)?;
    let mut stored = Vec::with_capacity(offers.len());
    for offer in offers {
        stored.push(store_operator_offer_secret(data_dir, offer)?);
    }
    let path = model_provider_config_path(data_dir);
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({ "offers": stored }))
        .context("failed to encode model-provider operator config")?;
    if bytes.len() > MODEL_PROVIDER_CONFIG_MAX_BYTES {
        anyhow::bail!("model-provider operator config exceeds its byte limit");
    }
    write_model_provider_config_atomic(&path, &bytes)
}

async fn refresh_registered_model_provider(
    data_dir: &Path,
    registry: Option<&provider::ProviderRegistry>,
) -> anyhow::Result<()> {
    let Some(registry) = registry else {
        return Ok(());
    };
    let (config, _guard) = model_provider_config(data_dir, registry).await?;
    #[cfg(target_os = "macos")]
    let mut config = config;
    #[cfg(target_os = "macos")]
    registry.apply_local_model_sockets(&mut config).await?;
    registry
        .refresh_local_model_configuration(&config)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(())
}

pub(crate) fn ai_provider_status(data_dir: &Path) -> anyhow::Result<AiProviderStatus> {
    let _guard = MODEL_PROVIDER_CONFIG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let loaded = load_model_provider_operator_offers(data_dir)?;
    if offers_need_secret_migration(&loaded) {
        persist_model_provider_operator_offers(data_dir, loaded)?;
    }
    Ok(status_from_offers(&load_model_provider_operator_offers(
        data_dir,
    )?))
}

pub(crate) struct HostedOfferSave<'a> {
    pub provider: HostedAiProvider,
    pub api_key: &'a str,
    pub model: &'a str,
    pub expected_response_model: Option<&'a str>,
    pub privacy: Option<&'a str>,
    pub name: &'a str,
    pub instance_id: Option<&'a str>,
}

pub(crate) async fn save_hosted_offer(
    data_dir: &Path,
    registry: Option<&provider::ProviderRegistry>,
    request: HostedOfferSave<'_>,
) -> anyhow::Result<AiProviderStatus> {
    let HostedOfferSave {
        provider,
        api_key,
        model,
        expected_response_model,
        privacy,
        name,
        instance_id,
    } = request;
    {
        let _guard = MODEL_PROVIDER_CONFIG_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = load_model_provider_operator_offers(data_dir)?;
        let name = normalize_display_name(name, provider.label())?;
        let offer_id = match instance_id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(id) => {
                validate_hosted_instance_id(id)?;
                if let Some(existing) = current
                    .iter()
                    .find(|offer| is_same_hosted_instance(offer, id))
                {
                    let existing_provider = hosted_provider_from_offer(existing)
                        .ok_or_else(|| anyhow::anyhow!("hosted connection is required"))?;
                    if existing_provider != provider {
                        anyhow::bail!("hosted instance provider does not match");
                    }
                }
                id.to_string()
            }
            None => new_hosted_instance_id(),
        };
        write_hosted_secret(data_dir, &offer_id, api_key)?;
        let share = existing_hosted_share(&current, &offer_id);
        let mut replacement = preserve_hosted_share(
            hosted_offer(provider, &offer_id, &name, model, privacy),
            provider,
            model,
            share,
        );
        if replacement["operation"] == elastos_model_contract::decisions::OPERATION {
            if let Some(expected) = expected_response_model {
                replacement["adapter"]["expected_response_model"] = serde_json::json!(expected);
            }
        }
        let mut offers = current;
        if let Some(index) = offers
            .iter()
            .position(|offer| is_same_hosted_instance(offer, &offer_id))
        {
            offers[index] = replacement;
        } else {
            offers.push(replacement);
        }
        persist_model_provider_operator_offers(data_dir, offers)?;
    }
    refresh_registered_model_provider(data_dir, registry)
        .await
        .map_err(|_| anyhow::anyhow!("model activation pending"))?;
    ai_provider_status(data_dir)
}

pub(crate) async fn remove_hosted_offer(
    data_dir: &Path,
    registry: Option<&provider::ProviderRegistry>,
    offer_id: &str,
) -> anyhow::Result<AiProviderStatus> {
    {
        let _guard = MODEL_PROVIDER_CONFIG_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        validate_hosted_instance_id(offer_id)?;
        let offers = load_model_provider_operator_offers(data_dir)?
            .into_iter()
            .filter(|offer| !is_same_hosted_instance(offer, offer_id))
            .collect::<Vec<_>>();
        persist_model_provider_operator_offers(data_dir, offers)?;
        delete_hosted_secret(data_dir, offer_id)?;
    }
    refresh_registered_model_provider(data_dir, registry)
        .await
        .map_err(|_| anyhow::anyhow!("model retirement pending"))?;
    ai_provider_status(data_dir)
}

#[cfg(test)]
pub(crate) fn seed_model_provider_operator_offers_for_test(
    data_dir: &Path,
    offers: Vec<serde_json::Value>,
) -> anyhow::Result<()> {
    persist_model_provider_operator_offers(data_dir, offers)
}

fn validate_model_provider_private_directory(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("{label} must be a real directory");
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.uid() != unsafe { libc::geteuid() } || mode != 0o700 {
            anyhow::bail!("{label} must be owned by the current user with mode 0700");
        }
    }
    Ok(())
}

fn validate_model_provider_private_file(
    path: &Path,
    metadata: &fs::Metadata,
    label: &str,
) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("{label} must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.uid() != unsafe { libc::geteuid() } || mode != 0o600 {
            anyhow::bail!("{label} must be owned by the current user with mode 0600");
        }
    }
    let _ = path;
    Ok(())
}

fn read_model_provider_private_file(
    path: &Path,
    metadata: &fs::Metadata,
    max_bytes: usize,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    validate_model_provider_private_file(path, metadata, label)?;
    let metadata_len = usize::try_from(metadata.len())
        .context("model-provider operator config length does not fit memory bounds")?;
    if metadata_len > max_bytes {
        anyhow::bail!("{label} exceeds its byte limit");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {label} {}", path.display()))?;
    let opened_metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect opened {label} {}", path.display()))?;
    validate_model_provider_private_file(path, &opened_metadata, label)?;
    let mut bytes = Vec::with_capacity(metadata_len);
    let read_limit = u64::try_from(max_bytes)?
        .checked_add(1)
        .context("model-provider operator config read bound overflow")?;
    file.take(read_limit).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        anyhow::bail!("{label} exceeds its byte limit");
    }
    Ok(bytes)
}

#[cfg(test)]
mod hosted_hint_tests {
    use super::{
        hosted_hint_from_offers, hosted_model_share_cards, hosted_offer, named_jev_hosted_offer,
        seed_model_provider_operator_offers_for_test, set_hosted_offer_share, status_from_offers,
        HostedAiProvider,
    };
    use serde_json::json;

    #[test]
    fn hosted_hint_omits_urls_and_keys() {
        let offers = vec![json!({
            "id": "offer-hosted",
            "adapter": {
                "kind": "open_ai_compatible_text",
                "api_url": "https://example.invalid/v1/chat/completions",
                "api_key": "secret-should-not-leak",
                "model": "gpt-test",
                "hosted": {
                    "backend_provider_label": "Fixture Provider",
                    "privacy_policy_ref": "fixture:privacy:v1",
                    "upstream_routing_fallback_assertion": "operator_asserted_disabled"
                }
            }
        })];
        let hint = hosted_hint_from_offers(&offers, "offer-hosted").unwrap();
        assert_eq!(hint.provider_label, "Fixture Provider");
        assert_eq!(hint.requested_selector, "gpt-test");
        assert_eq!(hint.privacy_policy_ref, "fixture:privacy:v1");
        assert_eq!(hint.fallback, "operator_asserted_disabled");
        let encoded = format!("{hint:?}");
        assert!(!encoded.contains("https://"));
        assert!(!encoded.contains("secret-should-not-leak"));
        let alias = vec![json!({
            "id": "offer-hosted-alias",
            "adapter": {
                "kind": "openai_compatible_text",
                "model": "gpt-test",
                "hosted": {
                    "backend_provider_label": "Fixture Provider",
                    "privacy_policy_ref": "fixture:privacy:v1",
                    "upstream_routing_fallback_assertion": "operator_asserted_disabled"
                }
            }
        })];
        assert_eq!(
            hosted_hint_from_offers(&alias, "offer-hosted-alias")
                .unwrap()
                .provider_label,
            "Fixture Provider"
        );
        let status = status_from_offers(&[
            hosted_offer(
                HostedAiProvider::OpenRouter,
                "model:openrouter",
                "OpenRouter",
                "fixture/model",
                None,
            ),
            hosted_offer(
                HostedAiProvider::Venice,
                "model:venice",
                "Venice",
                "fixture/model",
                Some("private"),
            ),
        ]);
        assert_eq!(status.storage_home, "this Home");
        assert_eq!(status.connections.len(), 2);
        assert_eq!(status.connections[0].id, "model:openrouter");
        assert_eq!(status.connections[0].name, "OpenRouter");
        assert_eq!(status.connections[0].provider, "openrouter");
        assert!(status.connections[0].connected);
        assert_eq!(
            status.connections[0].selected_model.as_deref(),
            Some("fixture/model")
        );
        assert!(status.connections[0].key_present);
        assert_eq!(status.connections[0].privacy, None);
        assert_eq!(status.connections[1].provider, "venice");
        assert!(status.connections[1].connected);
        assert_eq!(status.connections[1].privacy.as_deref(), Some("private"));
        let status_json = serde_json::to_string(&status).unwrap();
        let status_debug = format!("{status:?}");
        assert!(!status_json.contains("sk-or-fixture-valid"));
        assert!(!status_json.contains("sk-vnz-fixture-valid"));
        assert!(!status_json.contains("openrouter.ai"));
        assert!(!status_json.contains("venice.ai"));
        assert!(!status_json.contains("api_key"));
        assert!(!status_json.contains("api_url"));
        assert!(!status_debug.contains("sk-or-fixture-valid"));
        assert!(!status_debug.contains("openrouter.ai"));
    }

    #[test]
    fn named_jev_requires_a_decision_model_and_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut offer = hosted_offer(
            HostedAiProvider::OpenRouter,
            "model:openrouter",
            "Jev",
            "typesafe/jev-1.13",
            None,
        );
        offer["adapter"]["api_key"] = serde_json::json!("fixture-secret");
        seed_model_provider_operator_offers_for_test(dir.path(), vec![offer.clone()]).unwrap();
        assert!(named_jev_hosted_offer(dir.path()).is_some());
        assert!(hosted_model_share_cards(dir.path()).unwrap().is_empty());
        assert!(set_hosted_offer_share(
            dir.path(),
            "model:openrouter",
            true,
            Some(HostedAiProvider::OpenRouter.share_terms_ack())
        )
        .is_err());
        offer["adapter"]["model"] = serde_json::json!("other/model");
        seed_model_provider_operator_offers_for_test(dir.path(), vec![offer]).unwrap();
        assert!(named_jev_hosted_offer(dir.path()).is_none());
    }

    #[test]
    fn approval_lens_selection_survives_rename_and_never_substitutes() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = hosted_offer(
            HostedAiProvider::OpenRouter,
            "model:first",
            "Jev",
            "typesafe/jev-1.13",
            None,
        );
        first["adapter"]["api_key"] = json!("fixture-secret");
        let mut second = first.clone();
        second["id"] = json!("model:second");
        seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![first.clone(), second.clone()],
        )
        .unwrap();
        assert_eq!(named_jev_hosted_offer(dir.path()).unwrap().0, "model:first");
        first["title"] = json!("My reviewer");
        seed_model_provider_operator_offers_for_test(dir.path(), vec![first, second.clone()])
            .unwrap();
        assert_eq!(named_jev_hosted_offer(dir.path()).unwrap().0, "model:first");
        seed_model_provider_operator_offers_for_test(dir.path(), vec![second.clone()]).unwrap();
        assert!(named_jev_hosted_offer(dir.path()).is_none());
        assert!(super::approval_lens_has_selection(dir.path()));
        super::select_approval_lens(dir.path(), "model:second").unwrap();
        assert_eq!(
            named_jev_hosted_offer(dir.path()).unwrap().0,
            "model:second"
        );
        second["enabled"] = json!(false);
        seed_model_provider_operator_offers_for_test(dir.path(), vec![second]).unwrap();
        assert!(super::select_approval_lens(dir.path(), "model:second").is_err());
        assert!(super::select_approval_lens(dir.path(), "model:foreign").is_err());
    }

    #[test]
    fn approval_lens_policy_read_error_keeps_review_required() {
        let dir = tempfile::tempdir().unwrap();
        let mut offer = hosted_offer(
            HostedAiProvider::OpenRouter,
            "model:first",
            "Jev",
            "typesafe/jev-1.13",
            None,
        );
        offer["adapter"]["api_key"] = json!("fixture-secret");
        seed_model_provider_operator_offers_for_test(dir.path(), vec![offer]).unwrap();
        std::fs::create_dir(super::approval_lens_selection_path(dir.path())).unwrap();
        assert!(super::approval_lens_offer_id(dir.path()).is_err());
        assert!(named_jev_hosted_offer(dir.path()).is_none());
        assert!(super::approval_lens_has_selection(dir.path()));
    }

    #[test]
    fn local_llama_adapter_is_not_hosted() {
        let offers = vec![json!({
            "id": "local",
            "adapter": { "kind": "local_llama_cpp_text" }
        })];
        assert!(hosted_hint_from_offers(&offers, "local").is_none());
    }

    #[test]
    fn offer_is_shareable_keeps_local_and_explicit_hosted_share() {
        assert!(super::offer_is_shareable(&json!({ "id": "qwen" })));
        assert!(super::offer_is_shareable(
            &json!({ "id": "qwen", "hosted": null })
        ));
        assert!(!super::offer_is_shareable(&json!({
            "id": "gpt",
            "hosted": { "placement": "hosted" }
        })));
        assert!(super::offer_is_shareable(&json!({
            "id": "model:openrouter",
            "hosted": { "placement": "hosted" },
            "operation": "text.generate",
            "share": { "enabled": true },
            "key_present": true
        })));
        assert!(super::offer_is_shareable(&json!({
            "id": "model:hosted-0123456789abcdef0123456789abcdef",
            "hosted": { "placement": "hosted" },
            "operation": "text.generate",
            "share": { "enabled": true },
            "key_present": true
        })));
        assert!(!super::offer_is_shareable(&json!({
            "id": "model:venice",
            "hosted": { "placement": "hosted" },
            "operation": "text.generate",
            "share": { "enabled": false },
            "key_present": true
        })));
        assert!(!super::offer_is_shareable(&json!({
            "id": "model:openrouter",
            "hosted": { "placement": "hosted" },
            "operation": "text.generate",
            "share": { "enabled": true },
            "enabled": false,
            "key_present": true
        })));
    }

    #[test]
    fn preserve_hosted_share_keeps_enabled_and_updates_model() {
        let preserved = super::preserve_hosted_share(
            json!({ "id": "model:openrouter", "operation": "text.generate" }),
            HostedAiProvider::OpenRouter,
            "fixture/replaced",
            Some(json!({
                "enabled": true,
                "terms_ack": "openrouter-5.1-5.2+model",
                "processor": "OpenRouter",
                "payer": "this Home",
                "model": "fixture/model"
            })),
        );
        assert_eq!(preserved["share"]["enabled"], true);
        assert_eq!(preserved["share"]["terms_ack"], "openrouter-5.1-5.2+model");
        assert_eq!(preserved["share"]["model"], "fixture/replaced");
        assert_eq!(preserved["share"]["processor"], "OpenRouter");
        assert_eq!(preserved["share"]["payer"], "this Home");
    }
    #[tokio::test]
    async fn editing_jev_preserves_evaluator_selection_and_other_instances() {
        let dir = tempfile::tempdir().unwrap();
        let first = "model:hosted-0123456789abcdef0123456789abcdef";
        let second = "model:hosted-1123456789abcdef0123456789abcdef";
        for (id, canonical) in [
            (first, None),
            (second, None),
            (first, Some("typesafe/jev-1.13-20260917")),
        ] {
            super::save_hosted_offer(
                dir.path(),
                None,
                super::HostedOfferSave {
                    provider: HostedAiProvider::OpenRouter,
                    api_key: "fixture-key",
                    model: "typesafe/jev-1.13",
                    expected_response_model: canonical,
                    privacy: None,
                    name: "Jev",
                    instance_id: Some(id),
                },
            )
            .await
            .unwrap();
        }
        let offers = super::load_model_provider_operator_offers(dir.path()).unwrap();
        assert_eq!(offers.len(), 2);
        assert_eq!(offers[0]["id"], first);
        assert_eq!(offers[1]["id"], second);
        assert!(offers[1]["adapter"]
            .get("expected_response_model")
            .is_none());
        assert_eq!(super::named_jev_hosted_offer(dir.path()).unwrap().0, first);
        assert_eq!(
            offers[0]["adapter"]["expected_response_model"],
            "typesafe/jev-1.13-20260917"
        );
    }

    #[tokio::test]
    async fn replacing_shared_text_with_jev_clears_share() {
        let dir = tempfile::tempdir().unwrap();
        let id = "model:hosted-0123456789abcdef0123456789abcdef";
        super::save_hosted_offer(
            dir.path(),
            None,
            crate::api::model_provider_config::HostedOfferSave {
                provider: HostedAiProvider::OpenRouter,
                api_key: "fixture-key",
                model: "fixture/text",
                expected_response_model: None,
                privacy: None,
                name: "Chat",
                instance_id: Some(id),
            },
        )
        .await
        .unwrap();
        let mut offers = super::load_model_provider_operator_offers(dir.path()).unwrap();
        offers[0]["share"] = json!({"enabled": true});
        super::persist_model_provider_operator_offers(dir.path(), offers).unwrap();
        super::save_hosted_offer(
            dir.path(),
            None,
            crate::api::model_provider_config::HostedOfferSave {
                provider: HostedAiProvider::OpenRouter,
                api_key: "fixture-key",
                model: "typesafe/jev-1.13",
                expected_response_model: Some("typesafe/jev-1.13-20260917"),
                privacy: None,
                name: "Jev",
                instance_id: Some(id),
            },
        )
        .await
        .unwrap();
        let mut offers = super::load_model_provider_operator_offers(dir.path()).unwrap();
        assert!(!super::hosted_share_enabled(&offers[0]));
        assert_eq!(
            offers[0]["adapter"]["expected_response_model"],
            "typesafe/jev-1.13-20260917"
        );
        assert_eq!(
            super::hosted_key_for_save(dir.path(), HostedAiProvider::OpenRouter, Some(id), "")
                .unwrap(),
            "fixture-key"
        );
        assert!(
            super::hosted_key_for_save(dir.path(), HostedAiProvider::Venice, Some(id), "").is_err()
        );
        assert!(
            super::hosted_key_for_save(dir.path(), HostedAiProvider::OpenRouter, None, "").is_err()
        );
        // Even a stale stored share cannot expose a decision offer.
        offers[0]["share"] = json!({"enabled": true});
        assert!(!super::offer_is_shareable(&offers[0]));
    }
}

#[cfg(test)]
mod validate_fixture_tests {
    use super::{load_hosted_validate_fixtures, parse_loopback_http_url};

    fn write_validate_fixtures(data_dir: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        let providers = data_dir.join("providers");
        let root = providers.join("model-provider");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&providers, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("validate-fixtures.json");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn fixtures_json(openrouter: &str, venice_rate: &str, venice_models: &str) -> String {
        serde_json::json!({
            "openrouter_models_url": openrouter,
            "venice_rate_limits_url": venice_rate,
            "venice_models_url": venice_models
        })
        .to_string()
    }

    #[test]
    fn parse_loopback_http_url_accepts_explicit_127() {
        let url =
            parse_loopback_http_url("http://127.0.0.1:43721/openrouter/api/v1/models").unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:43721/openrouter/api/v1/models"
        );
        let venice =
            parse_loopback_http_url("http://127.0.0.1:43721/venice/api/v1/models?type=text")
                .unwrap();
        assert_eq!(
            venice.as_str(),
            "http://127.0.0.1:43721/venice/api/v1/models?type=text"
        );
        let tls = parse_loopback_http_url("https://127.0.0.1:43721/models").unwrap();
        assert_eq!(tls.as_str(), "https://127.0.0.1:43721/models");
    }

    #[test]
    fn parse_loopback_http_url_rejects_public_https() {
        assert!(parse_loopback_http_url("https://openrouter.ai/api/v1/models").is_err());
        assert!(
            parse_loopback_http_url("https://api.venice.ai/api/v1/api_keys/rate_limits").is_err()
        );
    }

    #[test]
    fn parse_loopback_http_url_rejects_non_loopback_ip() {
        assert!(parse_loopback_http_url("http://8.8.8.8:80/openrouter/api/v1/models").is_err());
        assert!(parse_loopback_http_url("http://127.0.0.2:43721/models").is_err());
        assert!(parse_loopback_http_url("https://127.0.0.2:43721/models").is_err());
        assert!(
            parse_loopback_http_url("http://localhost:43721/openrouter/api/v1/models").is_err()
        );
        assert!(parse_loopback_http_url("http://[::1]:43721/openrouter/api/v1/models").is_err());
        assert!(parse_loopback_http_url(
            "http://user:pass@127.0.0.1:43721/openrouter/api/v1/models"
        )
        .is_err());
        assert!(parse_loopback_http_url("http://127.0.0.1/openrouter/api/v1/models").is_err());
    }

    #[test]
    fn load_hosted_validate_fixtures_absent_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_hosted_validate_fixtures(dir.path()).unwrap().is_none());
    }

    #[test]
    fn load_hosted_validate_fixtures_accepts_loopback_http() {
        let dir = tempfile::tempdir().unwrap();
        write_validate_fixtures(
            dir.path(),
            &fixtures_json(
                "http://127.0.0.1:43721/openrouter/api/v1/models",
                "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "http://127.0.0.1:43721/venice/api/v1/models?type=text",
            ),
        );
        let loaded = load_hosted_validate_fixtures(dir.path())
            .unwrap()
            .expect("fixtures present");
        assert_eq!(
            loaded.openrouter_models_url.as_str(),
            "http://127.0.0.1:43721/openrouter/api/v1/models"
        );
        assert_eq!(
            loaded.venice_rate_limits_url.as_str(),
            "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits"
        );
        assert_eq!(
            loaded.venice_models_url.as_str(),
            "http://127.0.0.1:43721/venice/api/v1/models?type=text"
        );
        assert!(loaded.openrouter_chat_url.is_none());
        assert!(loaded.venice_chat_url.is_none());
    }

    #[test]
    fn load_hosted_validate_fixtures_accepts_optional_loopback_chat_urls() {
        let dir = tempfile::tempdir().unwrap();
        write_validate_fixtures(
            dir.path(),
            &serde_json::json!({
                "openrouter_models_url": "http://127.0.0.1:43721/openrouter/api/v1/models",
                "venice_rate_limits_url": "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "venice_models_url": "http://127.0.0.1:43721/venice/api/v1/models?type=text",
                "openrouter_chat_url": "http://127.0.0.1:43721/openrouter/api/v1/chat/completions",
                "venice_chat_url": "http://127.0.0.1:43721/venice/api/v1/chat/completions"
            })
            .to_string(),
        );
        let loaded = load_hosted_validate_fixtures(dir.path())
            .unwrap()
            .expect("fixtures present");
        assert_eq!(
            loaded.openrouter_chat_url.as_ref().map(|url| url.as_str()),
            Some("http://127.0.0.1:43721/openrouter/api/v1/chat/completions")
        );
        assert_eq!(
            loaded.venice_chat_url.as_ref().map(|url| url.as_str()),
            Some("http://127.0.0.1:43721/venice/api/v1/chat/completions")
        );
    }

    #[test]
    fn load_hosted_validate_fixtures_rejects_public_chat_url() {
        let dir = tempfile::tempdir().unwrap();
        write_validate_fixtures(
            dir.path(),
            &serde_json::json!({
                "openrouter_models_url": "http://127.0.0.1:43721/openrouter/api/v1/models",
                "venice_rate_limits_url": "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "venice_models_url": "http://127.0.0.1:43721/venice/api/v1/models?type=text",
                "openrouter_chat_url": "https://openrouter.ai/api/v1/chat/completions"
            })
            .to_string(),
        );
        assert!(load_hosted_validate_fixtures(dir.path()).is_err());
    }

    #[test]
    fn bridge_config_applies_loopback_chat_urls_without_rewriting_operator_config() {
        let dir = tempfile::tempdir().unwrap();
        super::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![
                serde_json::json!({
                    "id": "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "title": "Jev",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "enabled": true,
                    "adapter": {
                        "kind": "open_ai_compatible_text",
                        "api_url": "https://openrouter.ai/api/v1/chat/completions",
                        "api_key": "sk-or-fixture-valid",
                        "model": "fixture/model",
                        "hosted": {
                            "backend_provider_label": "OpenRouter",
                            "selection_mode": "pinned",
                            "privacy_policy_ref": "fixture:privacy:v1",
                            "terms_ref": "fixture:terms:v1",
                            "upstream_routing_fallback_assertion": "operator_asserted_disabled"
                        }
                    }
                }),
                serde_json::json!({
                    "id": "model:hosted-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "title": "Venice",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "enabled": true,
                    "adapter": {
                        "kind": "open_ai_compatible_text",
                        "api_url": "https://api.venice.ai/api/v1/chat/completions",
                        "api_key": "sk-vnz-fixture-valid",
                        "model": "fixture/model",
                        "hosted": {
                            "backend_provider_label": "Venice",
                            "selection_mode": "pinned",
                            "privacy_policy_ref": "fixture:privacy:v1",
                            "terms_ref": "fixture:terms:v1",
                            "upstream_routing_fallback_assertion": "operator_asserted_disabled"
                        }
                    }
                }),
            ],
        )
        .unwrap();
        write_validate_fixtures(
            dir.path(),
            &serde_json::json!({
                "openrouter_models_url": "http://127.0.0.1:43721/openrouter/api/v1/models",
                "venice_rate_limits_url": "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "venice_models_url": "http://127.0.0.1:43721/venice/api/v1/models?type=text",
                "openrouter_chat_url": "http://127.0.0.1:43721/openrouter/api/v1/chat/completions",
                "venice_chat_url": "http://127.0.0.1:43721/venice/api/v1/chat/completions"
            })
            .to_string(),
        );
        let config = super::model_provider_bridge_config(dir.path()).unwrap();
        let offers = config.extra["offers"].as_array().expect("offers");
        let jev = offers
            .iter()
            .find(|offer| offer["title"] == "Jev")
            .expect("Jev");
        let venice = offers
            .iter()
            .find(|offer| offer["title"] == "Venice")
            .expect("Venice");
        assert_eq!(
            jev["adapter"]["api_url"],
            "http://127.0.0.1:43721/openrouter/api/v1/chat/completions"
        );
        assert_eq!(
            venice["adapter"]["api_url"],
            "http://127.0.0.1:43721/venice/api/v1/chat/completions"
        );
        assert!(jev["adapter"].get("api_key").is_none());
        assert!(venice["adapter"].get("api_key").is_none());
        assert!(jev["adapter"].get("secret_ref").is_none());
        assert!(venice["adapter"].get("secret_ref").is_none());
        assert!(super::read_hosted_secret(
            dir.path(),
            "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        .unwrap()
        .is_some());
        assert!(super::read_hosted_secret(
            dir.path(),
            "model:hosted-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        )
        .unwrap()
        .is_some());
        let disk = std::fs::read_to_string(
            dir.path()
                .join("providers")
                .join("model-provider")
                .join("config.json"),
        )
        .unwrap();
        assert!(disk.contains("https://openrouter.ai/api/v1/chat/completions"));
        assert!(disk.contains("https://api.venice.ai/api/v1/chat/completions"));
        assert!(!disk.contains("127.0.0.1:43721"));
        assert!(!disk.contains("sk-or-fixture-valid"));
        assert!(!disk.contains("sk-vnz-fixture-valid"));
    }

    #[test]
    fn load_hosted_validate_fixtures_rejects_public_https() {
        let dir = tempfile::tempdir().unwrap();
        write_validate_fixtures(
            dir.path(),
            &fixtures_json(
                "https://openrouter.ai/api/v1/models",
                "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "http://127.0.0.1:43721/venice/api/v1/models?type=text",
            ),
        );
        assert!(load_hosted_validate_fixtures(dir.path()).is_err());
    }

    #[test]
    fn load_hosted_validate_fixtures_rejects_non_loopback_ip() {
        let dir = tempfile::tempdir().unwrap();
        write_validate_fixtures(
            dir.path(),
            &fixtures_json(
                "http://8.8.8.8:80/openrouter/api/v1/models",
                "http://127.0.0.1:43721/venice/api/v1/api_keys/rate_limits",
                "http://127.0.0.1:43721/venice/api/v1/models?type=text",
            ),
        );
        assert!(load_hosted_validate_fixtures(dir.path()).is_err());
    }
}
