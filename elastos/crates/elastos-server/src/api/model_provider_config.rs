//! Runtime-owned model configuration shared by startup and admitted-content activation.
use anyhow::Context as _;
use elastos_runtime::provider;
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read as _};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const MODEL_PROVIDER_ID: &str = "model-provider";
const MODEL_PROVIDER_CONFIG_FILE_NAME: &str = "config.json";
const MODEL_PROVIDER_CONFIG_MAX_BYTES: usize = 256 * 1024;

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
    let offers = load_model_provider_operator_offers(data_dir)?;
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

fn load_model_provider_operator_offers(data_dir: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
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
    if kind != "openai_compatible_text" && kind != "openai_responses_text" {
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
    Some(HostedModelOfferHint {
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
    use super::hosted_hint_from_offers;
    use serde_json::json;

    #[test]
    fn hosted_hint_omits_urls_and_keys() {
        let offers = vec![json!({
            "id": "offer-hosted",
            "adapter": {
                "kind": "openai_compatible_text",
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
    }

    #[test]
    fn local_llama_adapter_is_not_hosted() {
        let offers = vec![json!({
            "id": "local",
            "adapter": { "kind": "local_llama_cpp_text" }
        })];
        assert!(hosted_hint_from_offers(&offers, "local").is_none());
    }
}
