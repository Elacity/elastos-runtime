use anyhow::Context as _;
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read as _};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use elastos_common::localhost::{ensure_file_backed_roots, file_backed_prefixes};
use elastos_runtime::provider::{
    ProviderInvocation, ProviderInvocationTransport, ProviderTransfer,
};
use elastos_runtime::{capability, content, namespace, primitives, provider, session};
use elastos_server::api::browser_engine_protocol::{
    BROWSER_ENGINE_PROTOCOL_VERSION, BROWSER_ENGINE_PROVIDER_ID,
};
use elastos_server::binaries;
use elastos_server::content::ContentProvider;
use elastos_server::documents::DocumentsProvider;
use elastos_server::sources::{default_data_dir, local_session_owner};
use elastos_server::{api, fetcher, ownership};
use elastos_wallet_contract::WALLET_PROTOCOL_VERSION;

pub(crate) struct ServerInfrastructure {
    pub(crate) audit_log: Arc<primitives::audit::AuditLog>,
    pub(crate) session_registry: Arc<session::SessionRegistry>,
    pub(crate) capability_manager: Arc<capability::CapabilityManager>,
    pub(crate) pending_store: Arc<capability::pending::PendingRequestStore>,
    pub(crate) provider_registry: Arc<provider::ProviderRegistry>,
    pub(crate) namespace_store: Arc<namespace::NamespaceStore>,
    pub(crate) identity_state: Option<api::handlers::identity::IdentityState>,
    pub(crate) tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    pub(crate) provider_cid: String,
    pub(crate) shell_cid: Option<String>,
    pub(crate) host_helpers: Vec<api::server::HostHelperProcess>,
    pub(crate) carrier_service: Option<elastos_server::carrier::CarrierRuntimeService>,
    pub(crate) collaboration_context: api::gateway::GatewayCollaborationContext,
    pub(crate) collaboration_service:
        Option<elastos_server::collaboration_startup::CollaborationRuntimeService>,
}

const CONTENT_REPAIR_SCHEDULER_ENV: &str = "ELASTOS_CONTENT_REPAIR_SCHEDULER";
const CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV: &str = "ELASTOS_CONTENT_REPAIR_INTERVAL_SECS";
const CONTENT_REPAIR_SCHEDULER_LIMIT_ENV: &str = "ELASTOS_CONTENT_REPAIR_LIMIT";
const CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV: &str = "ELASTOS_CONTENT_REPAIR_MAX_ATTEMPTS";
const CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV: &str = "ELASTOS_CONTENT_REPAIR_FAILURE_BUDGET";
const CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV: &str = "ELASTOS_CONTENT_REPAIR_INCLUDE_HEALTHY";
const CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS: u64 = 60;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_INTERVAL_SECS: u64 = 15 * 60;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_LIMIT: u64 = 10;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_MAX_ATTEMPTS: u64 = 3;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_FAILURE_BUDGET: u64 = 5;
const BROWSER_ENGINE_PROVIDER_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const MODEL_PROVIDER_ID: &str = "model-provider";
const MODEL_PROVIDER_PROTOCOL_VERSION: &str = "elastos.model-provider/v1";
const MODEL_PROVIDER_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const MODEL_PROVIDER_CONFIG_FILE_NAME: &str = "config.json";
const MODEL_PROVIDER_CONFIG_MAX_BYTES: usize = 256 * 1024;
const MEDIA_PROVIDER_ID: &str = "media-provider";
const MEDIA_PROVIDER_ROUTE: &str = "media";
const MEDIA_PROVIDER_PROTOCOL_VERSION: &str = "elastos.media-provider/v1";
const MEDIA_PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => "0.1.0-dev",
};
const MEDIA_PROVIDER_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const MEDIA_PROVIDER_CONFIG_MAX_BYTES: usize = 8 * 1024;
const MEDIA_PROVIDER_CONFIG_SCHEMA: &str = "elastos.protected-content.media-provider-config/v1";
const MEDIA_PROVIDER_OUTPUT_PROFILE: &str = "browser_fmp4_h264_v1";
const MEDIA_PROVIDER_MAX_TIMEOUT_MS: u64 = 3_600_000;
const MEDIA_PROVIDER_MAX_STDIO_BYTES: usize = 1024 * 1024;
const MEDIA_PROVIDER_MAX_INPUT_BYTES: u64 = 1024 * 1024 * 1024;
const MEDIA_PROVIDER_MAX_OUTPUT_PART_BYTES: u64 = 64 * 1024 * 1024;
const MEDIA_PROVIDER_MAX_DURATION_SECS: u64 = 1_800;
const MEDIA_PROVIDER_MAX_SOURCE_WIDTH: u32 = 3_840;
const MEDIA_PROVIDER_MAX_SOURCE_HEIGHT: u32 = 2_160;
const MEDIA_PROVIDER_MAX_SOURCE_FPS: u32 = 60;
const MEDIA_PROVIDER_MAX_SEGMENT_COUNT: usize = 512;
const MEDIA_PROVIDER_MAX_TOTAL_OUTPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const WALLET_PROVIDER_STATUS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelProviderOperatorConfigFile {
    offers: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaProviderOperatorConfigFile {
    schema: String,
    ffmpeg_path: String,
    ffprobe_path: String,
    staging_root: String,
    output_profile: String,
    timeout_ms: u64,
    max_stdio_bytes: usize,
    max_input_bytes: u64,
    max_output_part_bytes: u64,
    max_duration_secs: u64,
    max_source_width: u32,
    max_source_height: u32,
    max_source_fps: u32,
    max_segment_count: usize,
    max_total_output_bytes: u64,
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

fn model_provider_bridge_config(data_dir: &Path) -> anyhow::Result<provider::BridgeProviderConfig> {
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

fn media_provider_root_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("protected-content").join(MEDIA_PROVIDER_ID)
}

fn media_provider_config_path(data_dir: &Path) -> PathBuf {
    media_provider_root_dir(data_dir).join("config.json")
}

fn media_provider_bridge_config(
    data_dir: &Path,
) -> anyhow::Result<Option<provider::BridgeProviderConfig>> {
    let config_path = media_provider_config_path(data_dir);
    let metadata = match fs::symlink_metadata(&config_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => anyhow::bail!("media-provider private config is unavailable"),
    };
    let protected_content_root = data_dir.join("protected-content");
    let provider_root = media_provider_root_dir(data_dir);
    let tools_root = provider_root.join("tools");
    let expected_staging_root = provider_root.join("staging");
    validate_model_provider_private_directory(
        &protected_content_root,
        "media-provider private parent",
    )
    .map_err(|_| anyhow::anyhow!("media-provider private config is unsafe"))?;
    validate_model_provider_private_directory(&provider_root, "media-provider private root")
        .map_err(|_| anyhow::anyhow!("media-provider private config is unsafe"))?;
    validate_model_provider_private_directory(&tools_root, "media-provider private tools")
        .map_err(|_| anyhow::anyhow!("media-provider private config is unsafe"))?;
    validate_model_provider_private_directory(
        &expected_staging_root,
        "media-provider private staging",
    )
    .map_err(|_| anyhow::anyhow!("media-provider private config is unsafe"))?;
    let bytes = read_model_provider_private_file(
        &config_path,
        &metadata,
        MEDIA_PROVIDER_CONFIG_MAX_BYTES,
        "media-provider private config",
    )
    .map_err(|_| anyhow::anyhow!("media-provider private config is unsafe"))?;
    let raw: MediaProviderOperatorConfigFile = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("media-provider private config is invalid"))?;
    let ffmpeg_path = PathBuf::from(&raw.ffmpeg_path);
    let ffprobe_path = PathBuf::from(&raw.ffprobe_path);
    if raw.schema != MEDIA_PROVIDER_CONFIG_SCHEMA
        || raw.output_profile != MEDIA_PROVIDER_OUTPUT_PROFILE
        || PathBuf::from(&raw.staging_root) != expected_staging_root
        || ffmpeg_path != tools_root.join("ffmpeg")
        || ffprobe_path != tools_root.join("ffprobe")
        || raw.timeout_ms == 0
        || raw.timeout_ms > MEDIA_PROVIDER_MAX_TIMEOUT_MS
        || raw.max_stdio_bytes == 0
        || raw.max_stdio_bytes > MEDIA_PROVIDER_MAX_STDIO_BYTES
        || raw.max_input_bytes == 0
        || raw.max_input_bytes > MEDIA_PROVIDER_MAX_INPUT_BYTES
        || raw.max_output_part_bytes == 0
        || raw.max_output_part_bytes > MEDIA_PROVIDER_MAX_OUTPUT_PART_BYTES
        || raw.max_duration_secs == 0
        || raw.max_duration_secs > MEDIA_PROVIDER_MAX_DURATION_SECS
        || raw.max_source_width == 0
        || raw.max_source_width > MEDIA_PROVIDER_MAX_SOURCE_WIDTH
        || raw.max_source_height == 0
        || raw.max_source_height > MEDIA_PROVIDER_MAX_SOURCE_HEIGHT
        || raw.max_source_fps == 0
        || raw.max_source_fps > MEDIA_PROVIDER_MAX_SOURCE_FPS
        || raw.max_segment_count == 0
        || raw.max_segment_count > MEDIA_PROVIDER_MAX_SEGMENT_COUNT
        || raw.max_total_output_bytes == 0
        || raw.max_total_output_bytes > MEDIA_PROVIDER_MAX_TOTAL_OUTPUT_BYTES
        || raw.max_total_output_bytes < raw.max_output_part_bytes
    {
        anyhow::bail!("media-provider private config is invalid");
    }
    validate_media_provider_tool(&ffmpeg_path)?;
    validate_media_provider_tool(&ffprobe_path)?;
    Ok(Some(provider::BridgeProviderConfig {
        extra: serde_json::json!({
            "provider_id": MEDIA_PROVIDER_ID,
            "staging_root": raw.staging_root,
            "ffmpeg_path": raw.ffmpeg_path,
            "ffprobe_path": raw.ffprobe_path,
            "output_profile": raw.output_profile,
            "timeout_ms": raw.timeout_ms,
            "max_stdio_bytes": raw.max_stdio_bytes,
            "max_input_bytes": raw.max_input_bytes,
            "max_output_part_bytes": raw.max_output_part_bytes,
            "max_duration_secs": raw.max_duration_secs,
            "max_source_width": raw.max_source_width,
            "max_source_height": raw.max_source_height,
            "max_source_fps": raw.max_source_fps,
            "max_segment_count": raw.max_segment_count,
            "max_total_output_bytes": raw.max_total_output_bytes,
        }),
        ..Default::default()
    }))
}

fn validate_media_provider_tool(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow::anyhow!("media-provider private tool is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("media-provider private tool is unsafe");
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || mode & 0o077 != 0
            || mode & 0o100 == 0
        {
            anyhow::bail!("media-provider private tool is unsafe");
        }
    }
    Ok(())
}

async fn request_browser_engine_provider_status(
    bridge: &provider::ProviderBridge,
    status_timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(
        status_timeout,
        bridge.send_raw(&serde_json::json!({"op": "status"})),
    )
    .await
    .map_err(|_| anyhow::anyhow!("browser-engine-adapter status request timed out"))?
    .map_err(|err| anyhow::anyhow!("browser-engine-adapter status request failed: {err}"))
}

async fn start_browser_engine_provider(
    registry: &provider::ProviderRegistry,
    bridge: Arc<provider::ProviderBridge>,
    status_timeout: Duration,
) -> anyhow::Result<()> {
    let startup = async {
        let status = request_browser_engine_provider_status(&bridge, status_timeout).await?;
        require_browser_engine_provider_status(&status)?;
        let browser_engine_provider: Arc<dyn provider::Provider> = Arc::new(
            provider::CapsuleProvider::with_scheme(bridge.clone(), "browser-engine"),
        );
        register_browser_engine_provider(registry, browser_engine_provider).await
    }
    .await;
    if let Err(startup_error) = startup {
        if let Err(shutdown_error) = bridge.shutdown().await {
            return Err(anyhow::anyhow!(
                "{startup_error}; browser-engine-adapter shutdown/reap also failed: {shutdown_error}"
            ));
        }
        return Err(startup_error);
    }
    Ok(())
}

fn require_browser_engine_provider_status(status: &serde_json::Value) -> anyhow::Result<()> {
    if status.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        anyhow::bail!("browser-engine-adapter status request did not succeed");
    }
    let data = status
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("browser-engine-adapter status is missing data"))?;
    if data.get("provider").and_then(serde_json::Value::as_str) != Some(BROWSER_ENGINE_PROVIDER_ID)
    {
        anyhow::bail!("browser-engine-adapter status has an unsupported provider identity");
    }
    if data
        .get("protocol_version")
        .and_then(serde_json::Value::as_str)
        != Some(BROWSER_ENGINE_PROTOCOL_VERSION)
    {
        anyhow::bail!("browser-engine-adapter status has an unsupported protocol version");
    }
    Ok(())
}

async fn register_browser_engine_provider(
    registry: &provider::ProviderRegistry,
    browser_engine_provider: Arc<dyn provider::Provider>,
) -> anyhow::Result<()> {
    if registry
        .registration_for_uri("elastos://browser-engine/status")
        .await
        .is_some()
    {
        anyhow::bail!("failed to register Browser Engine provider: route already registered");
    }
    registry
        .register_sub_provider("browser-engine", browser_engine_provider)
        .await
        .map_err(|err| anyhow::anyhow!("failed to register Browser Engine provider: {err}"))
}

async fn request_wallet_provider_v2_status(
    bridge: &provider::ProviderBridge,
    status_timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(
        status_timeout,
        bridge.send_raw(&serde_json::json!({"op": "status"})),
    )
    .await
    .map_err(|_| anyhow::anyhow!("wallet-provider status request timed out"))?
    .map_err(|err| anyhow::anyhow!("wallet-provider status request failed: {err}"))
}

async fn start_wallet_provider_v2(
    registry: &provider::ProviderRegistry,
    bridge: Arc<provider::ProviderBridge>,
    status_timeout: Duration,
) -> anyhow::Result<()> {
    let startup = async {
        let status = request_wallet_provider_v2_status(&bridge, status_timeout).await?;
        require_wallet_provider_v2_status(&status)?;
        let wallet_provider: Arc<dyn provider::Provider> = Arc::new(
            provider::CapsuleProvider::with_scheme(bridge.clone(), "wallet"),
        );
        register_wallet_provider_v2(registry, wallet_provider).await
    }
    .await;
    if let Err(startup_error) = startup {
        if let Err(shutdown_error) = bridge.shutdown().await {
            return Err(anyhow::anyhow!(
                "{startup_error}; wallet-provider shutdown/reap also failed: {shutdown_error}"
            ));
        }
        return Err(startup_error);
    }
    Ok(())
}

async fn request_model_provider_status(
    bridge: &provider::ProviderBridge,
    status_timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(
        status_timeout,
        bridge.send_raw(&serde_json::json!({"op": "status"})),
    )
    .await
    .map_err(|_| anyhow::anyhow!("model-provider status request timed out"))?
    .map_err(|err| anyhow::anyhow!("model-provider status request failed: {err}"))
}

async fn start_model_provider(
    registry: &provider::ProviderRegistry,
    bridge: Arc<provider::ProviderBridge>,
    status_timeout: Duration,
) -> anyhow::Result<()> {
    let startup = async {
        let status = request_model_provider_status(&bridge, status_timeout).await?;
        require_model_provider_status(&status)?;
        let model_provider: Arc<dyn provider::Provider> = Arc::new(
            provider::CapsuleProvider::with_scheme(bridge.clone(), "model"),
        );
        register_model_provider(registry, model_provider).await
    }
    .await;
    if let Err(startup_error) = startup {
        if let Err(shutdown_error) = bridge.shutdown().await {
            return Err(anyhow::anyhow!(
                "{startup_error}; model-provider shutdown/reap also failed: {shutdown_error}"
            ));
        }
        return Err(startup_error);
    }
    Ok(())
}

async fn request_media_provider_status(
    bridge: &provider::ProviderBridge,
    status_timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(
        status_timeout,
        bridge.send_raw(&serde_json::json!({"op": "status"})),
    )
    .await
    .map_err(|_| anyhow::anyhow!("media-provider status request timed out"))?
    .map_err(|_| anyhow::anyhow!("media-provider status request failed"))
}

async fn start_media_provider(
    registry: &provider::ProviderRegistry,
    bridge: Arc<provider::ProviderBridge>,
    status_timeout: Duration,
) -> anyhow::Result<()> {
    let startup = async {
        let status = request_media_provider_status(&bridge, status_timeout).await?;
        require_media_provider_status(&status)?;
        let media_provider: Arc<dyn provider::Provider> = Arc::new(
            provider::CapsuleProvider::with_scheme(bridge.clone(), MEDIA_PROVIDER_ROUTE),
        );
        register_media_provider(registry, media_provider).await
    }
    .await;
    if let Err(startup_error) = startup {
        if let Err(shutdown_error) = bridge.shutdown().await {
            return Err(anyhow::anyhow!(
                "{startup_error}; media-provider shutdown/reap also failed: {shutdown_error}"
            ));
        }
        return Err(startup_error);
    }
    Ok(())
}

fn require_wallet_provider_v2_status(status: &serde_json::Value) -> anyhow::Result<()> {
    if status.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        anyhow::bail!("wallet-provider status request did not succeed");
    }
    let data = status
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("wallet-provider status is missing data"))?;
    if data.get("provider").and_then(serde_json::Value::as_str) != Some("wallet-provider") {
        anyhow::bail!("wallet-provider status has an unsupported provider identity");
    }
    if data
        .get("protocol_version")
        .and_then(serde_json::Value::as_str)
        != Some(WALLET_PROTOCOL_VERSION)
    {
        anyhow::bail!("wallet-provider status has an unsupported protocol version");
    }
    Ok(())
}

fn require_model_provider_status(status: &serde_json::Value) -> anyhow::Result<()> {
    if status.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        anyhow::bail!("model-provider status request did not succeed");
    }
    let data = status
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("model-provider status is missing data"))?;
    if data.get("provider").and_then(serde_json::Value::as_str) != Some(MODEL_PROVIDER_ID) {
        anyhow::bail!("model-provider status has an unsupported provider identity");
    }
    if data
        .get("protocol_version")
        .and_then(serde_json::Value::as_str)
        != Some(MODEL_PROVIDER_PROTOCOL_VERSION)
    {
        anyhow::bail!("model-provider status has an unsupported protocol version");
    }
    Ok(())
}

fn require_media_provider_status(status: &serde_json::Value) -> anyhow::Result<()> {
    if status.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        anyhow::bail!("media-provider status request did not succeed");
    }
    let data = status
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("media-provider status is missing data"))?;
    if data.get("provider").and_then(serde_json::Value::as_str) != Some(MEDIA_PROVIDER_ID) {
        anyhow::bail!("media-provider status has an unsupported provider identity");
    }
    if data
        .get("protocol_version")
        .and_then(serde_json::Value::as_str)
        != Some(MEDIA_PROVIDER_PROTOCOL_VERSION)
    {
        anyhow::bail!("media-provider status has an unsupported protocol version");
    }
    if data.get("version").and_then(serde_json::Value::as_str) != Some(MEDIA_PROVIDER_VERSION) {
        anyhow::bail!("media-provider status has an unsupported version");
    }
    if data.get("configured").and_then(serde_json::Value::as_bool) != Some(true) {
        anyhow::bail!("media-provider is not configured");
    }
    let operations = data
        .get("supported_operations")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("media-provider status is missing operations"))?;
    if operations.len() != 2
        || operations[0].as_str() != Some("status")
        || operations[1].as_str() != Some("prepare")
    {
        anyhow::bail!("media-provider status has unsupported operations");
    }
    Ok(())
}

async fn register_wallet_provider_v2(
    registry: &provider::ProviderRegistry,
    wallet_provider: Arc<dyn provider::Provider>,
) -> anyhow::Result<()> {
    registry
        .register_sub_provider("wallet", wallet_provider)
        .await
        .map_err(|err| anyhow::anyhow!("failed to register Wallet provider v2: {err}"))
}

async fn register_model_provider(
    registry: &provider::ProviderRegistry,
    model_provider: Arc<dyn provider::Provider>,
) -> anyhow::Result<()> {
    if registry
        .registration_for_uri("elastos://model/offers")
        .await
        .is_some()
    {
        anyhow::bail!("failed to register model provider: route already registered");
    }
    registry
        .register_sub_provider("model", model_provider)
        .await
        .map_err(|err| anyhow::anyhow!("failed to register model provider: {err}"))
}

async fn register_media_provider(
    registry: &provider::ProviderRegistry,
    media_provider: Arc<dyn provider::Provider>,
) -> anyhow::Result<()> {
    if registry
        .registration_for_uri("elastos://media/status")
        .await
        .is_some()
    {
        anyhow::bail!("failed to register media provider: route already registered");
    }
    registry
        .register_sub_provider(MEDIA_PROVIDER_ROUTE, media_provider)
        .await
        .map_err(|err| anyhow::anyhow!("failed to register media provider: {err}"))
}

pub(crate) async fn setup_server_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(true).await
}

pub(crate) async fn setup_control_plane_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(false).await
}

async fn setup_server_infrastructure_impl(
    spawn_host_providers: bool,
) -> anyhow::Result<ServerInfrastructure> {
    let data_dir = default_data_dir();
    let _ = ownership::repair_path_recursive(&data_dir);
    let collaboration_configuration =
        elastos_server::collaboration_startup::load_and_accept_collaboration_startup_configuration(
            &data_dir,
        )?;

    let audit_log = Arc::new(primitives::audit::AuditLog::new());
    let session_registry = Arc::new(session::SessionRegistry::new(audit_log.clone()));
    session_registry
        .set_default_owner(local_session_owner(&data_dir)?)
        .await;
    let metrics = Arc::new(primitives::metrics::MetricsManager::new());
    let capability_store = Arc::new(capability::CapabilityStore::new());
    let capability_manager = Arc::new(capability::CapabilityManager::load_or_generate(
        &data_dir,
        capability_store,
        audit_log.clone(),
        metrics.clone(),
    ));
    let pending_store = Arc::new(capability::pending::PendingRequestStore::new(
        audit_log.clone(),
    ));

    let tls_config = match elastos_tls::load_or_create_tls_config(&data_dir).await {
        Ok(config) => {
            tracing::info!("TLS enabled (self-signed CA)");
            Some(config)
        }
        Err(e) => {
            tracing::warn!("TLS disabled: {}. Running without HTTPS.", e);
            None
        }
    };

    ensure_file_backed_roots(&data_dir).ok();
    elastos_server::protected_content_runtime::log_unresolved_runtime_releases(&data_dir);
    let provider_registry = Arc::new(provider::ProviderRegistry::new());
    let mut managed_host_processes = Vec::new();
    let mut external_availability_registered = false;
    let mut carrier_service = None;
    let content_provider = Arc::new(ContentProvider::new(
        data_dir.clone(),
        Arc::downgrade(&provider_registry),
    ));
    provider_registry.register(content_provider.clone()).await;
    if let Err(err) = provider_registry
        .register_sub_provider("content", content_provider)
        .await
    {
        tracing::warn!("Failed to register elastos://content sub-provider: {}", err);
    }
    provider_registry
        .register(Arc::new(DocumentsProvider::new(
            data_dir.clone(),
            Arc::downgrade(&provider_registry),
        )))
        .await;
    let inspect_source: Arc<dyn elastos_server::inspect_provider::InspectSource> = Arc::new(
        elastos_server::inspect_provider::AggregateInspectSource::new(vec![
            Arc::new(elastos_server::inspect_provider::CatalogInspectSource::new(
                data_dir.join("capsules"),
                Arc::downgrade(&provider_registry),
            )),
            Arc::new(
                elastos_server::inspect_provider::RegistryInspectSource::new(Arc::downgrade(
                    &provider_registry,
                )),
            ),
        ]),
    );
    let inspect_provider: Arc<dyn provider::Provider> = Arc::new(
        elastos_server::inspect_provider::InspectProvider::with_registry(
            inspect_source,
            Arc::downgrade(&provider_registry),
        ),
    );
    provider_registry.register(inspect_provider.clone()).await;
    if let Err(err) = provider_registry
        .register_sub_provider("inspect", inspect_provider)
        .await
    {
        tracing::warn!("Failed to register elastos://inspect sub-provider: {}", err);
    }
    let device_key = elastos_identity::load_or_create_device_key(&data_dir)?;
    let device_key_hex = hex::encode(device_key.as_ref());
    let mut provider_cid = "sha256:unavailable".to_string();
    let verify_provider_binary = |name: &str, path: &std::path::Path| -> anyhow::Result<()> {
        let checksum = crate::setup::verify_installed_component_binary(&data_dir, name, path)?;
        tracing::info!(
            "{} binary verified against installed manifest ({})",
            name,
            checksum
        );
        Ok(())
    };

    match binaries::resolve_verified_native_provider_binary("did-provider") {
        Ok(Some(path)) => {
            let did_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                allowed_paths: file_backed_prefixes(),
                read_only: false,
                encryption_key: device_key_hex.clone(),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, did_config).await {
                Ok(bridge) => {
                    let provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "did"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("did", provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://did sub-provider: {}", e);
                    }
                    tracing::info!("did-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn did-provider: {}", e),
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping did-provider due to verification failure: {}", e),
    }

    if spawn_host_providers {
        let binary_path = binaries::resolve_verified_native_provider_binary("localhost-provider")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "localhost-provider not installed.\n  \
                 Run:\n  \
                   elastos setup --with localhost-provider"
                )
            })?;

        let provider_bytes = std::fs::read(&binary_path)?;
        provider_cid = format!(
            "sha256:{}",
            hex::encode(elastos_runtime::signature::hash_content(&provider_bytes))
        );

        let config = provider::BridgeProviderConfig {
            base_path: data_dir.to_string_lossy().to_string(),
            allowed_paths: file_backed_prefixes(),
            read_only: false,
            encryption_key: device_key_hex.clone(),
            ..Default::default()
        };
        let bridge = provider::ProviderBridge::spawn(&binary_path, config)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to spawn localhost-provider capsule: {}.\n  \
                     Reinstall with:\n  \
                       elastos setup --with localhost-provider",
                    e
                )
            })?;
        tracing::info!(
            "localhost-provider capsule {} from {}",
            provider_cid,
            binary_path.display()
        );
        let provider: Arc<dyn provider::Provider> =
            Arc::new(provider::CapsuleProvider::new(Arc::new(bridge)));
        provider_registry.register(provider).await;

        if let Some(availability_config) = availability_provider_config_from_env() {
            if let Some(path) = crate::find_installed_provider_binary("availability-provider") {
                if let Err(e) = verify_provider_binary("availability-provider", &path) {
                    tracing::warn!(
                        "Skipping availability-provider due to verification failure: {}",
                        e
                    );
                } else {
                    let config = provider::BridgeProviderConfig {
                        extra: availability_config,
                        ..Default::default()
                    };
                    match provider::ProviderBridge::spawn(&path, config).await {
                        Ok(bridge) => {
                            let availability_provider: Arc<dyn provider::Provider> =
                                Arc::new(provider::CapsuleProvider::with_scheme(
                                    Arc::new(bridge),
                                    "availability",
                                ));
                            if let Err(e) = provider_registry
                                .register_sub_provider("availability", availability_provider)
                                .await
                            {
                                tracing::warn!(
                                    "Failed to register elastos://availability sub-provider: {}",
                                    e
                                );
                            } else {
                                external_availability_registered = true;
                            }
                            tracing::info!("availability-provider capsule from {}", path.display());
                        }
                        Err(e) => tracing::warn!("Failed to spawn availability-provider: {}", e),
                    }
                }
            } else {
                tracing::warn!(
                    "Availability provider configured but availability-provider binary is not installed"
                );
            }
        }
    }

    match binaries::resolve_verified_native_provider_binary("content-block-graph-provider") {
        Ok(Some(path)) => {
            let block_graph_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                extra: serde_json::json!({
                    "backend": "kubo_coord"
                }),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, block_graph_config).await {
                Ok(bridge) => {
                    let block_graph_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "block-graph"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("block-graph", block_graph_provider)
                        .await
                    {
                        tracing::warn!(
                            "Failed to register elastos://block-graph sub-provider: {}",
                            e
                        );
                    }
                    tracing::info!(
                        "content-block-graph-provider capsule from {}",
                        path.display()
                    );
                }
                Err(e) => tracing::warn!("Failed to spawn content-block-graph-provider: {}", e),
            }
        }
        Ok(None) => {
            tracing::warn!(
                "content-block-graph-provider binary is not installed; arbitrary DAG repair will fail closed"
            );
        }
        Err(e) => tracing::warn!(
            "Skipping content-block-graph-provider due to verification failure: {}",
            e
        ),
    }

    match binaries::resolve_verified_native_provider_binary("object-provider") {
        Ok(Some(path)) => {
            let object_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                allowed_paths: file_backed_prefixes(),
                read_only: false,
                encryption_key: device_key_hex.clone(),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, object_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    let object_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(bridge.clone(), "object"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("object", object_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://object sub-provider: {}", e);
                    }
                    tracing::info!("object-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn object-provider: {}", e),
            }
        }
        Ok(None) => {
            tracing::warn!(
                "object-provider binary is not installed; Library object operations will fail closed"
            );
        }
        Err(e) => tracing::warn!(
            "Skipping object-provider due to verification failure: {}",
            e
        ),
    }

    match binaries::resolve_verified_native_provider_binary("webspace-provider") {
        Ok(Some(path)) => {
            let webspace_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                read_only: false,
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, webspace_config).await {
                Ok(bridge) => {
                    let provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "webspace"),
                    );
                    provider_registry.register(provider).await;
                    tracing::info!("webspace-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn webspace-provider: {}", e),
            }
        }
        Ok(None) => {
            tracing::warn!(
                "webspace-provider binary is not installed; WebSpace roots will fail closed"
            );
        }
        Err(e) => tracing::warn!(
            "Skipping webspace-provider due to verification failure: {}",
            e
        ),
    }

    match binaries::resolve_verified_native_provider_binary("model-provider") {
        Ok(Some(path)) => match model_provider_bridge_config(&data_dir) {
            Ok(model_config) => match provider::ProviderBridge::spawn(&path, model_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    match start_model_provider(
                        &provider_registry,
                        bridge,
                        MODEL_PROVIDER_STATUS_TIMEOUT,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::info!("model-provider capsule from {}", path.display())
                        }
                        Err(_) => {
                            tracing::warn!("Skipping model-provider because startup failed")
                        }
                    }
                }
                Err(_) => tracing::warn!("Skipping model-provider because startup failed"),
            },
            Err(_) => {
                tracing::warn!("Skipping model-provider due to invalid private operator config")
            }
        },
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping model-provider due to verification failure: {}", e),
    }

    match binaries::resolve_verified_native_provider_binary(MEDIA_PROVIDER_ID) {
        Ok(Some(path)) => match media_provider_bridge_config(&data_dir) {
            Ok(Some(media_config)) => {
                match provider::ProviderBridge::spawn(&path, media_config).await {
                    Ok(bridge) => {
                        let bridge = Arc::new(bridge);
                        match start_media_provider(
                            &provider_registry,
                            bridge,
                            MEDIA_PROVIDER_STATUS_TIMEOUT,
                        )
                        .await
                        {
                            Ok(()) => {
                                tracing::info!("media-provider capsule from verified install")
                            }
                            Err(_) => {
                                tracing::warn!("Skipping media-provider because startup failed")
                            }
                        }
                    }
                    Err(_) => tracing::warn!("Skipping media-provider because startup failed"),
                }
            }
            Ok(None) => tracing::info!("media-provider is installed but unconfigured"),
            Err(_) => tracing::warn!("Skipping media-provider due to invalid private config"),
        },
        Ok(None) => {}
        Err(_) => tracing::warn!("Skipping media-provider due to verification failure"),
    }

    if let Some(path) = crate::find_installed_provider_binary("operator-drive-adapter") {
        if let Err(e) = verify_provider_binary("operator-drive-adapter", &path) {
            tracing::warn!(
                "Skipping operator-drive-adapter due to verification failure: {}",
                e
            );
        } else {
            let adapter_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                read_only: false,
                extra: operator_drive_adapter_config_from_env(&data_dir)
                    .unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, adapter_config).await {
                Ok(bridge) => {
                    let provider: Arc<dyn provider::Provider> =
                        Arc::new(provider::CapsuleProvider::with_scheme(
                            Arc::new(bridge),
                            "operator-drive-adapter",
                        ));
                    provider_registry.register(provider).await;
                    tracing::info!("operator-drive-adapter capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn operator-drive-adapter: {}", e),
            }
        }
    }

    match binaries::resolve_verified_native_provider_binary("ipfs-provider") {
        Ok(Some(path)) => match provider::ProviderBridge::spawn(&path, Default::default()).await {
            Ok(bridge) => {
                let bridge = Arc::new(bridge);
                let ipfs_provider: Arc<dyn provider::Provider> = Arc::new(
                    provider::CapsuleProvider::with_scheme(Arc::clone(&bridge), "ipfs"),
                );
                if let Err(e) = provider_registry
                    .register_sub_provider("ipfs", ipfs_provider)
                    .await
                {
                    tracing::warn!("Failed to register elastos://ipfs sub-provider: {}", e);
                }
                tracing::info!("ipfs-provider capsule from {}", path.display());
            }
            Err(e) => tracing::warn!("ipfs-provider unavailable: {}", e),
        },
        Ok(None) => {
            tracing::warn!(
                "ipfs-provider binary is not installed; elastos://content publish/fetch will fail closed"
            );
        }
        Err(e) => tracing::warn!("Skipping ipfs-provider due to verification failure: {}", e),
    }

    match binaries::resolve_verified_native_provider_binary("chain-provider") {
        Ok(Some(path)) => match provider::ProviderBridge::spawn(&path, Default::default()).await {
            Ok(bridge) => {
                let chain_provider: Arc<dyn provider::Provider> = Arc::new(
                    provider::CapsuleProvider::with_scheme(Arc::new(bridge), "chain"),
                );
                if let Err(e) = provider_registry
                    .register_sub_provider("chain", chain_provider)
                    .await
                {
                    tracing::warn!("Failed to register elastos://chain sub-provider: {}", e);
                }
                tracing::info!("chain-provider capsule from {}", path.display());
            }
            Err(e) => tracing::warn!("Failed to spawn chain-provider: {}", e),
        },
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping chain-provider due to verification failure: {}", e),
    }

    match binaries::resolve_verified_native_provider_binary("net-provider") {
        Ok(Some(path)) => match provider::ProviderBridge::spawn(&path, Default::default()).await {
            Ok(bridge) => {
                let net_provider: Arc<dyn provider::Provider> = Arc::new(
                    provider::CapsuleProvider::with_scheme(Arc::new(bridge), "net"),
                );
                if let Err(e) = provider_registry
                    .register_sub_provider("net", net_provider)
                    .await
                {
                    tracing::warn!("Failed to register elastos://net sub-provider: {}", e);
                }
                tracing::info!("net-provider capsule from {}", path.display());
            }
            Err(e) => tracing::warn!("Failed to spawn net-provider: {}", e),
        },
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping net-provider due to verification failure: {}", e),
    }

    if let Some(local_exit_config) = browser_local_exit_config_from_env(&data_dir) {
        if let Some(path) = crate::find_installed_provider_binary("browser-local-exit") {
            if let Err(e) = verify_provider_binary("browser-local-exit", &path) {
                tracing::warn!(
                    "Skipping browser-local-exit due to verification failure: {}",
                    e
                );
            } else {
                match spawn_browser_local_exit(&path, &local_exit_config) {
                    Ok(child) => {
                        tracing::info!("browser-local-exit helper from {}", path.display());
                        managed_host_processes.push(api::server::HostHelperProcess {
                            name: "browser-local-exit",
                            child,
                        });
                    }
                    Err(e) => tracing::warn!("Failed to spawn browser-local-exit: {}", e),
                }
            }
        } else {
            tracing::warn!(
                "Browser local Exit configured but browser-local-exit binary is not installed"
            );
        }
    }

    match binaries::resolve_verified_native_provider_binary("exit-provider") {
        Ok(Some(path)) => {
            let exit_config = provider::BridgeProviderConfig {
                extra: exit_provider_config_from_env(&data_dir).unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, exit_config).await {
                Ok(bridge) => {
                    let exit_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "exit"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("exit", exit_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://exit sub-provider: {}", e);
                    }
                    tracing::info!("exit-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn exit-provider: {}", e),
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping exit-provider due to verification failure: {}", e),
    }

    match binaries::resolve_verified_native_provider_binary("browser-engine-adapter") {
        Ok(Some(path)) => {
            let browser_engine_config = provider::BridgeProviderConfig {
                extra: browser_engine_adapter_config_from_env(&data_dir)
                    .unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, browser_engine_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    match start_browser_engine_provider(
                        &provider_registry,
                        bridge,
                        BROWSER_ENGINE_PROVIDER_STATUS_TIMEOUT,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::info!("browser-engine-adapter capsule from {}", path.display())
                        }
                        Err(e) => {
                            tracing::warn!("Failed to start browser-engine-adapter: {}", e)
                        }
                    }
                }
                Err(e) => tracing::warn!("Failed to spawn browser-engine-adapter: {}", e),
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            "Skipping browser-engine-adapter due to verification failure: {}",
            e
        ),
    }

    match binaries::resolve_verified_native_provider_binary("wallet-provider") {
        Ok(Some(path)) => {
            let wallet_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                allowed_paths: file_backed_prefixes(),
                read_only: false,
                encryption_key: hex::encode(&device_key),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, wallet_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    let startup = start_wallet_provider_v2(
                        &provider_registry,
                        bridge,
                        WALLET_PROVIDER_STATUS_TIMEOUT,
                    )
                    .await;
                    match startup {
                        Ok(()) => {
                            tracing::info!("wallet-provider v2 capsule from {}", path.display())
                        }
                        Err(e) => {
                            tracing::warn!("Skipping wallet-provider after shutdown/reap: {}", e)
                        }
                    }
                }
                Err(e) => tracing::warn!("Failed to spawn wallet-provider: {}", e),
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            "Skipping wallet-provider due to verification failure: {}",
            e
        ),
    }

    if let Some(path) = crate::find_installed_provider_binary("drm-provider") {
        if let Err(e) = verify_provider_binary("drm-provider", &path) {
            tracing::warn!("Skipping drm-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let drm_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "drm"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("drm", drm_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://drm sub-provider: {}", e);
                    }
                    tracing::info!("drm-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn drm-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("rights-provider") {
        if let Err(e) = verify_provider_binary("rights-provider", &path) {
            tracing::warn!(
                "Skipping rights-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let rights_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "rights"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("rights", rights_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://rights sub-provider: {}", e);
                    }
                    tracing::info!("rights-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn rights-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("key-provider") {
        if let Err(e) = verify_provider_binary("key-provider", &path) {
            tracing::warn!("Skipping key-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let key_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "key"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("key", key_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://key sub-provider: {}", e);
                    }
                    tracing::info!("key-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn key-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("decrypt-provider") {
        if let Err(e) = verify_provider_binary("decrypt-provider", &path) {
            tracing::warn!(
                "Skipping decrypt-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let decrypt_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "decrypt"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("decrypt", decrypt_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://decrypt sub-provider: {}", e);
                    }
                    tracing::info!("decrypt-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn decrypt-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("protected-content-protect-provider")
    {
        if let Err(e) = verify_provider_binary("protected-content-protect-provider", &path) {
            tracing::warn!(
                "Skipping protected-content-protect-provider due to verification failure: {}",
                e
            );
        } else if let Err(e) = elastos_server::protected_content_runtime::register_protect_provider(
            &provider_registry,
            &path,
        )
        .await
        {
            tracing::warn!("Failed to register Runtime-only protect provider: {}", e);
        } else {
            tracing::info!(
                "protected-content-protect-provider capsule from {}",
                path.display()
            );
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("custody-provider") {
        if let Err(e) = verify_provider_binary("custody-provider", &path) {
            tracing::warn!(
                "Skipping custody-provider due to verification failure: {}",
                e
            );
        } else if let Err(e) =
            elastos_server::protected_content_runtime::register_inactive_custody_provider(
                &provider_registry,
                &path,
                &data_dir,
            )
            .await
        {
            tracing::warn!(
                "Failed to register inactive elastos://custody sub-provider: {}",
                e
            );
        } else {
            tracing::info!(
                "custody-provider registered as inactive Runtime custody route; provisional key-provider remains the product path"
            );
        }
    }

    // Built-in Carrier node — ALWAYS starts, not conditional on spawn_host_providers.
    // Carrier is fundamental infrastructure: gossip, content, identity.
    // Identity is DID (derived from device_key), not raw device_key.
    let (carrier_signing_key, carrier_did) = elastos_identity::derive_did(&device_key);
    let mut collaboration_carrier_provider: Option<Arc<dyn provider::Provider>> = None;
    {
        match elastos_server::carrier::start_carrier_node_with_registry(
            &carrier_signing_key,
            &carrier_did,
            data_dir.clone(),
            Some(Arc::downgrade(&provider_registry)),
        )
        .await
        {
            Ok(carrier_node) => {
                provider_registry
                    .set_carrier_invoker(Arc::new(
                        elastos_server::carrier::CarrierProviderInvoker::with_carrier_endpoint_and_registry(
                            carrier_node.endpoint.clone(),
                            Arc::downgrade(&provider_registry),
                        ),
                    ))
                    .await;
                let gossip_provider: Arc<dyn provider::Provider> =
                    Arc::new(elastos_server::carrier::CarrierGossipProvider::new(
                        carrier_node.gossip_state.clone(),
                    ));
                collaboration_carrier_provider = Some(gossip_provider.clone());
                if let Err(e) = provider_registry
                    .register_sub_provider("peer", gossip_provider)
                    .await
                {
                    tracing::warn!("Failed to register Carrier gossip provider: {}", e);
                }
                if !external_availability_registered {
                    let availability_provider: Arc<dyn provider::Provider> =
                        Arc::new(
                            elastos_server::carrier::CarrierAvailabilityProvider::with_provider_registry_data_dir_and_peer_attestation_exchange_config(
                            carrier_node.gossip_state.clone(),
                            Arc::downgrade(&provider_registry),
                            data_dir.clone(),
                            carrier_peer_attestation_exchange_config_from_env(),
                        ));
                    if let Err(e) = provider_registry
                        .register_sub_provider("availability", availability_provider)
                        .await
                    {
                        tracing::warn!("Failed to register Carrier availability provider: {}", e);
                    }
                }
                carrier_service = Some(elastos_server::carrier::CarrierRuntimeService::new(
                    carrier_node,
                ));
                tracing::info!("Carrier node online (P2P + gossip)");
            }
            Err(e) => {
                tracing::warn!("Carrier node failed: {:#}", e);
            }
        }
    }

    let collaboration_service =
        elastos_server::collaboration_startup::start_collaboration_runtime_service(
            &data_dir,
            carrier_signing_key,
            collaboration_configuration,
            collaboration_carrier_provider,
            provider_registry.clone(),
        )
        .await?;
    let collaboration_context = collaboration_service
        .as_ref()
        .map(|service| service.gateway_context())
        .unwrap_or_default();

    maybe_spawn_content_repair_scheduler(provider_registry.clone());

    let namespace_path = data_dir.join("namespaces");
    std::fs::create_dir_all(&namespace_path).ok();
    let resolver_config = content::ResolverConfig {
        // No ambient public-web fetch in the default trusted server path.
        ipfs_gateways: Vec::new(),
        ..content::ResolverConfig::default()
    };
    let content_resolver = Arc::new(content::ContentResolver::new(
        resolver_config,
        audit_log.clone(),
        Arc::new(fetcher::LoopbackIpfsGatewayFetcher::new()),
    ));
    let namespace_store = Arc::new(namespace::NamespaceStore::new(
        namespace_path,
        content_resolver,
        audit_log.clone(),
    ));

    let identity_state = match elastos_identity::IdentityManager::new(data_dir.clone()) {
        Ok(manager) => {
            tracing::info!("Identity manager initialized (dynamic RP)");
            Some(api::handlers::identity::IdentityState {
                manager: Arc::new(tokio::sync::Mutex::new(manager)),
                session_registry: session_registry.clone(),
                audit_log: Some(audit_log.clone()),
                data_dir: data_dir.clone(),
            })
        }
        Err(e) => {
            tracing::warn!("Identity manager disabled: {}", e);
            None
        }
    };

    let shell_cid = crate::find_installed_provider_binary("shell").and_then(|path| {
        std::fs::read(&path).ok().map(|bytes| {
            let cid = format!(
                "sha256:{}",
                hex::encode(elastos_runtime::signature::hash_content(&bytes))
            );
            tracing::info!("shell capsule {} from {}", cid, path.display());
            cid
        })
    });

    Ok(ServerInfrastructure {
        audit_log,
        session_registry,
        capability_manager,
        pending_store,
        provider_registry,
        namespace_store,
        identity_state,
        tls_config,
        provider_cid,
        shell_cid,
        host_helpers: managed_host_processes,
        carrier_service,
        collaboration_context,
        collaboration_service,
    })
}

fn spawn_browser_local_exit(path: &Path, config: &serde_json::Value) -> anyhow::Result<Child> {
    let relay_path = browser_local_exit_relay_path(config)?;
    if config
        .get("replace_existing_socket")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        remove_existing_browser_local_exit_socket(&relay_path)?;
    }
    // `stdin` is the helper's parent-liveness channel, not a data channel: we
    // hold the write end open for as long as this process lives, so the helper
    // reads EOF and reaps itself however we exit. `HostHelperProcess::drop` only
    // covers graceful shutdown — SIGKILL, an abort on panic, and the installed
    // binary supersession watch's `process::exit` all skip it, and each of those
    // used to strand a helper holding the relay socket.
    let mut child = Command::new(path)
        .env(
            "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
            serde_json::to_string(config)?,
        )
        .env("ELASTOS_BROWSER_LOCAL_EXIT_PARENT_EOF", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!("browser-local-exit exited during startup with {status}");
        }
        match browser_local_exit_socket_ready(&relay_path) {
            Ok(true) => return Ok(child),
            Ok(false) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("browser-local-exit did not create relay socket");
}

fn browser_local_exit_socket_ready(path: &Path) -> anyhow::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            anyhow::bail!(
                "failed to inspect browser-local-exit relay socket {}: {}",
                path.display(),
                err
            )
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            anyhow::bail!(
                "browser-local-exit relay_ipc_path is not a Unix socket: {}",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
    }

    Ok(true)
}

fn browser_local_exit_relay_path(config: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let relay_path = config
        .get("relay_ipc_path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("browser-local-exit config missing relay_ipc_path"))?;
    if relay_path.is_empty()
        || relay_path
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        anyhow::bail!(
            "browser-local-exit relay_ipc_path must be an absolute path without whitespace"
        );
    }
    let path = PathBuf::from(relay_path);
    if !path.is_absolute() {
        anyhow::bail!(
            "browser-local-exit relay_ipc_path must be an absolute path without whitespace"
        );
    }
    Ok(path)
}

fn remove_existing_browser_local_exit_socket(path: &Path) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            anyhow::bail!(
                "failed to inspect existing browser-local-exit relay socket {}: {}",
                path.display(),
                err
            )
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            anyhow::bail!(
                "refusing to remove browser-local-exit relay_ipc_path because it is not a Unix socket: {}",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        anyhow::bail!(
            "browser-local-exit replace_existing_socket is only supported for Unix socket paths"
        );
    }

    // Unlinking a socket a live helper is still listening on does not stop that
    // helper — it keeps serving an unreachable, unlinked socket forever. Doing it
    // blindly on every launch is how helpers piled up one per launch, all bound to
    // the same path. Give an incumbent a moment to reap itself (the parent-EOF
    // watch makes that prompt) before deciding the path is genuinely abandoned.
    #[cfg(unix)]
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        while browser_local_exit_socket_has_listener(path) {
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "browser-local-exit relay socket {} is still served by a live helper; \
                     stop the ElastOS host that owns it before starting another",
                    path.display()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        // The incumbent reaped itself and removed its own socket.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::anyhow!(
            "failed to remove existing browser-local-exit relay socket {}: {}",
            path.display(),
            err
        )),
    }
}

/// Whether anything is still accepting on `path`, distinguishing a live helper
/// from a socket file left behind by one that is already gone.
#[cfg(unix)]
fn browser_local_exit_socket_has_listener(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;

    match UnixStream::connect(path) {
        Ok(stream) => {
            // Connected without sending a relay-open handshake; drop it immediately.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            true
        }
        // ECONNREFUSED means the socket file outlived its listener.
        Err(_) => false,
    }
}

fn availability_provider_config_from_env() -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var("ELASTOS_AVAILABILITY_PROVIDER_CONFIG") {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => return Some(value),
            Err(err) => {
                tracing::warn!(
                    "Ignoring invalid ELASTOS_AVAILABILITY_PROVIDER_CONFIG JSON: {}",
                    err
                );
                return None;
            }
        }
    }

    let ensure_url = std::env::var("ELASTOS_AVAILABILITY_ENSURE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let mut target = serde_json::json!({
        "id": std::env::var("ELASTOS_AVAILABILITY_PROVIDER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "configured-supernode".to_string()),
        "ensure_url": ensure_url,
    });
    if let Ok(value) = std::env::var("ELASTOS_AVAILABILITY_AUTHORIZATION") {
        let value = value.trim();
        if !value.is_empty() {
            target["authorization"] = serde_json::Value::String(value.to_string());
        }
    }

    Some(serde_json::json!({
        "targets": [target]
    }))
}

fn carrier_peer_attestation_exchange_config_from_env() -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG") {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => return Some(value),
            Err(err) => {
                tracing::warn!(
                    "Ignoring invalid ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG JSON: {}",
                    err
                );
                return None;
            }
        }
    }

    let url = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let mut config = serde_json::json!({ "url": url });
    if let Ok(value) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION") {
        let value = value.trim();
        if !value.is_empty() {
            config["authorization"] = serde_json::Value::String(value.to_string());
        }
    }
    if let Ok(value) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS") {
        if let Ok(timeout_secs) = value.trim().parse::<u64>() {
            config["timeout_secs"] = serde_json::Value::from(timeout_secs);
        }
    }
    Some(config)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRepairSchedulerConfig {
    interval_secs: u64,
    limit: u64,
    max_attempts: u64,
    failure_budget: u64,
    include_healthy_check: bool,
}

fn content_repair_scheduler_config_from_env() -> Option<ContentRepairSchedulerConfig> {
    if !env_flag_enabled(CONTENT_REPAIR_SCHEDULER_ENV) {
        return None;
    }
    Some(ContentRepairSchedulerConfig {
        interval_secs: env_u64(
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_INTERVAL_SECS,
        )
        .max(CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS),
        limit: env_u64(
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_LIMIT,
        )
        .clamp(1, 100),
        max_attempts: env_u64(
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_MAX_ATTEMPTS,
        )
        .clamp(1, 25),
        failure_budget: env_u64(
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_FAILURE_BUDGET,
        )
        .clamp(1, 100),
        include_healthy_check: env_flag_enabled(CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV),
    })
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn maybe_spawn_content_repair_scheduler(registry: Arc<provider::ProviderRegistry>) {
    let Some(config) = content_repair_scheduler_config_from_env() else {
        tracing::debug!(
            "{} is disabled; content repair worker remains manual/operator-triggered",
            CONTENT_REPAIR_SCHEDULER_ENV
        );
        return;
    };
    tracing::info!(
        "content repair scheduler enabled: interval={}s limit={} max_attempts={} failure_budget={} include_healthy_check={}",
        config.interval_secs,
        config.limit,
        config.max_attempts,
        config.failure_budget,
        config.include_healthy_check,
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match invoke_content_repair_worker(&registry, config).await {
                Ok(response) => {
                    let data = response.get("data").unwrap_or(&response);
                    tracing::debug!(
                        "content repair scheduler run completed: checked={} repaired={} failed={} skipped={}",
                        data.get("checked")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                        data.get("repaired")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                        data.get("failed")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                        data.get("skipped")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                    );
                }
                Err(err) => {
                    tracing::warn!("content repair scheduler run failed: {}", err);
                }
            }
        }
    });
}

async fn invoke_content_repair_worker(
    registry: &provider::ProviderRegistry,
    config: ContentRepairSchedulerConfig,
) -> Result<serde_json::Value, provider::ProviderError> {
    registry
        .invoke_provider(ProviderInvocation {
            source: "content-provider".to_string(),
            target: "content".to_string(),
            op: "repair_worker".to_string(),
            request: serde_json::json!({
                "op": "repair_worker",
                "force": false,
                "include_healthy_check": config.include_healthy_check,
                "limit": config.limit,
                "max_attempts": config.max_attempts,
                "failure_budget": config.failure_budget,
            }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
}

fn exit_provider_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_EXIT_PROVIDER_CONFIG",
        "exit-provider.json",
    )
}

fn browser_engine_adapter_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG",
        "browser-engine-adapter.json",
    )
}

fn operator_drive_adapter_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG",
        "operator-drive-adapter.json",
    )
}

fn browser_local_exit_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
        "browser-local-exit.json",
    )
}

fn provider_config_from_env_or_file(
    data_dir: &std::path::Path,
    env_name: &str,
    file_name: &str,
) -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var(env_name) {
        return match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!("Ignoring invalid {} JSON: {}", env_name, err);
                None
            }
        };
    }
    let path = data_dir.join("config").join(file_name);
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(
                "Ignoring invalid provider config {}: {}",
                path.display(),
                err
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_provider_bridge(
        status: serde_json::Value,
        response_delay: Duration,
    ) -> (
        Arc<provider::ProviderBridge>,
        tokio::task::JoinHandle<Vec<serde_json::Value>>,
    ) {
        let (bridge_writer, provider_reader) = tokio::io::duplex(4096);
        let (provider_writer, bridge_reader) = tokio::io::duplex(4096);
        let bridge = Arc::new(provider::ProviderBridge::from_io(
            BufReader::new(bridge_reader),
            bridge_writer,
        ));
        let provider = tokio::spawn(async move {
            let mut reader = BufReader::new(provider_reader);
            let mut writer = provider_writer;
            let mut requests = Vec::new();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            requests.push(serde_json::from_str(line.trim()).unwrap());
            if !response_delay.is_zero() {
                tokio::time::sleep(response_delay).await;
            }
            writer
                .write_all(format!("{}\n", serde_json::to_string(&status).unwrap()).as_bytes())
                .await
                .unwrap();
            writer.flush().await.unwrap();

            line.clear();
            if reader.read_line(&mut line).await.unwrap() > 0 {
                requests.push(serde_json::from_str(line.trim()).unwrap());
                writer.write_all(b"{\"status\":\"ok\"}\n").await.unwrap();
                writer.flush().await.unwrap();
            }
            requests
        });
        (bridge, provider)
    }

    fn provider_status(provider_id: &str, protocol_version: &str) -> serde_json::Value {
        serde_json::json!({
            "status": "ok",
            "data": {
                "provider": provider_id,
                "protocol_version": protocol_version,
            }
        })
    }

    fn media_provider_status() -> serde_json::Value {
        serde_json::json!({
            "status": "ok",
            "data": {
                "provider": MEDIA_PROVIDER_ID,
                "protocol_version": MEDIA_PROVIDER_PROTOCOL_VERSION,
                "version": MEDIA_PROVIDER_VERSION,
                "configured": true,
                "supported_operations": ["status", "prepare"],
            }
        })
    }

    fn setup_model_provider_operator_root(tempdir: &TempDir) -> PathBuf {
        let root = model_provider_root_dir(tempdir.path());
        fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(
                tempdir.path().join("providers"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    fn write_model_provider_operator_config(tempdir: &TempDir, raw: &str) -> PathBuf {
        let root = setup_model_provider_operator_root(tempdir);
        let path = root.join(MODEL_PROVIDER_CONFIG_FILE_NAME);
        fs::write(&path, raw).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_media_provider_operator_config(
        tempdir: &TempDir,
        mutate: impl FnOnce(&mut serde_json::Value),
    ) -> PathBuf {
        let protected_content_root = tempdir.path().join("protected-content");
        let provider_root = media_provider_root_dir(tempdir.path());
        let tools_root = provider_root.join("tools");
        let staging_root = provider_root.join("staging");
        fs::create_dir_all(&tools_root).unwrap();
        fs::create_dir(&staging_root).unwrap();
        for path in [
            &protected_content_root,
            &provider_root,
            &tools_root,
            &staging_root,
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let ffmpeg_path = tools_root.join("ffmpeg");
        let ffprobe_path = tools_root.join("ffprobe");
        fs::write(&ffmpeg_path, b"ffmpeg-test").unwrap();
        fs::write(&ffprobe_path, b"ffprobe-test").unwrap();
        fs::set_permissions(&ffmpeg_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&ffprobe_path, fs::Permissions::from_mode(0o700)).unwrap();
        let mut value = serde_json::json!({
            "schema": MEDIA_PROVIDER_CONFIG_SCHEMA,
            "ffmpeg_path": ffmpeg_path,
            "ffprobe_path": ffprobe_path,
            "staging_root": staging_root,
            "output_profile": MEDIA_PROVIDER_OUTPUT_PROFILE,
            "timeout_ms": 5_000,
            "max_stdio_bytes": 4096,
            "max_input_bytes": 1 << 20,
            "max_output_part_bytes": 1 << 20,
            "max_duration_secs": 60,
            "max_source_width": 1920,
            "max_source_height": 1080,
            "max_source_fps": 60,
            "max_segment_count": 32,
            "max_total_output_bytes": 1 << 24,
        });
        mutate(&mut value);
        let config_path = media_provider_config_path(tempdir.path());
        fs::write(&config_path, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
        config_path
    }

    #[cfg(unix)]
    fn assert_pid_absent(pid: libc::pid_t) {
        let result = unsafe { libc::kill(pid, 0) };
        let err = std::io::Error::last_os_error();
        assert_eq!(result, -1, "pid {pid} should be absent");
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ESRCH),
            "pid {pid} should be reaped"
        );
    }

    #[tokio::test]
    async fn browser_engine_startup_registers_only_exact_identity_and_version() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status(BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        start_browser_engine_provider(&registry, bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let registration = registry
            .registration_for_uri("elastos://browser-engine/status")
            .await
            .unwrap();
        assert_eq!(registration.route, "browser-engine");
        assert_eq!(registration.provider, "capsule-provider");
        provider.abort();
    }

    #[tokio::test]
    async fn browser_engine_startup_reaps_old_or_mixed_version_before_launch() {
        for status in [
            provider_status(BROWSER_ENGINE_PROVIDER_ID, "1.0"),
            provider_status(BROWSER_ENGINE_PROVIDER_ID, "2.1"),
            provider_status("other-provider", BROWSER_ENGINE_PROTOCOL_VERSION),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            let error = start_browser_engine_provider(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(error.to_string().contains("unsupported"));
            assert!(registry
                .registration_for_uri("elastos://browser-engine/launch")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
            assert!(!requests.iter().any(|request| request["op"] == "launch"));
        }
    }

    #[tokio::test]
    async fn browser_engine_startup_reaps_failed_or_malformed_status() {
        for status in [
            serde_json::json!({"status": "error"}),
            serde_json::json!({"status": "ok", "data": []}),
            serde_json::json!({
                "status": "ok",
                "data": {
                    "provider": BROWSER_ENGINE_PROVIDER_ID,
                }
            }),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            start_browser_engine_provider(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(registry
                .registration_for_uri("elastos://browser-engine/status")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn browser_engine_status_probe_completes_within_its_bound() {
        let (bridge, provider) = test_provider_bridge(
            provider_status(BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let started = tokio::time::Instant::now();
        let error = request_browser_engine_provider_status(&bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(started.elapsed(), Duration::from_millis(5));
        provider.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn browser_engine_startup_reaps_provider_that_does_not_answer_in_time() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status(BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let error = start_browser_engine_provider(&registry, bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert!(registry
            .registration_for_uri("elastos://browser-engine/status")
            .await
            .is_none());
        let requests = provider.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
    }

    #[tokio::test]
    async fn browser_engine_startup_reaps_provider_when_registration_fails() {
        let registry = provider::ProviderRegistry::new();
        let (first_bridge, first_provider) = test_provider_bridge(
            provider_status(BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        start_browser_engine_provider(&registry, first_bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let (second_bridge, second_provider) = test_provider_bridge(
            provider_status(BROWSER_ENGINE_PROVIDER_ID, BROWSER_ENGINE_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        let error = start_browser_engine_provider(&registry, second_bridge, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failed to register"));

        let requests = second_provider.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
        first_provider.abort();
    }

    #[tokio::test]
    async fn wallet_provider_v2_startup_registers_only_exact_identity_and_version() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status("wallet-provider", WALLET_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        start_wallet_provider_v2(&registry, bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let registration = registry
            .registration_for_uri("elastos://wallet/status")
            .await
            .unwrap();
        assert_eq!(registration.route, "wallet");
        assert_eq!(registration.provider, "capsule-provider");
        provider.abort();
    }

    #[tokio::test]
    async fn wallet_provider_v2_startup_reaps_identity_or_version_mismatch() {
        for status in [
            provider_status("wallet-provider", "2.0"),
            provider_status("wallet-provider", "2.1"),
            provider_status("wallet-provider", "2.2"),
            provider_status("wallet-provider", "1.0"),
            provider_status("other-provider", WALLET_PROTOCOL_VERSION),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            let error = start_wallet_provider_v2(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(error.to_string().contains("unsupported"));
            assert!(registry
                .registration_for_uri("elastos://wallet/status")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test]
    async fn wallet_provider_v2_startup_reaps_failed_or_malformed_status() {
        for status in [
            serde_json::json!({"status": "error"}),
            serde_json::json!({"status": "ok", "data": []}),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            start_wallet_provider_v2(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(registry
                .registration_for_uri("elastos://wallet/status")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wallet_provider_v2_status_probe_completes_within_its_bound() {
        let (bridge, provider) = test_provider_bridge(
            provider_status("wallet-provider", WALLET_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let started = tokio::time::Instant::now();
        let error = request_wallet_provider_v2_status(&bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(started.elapsed(), Duration::from_millis(5));
        provider.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn wallet_provider_v2_startup_reaps_provider_that_does_not_answer_in_time() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status("wallet-provider", WALLET_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let error = start_wallet_provider_v2(&registry, bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert!(registry
            .registration_for_uri("elastos://wallet/status")
            .await
            .is_none());
        let requests = provider.await.unwrap();
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
    }

    #[tokio::test]
    async fn model_provider_startup_registers_only_exact_identity_and_version() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status(MODEL_PROVIDER_ID, MODEL_PROVIDER_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        start_model_provider(&registry, bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let registration = registry
            .registration_for_uri("elastos://model/offers")
            .await
            .unwrap();
        assert_eq!(registration.route, "model");
        assert_eq!(registration.provider, "capsule-provider");
        provider.abort();
    }

    #[tokio::test]
    async fn model_provider_startup_reaps_identity_or_version_mismatch() {
        for status in [
            provider_status(MODEL_PROVIDER_ID, "1.0"),
            provider_status(MODEL_PROVIDER_ID, "2.0"),
            provider_status("other-provider", MODEL_PROVIDER_PROTOCOL_VERSION),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            let error = start_model_provider(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(error.to_string().contains("unsupported"));
            assert!(registry
                .registration_for_uri("elastos://model/offers")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test]
    async fn model_provider_startup_reaps_failed_or_malformed_status() {
        for status in [
            serde_json::json!({"status": "error"}),
            serde_json::json!({"status": "ok", "data": []}),
            serde_json::json!({
                "status": "ok",
                "data": {
                    "provider": MODEL_PROVIDER_ID,
                }
            }),
        ] {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);
            start_model_provider(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(registry
                .registration_for_uri("elastos://model/offers")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn model_provider_status_probe_completes_within_its_bound() {
        let (bridge, provider) = test_provider_bridge(
            provider_status(MODEL_PROVIDER_ID, MODEL_PROVIDER_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let started = tokio::time::Instant::now();
        let error = request_model_provider_status(&bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(started.elapsed(), Duration::from_millis(5));
        provider.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn model_provider_startup_reaps_provider_that_does_not_answer_in_time() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(
            provider_status(MODEL_PROVIDER_ID, MODEL_PROVIDER_PROTOCOL_VERSION),
            Duration::from_millis(20),
        );
        let error = start_model_provider(&registry, bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert!(registry
            .registration_for_uri("elastos://model/offers")
            .await
            .is_none());
        let requests = provider.await.unwrap();
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
    }

    #[tokio::test]
    async fn model_provider_startup_reaps_provider_when_registration_fails() {
        let registry = provider::ProviderRegistry::new();
        let (first_bridge, first_provider) = test_provider_bridge(
            provider_status(MODEL_PROVIDER_ID, MODEL_PROVIDER_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        start_model_provider(&registry, first_bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let (second_bridge, second_provider) = test_provider_bridge(
            provider_status(MODEL_PROVIDER_ID, MODEL_PROVIDER_PROTOCOL_VERSION),
            Duration::ZERO,
        );
        let error = start_model_provider(&registry, second_bridge, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failed to register"));

        let requests = second_provider.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
        first_provider.abort();
    }

    #[tokio::test]
    async fn media_provider_startup_registers_only_the_runtime_media_route() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) = test_provider_bridge(media_provider_status(), Duration::ZERO);

        start_media_provider(&registry, bridge, Duration::from_secs(1))
            .await
            .unwrap();

        let registration = registry
            .registration_for_uri("elastos://media/status")
            .await
            .unwrap();
        assert_eq!(registration.route, MEDIA_PROVIDER_ROUTE);
        assert_eq!(registration.provider, "capsule-provider");
        provider.abort();
    }

    #[tokio::test]
    async fn media_provider_startup_reaps_mismatched_or_unconfigured_processes() {
        let mut cases = Vec::new();
        let mut wrong_identity = media_provider_status();
        wrong_identity["data"]["provider"] = serde_json::json!("other-provider");
        cases.push(wrong_identity);
        let mut wrong_protocol = media_provider_status();
        wrong_protocol["data"]["protocol_version"] = serde_json::json!("old");
        cases.push(wrong_protocol);
        let mut wrong_version = media_provider_status();
        wrong_version["data"]["version"] = serde_json::json!("old");
        cases.push(wrong_version);
        let mut unconfigured = media_provider_status();
        unconfigured["data"]["configured"] = serde_json::json!(false);
        cases.push(unconfigured);
        let mut wrong_operations = media_provider_status();
        wrong_operations["data"]["supported_operations"] =
            serde_json::json!(["status", "prepare", "publish"]);
        cases.push(wrong_operations);

        for status in cases {
            let registry = provider::ProviderRegistry::new();
            let (bridge, provider) = test_provider_bridge(status, Duration::ZERO);

            start_media_provider(&registry, bridge, Duration::from_secs(1))
                .await
                .unwrap_err();

            assert!(registry
                .registration_for_uri("elastos://media/status")
                .await
                .is_none());
            let requests = provider.await.unwrap();
            assert_eq!(requests[0]["op"], "status");
            assert_eq!(requests[1]["op"], "shutdown");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn media_provider_status_and_failed_startup_are_bounded_and_settled() {
        let registry = provider::ProviderRegistry::new();
        let (bridge, provider) =
            test_provider_bridge(media_provider_status(), Duration::from_millis(20));
        let started = tokio::time::Instant::now();

        let error = start_media_provider(&registry, bridge, Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(started.elapsed(), Duration::from_millis(20));
        assert!(registry
            .registration_for_uri("elastos://media/status")
            .await
            .is_none());
        let requests = provider.await.unwrap();
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
    }

    #[tokio::test]
    async fn media_provider_duplicate_registration_reaps_the_rejected_process() {
        let registry = provider::ProviderRegistry::new();
        let (first_bridge, first_provider) =
            test_provider_bridge(media_provider_status(), Duration::ZERO);
        start_media_provider(&registry, first_bridge, Duration::from_secs(1))
            .await
            .unwrap();
        let (second_bridge, second_provider) =
            test_provider_bridge(media_provider_status(), Duration::ZERO);

        let error = start_media_provider(&registry, second_bridge, Duration::from_secs(1))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("failed to register"));
        let requests = second_provider.await.unwrap();
        assert_eq!(requests[0]["op"], "status");
        assert_eq!(requests[1]["op"], "shutdown");
        first_provider.abort();
    }

    #[cfg(unix)]
    #[test]
    fn media_provider_config_is_private_bounded_and_path_fixed() {
        let absent = TempDir::new().unwrap();
        assert!(media_provider_bridge_config(absent.path())
            .unwrap()
            .is_none());

        let valid = TempDir::new().unwrap();
        let path = write_media_provider_operator_config(&valid, |_| {});
        let original = fs::read(&path).unwrap();
        let config = media_provider_bridge_config(valid.path()).unwrap().unwrap();
        assert_eq!(config.extra["provider_id"], MEDIA_PROVIDER_ID);
        let expected_staging = media_provider_root_dir(valid.path()).join("staging");
        assert_eq!(
            config.extra["staging_root"].as_str(),
            expected_staging.to_str()
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        let invalid_cases: [fn(&mut serde_json::Value); 3] = [
            |value: &mut serde_json::Value| {
                value["timeout_ms"] = serde_json::json!(MEDIA_PROVIDER_MAX_TIMEOUT_MS + 1)
            },
            |value: &mut serde_json::Value| {
                value["max_input_bytes"] = serde_json::json!(MEDIA_PROVIDER_MAX_INPUT_BYTES + 1)
            },
            |value: &mut serde_json::Value| {
                value["staging_root"] = serde_json::json!("/private/tmp/escaped")
            },
        ];
        for mutate in invalid_cases {
            let tempdir = TempDir::new().unwrap();
            write_media_provider_operator_config(&tempdir, mutate);
            assert!(media_provider_bridge_config(tempdir.path()).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn media_provider_config_rejects_links_and_redacts_private_values() {
        let tempdir = TempDir::new().unwrap();
        let config_path = write_media_provider_operator_config(&tempdir, |value| {
            value["secret"] = serde_json::json!("super-secret-private-path")
        });
        let error = media_provider_bridge_config(tempdir.path()).unwrap_err();
        assert!(!error.to_string().contains("super-secret-private-path"));
        fs::remove_file(&config_path).unwrap();

        let target = tempdir.path().join("linked-config");
        fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(&target, &config_path).unwrap();
        assert!(media_provider_bridge_config(tempdir.path()).is_err());
    }

    #[test]
    fn model_provider_bridge_config_uses_empty_offers_when_config_is_absent() {
        let tempdir = TempDir::new().unwrap();

        let config = model_provider_bridge_config(tempdir.path()).unwrap();

        assert_eq!(config.base_path, tempdir.path().to_string_lossy());
        assert_eq!(config.extra["provider_id"], MODEL_PROVIDER_ID);
        assert_eq!(
            config.extra["journal_dir"],
            model_provider_journal_dir(tempdir.path())
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(config.extra["offers"], serde_json::json!([]));
        assert!(!model_provider_config_path(tempdir.path()).exists());
    }

    #[test]
    fn model_provider_bridge_config_passes_raw_operator_offers_without_nested_validation() {
        let tempdir = TempDir::new().unwrap();
        let raw = serde_json::json!({
            "offers": [
                {
                    "id": "offer:flash-chat:pair-a",
                    "title": "Flash chat",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "policy": {
                        "concurrency_limit": 1,
                        "input_bytes_limit": 1024,
                        "inline_output_bytes_limit": 2048,
                        "event_bytes_limit": 4096,
                        "runtime_ms_limit": 1000,
                        "retention_secs": 60,
                        "cancel_settlement_timeout_ms": 1000
                    },
                    "adapter": {
                        "kind": "open_ai_compatible_text",
                        "api_url": "https://example.invalid/v1/chat/completions",
                        "api_key": "Bearer super-secret",
                        "model": "pair-a"
                    }
                },
                {
                    "id": "offer:nested-invalid",
                    "title": "Broken nested config",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "policy": {
                        "concurrency_limit": 1,
                        "input_bytes_limit": 1024,
                        "inline_output_bytes_limit": 2048,
                        "event_bytes_limit": 4096,
                        "runtime_ms_limit": 1000,
                        "retention_secs": 60,
                        "cancel_settlement_timeout_ms": 1000
                    },
                    "adapter": {
                        "kind": "open_ai_compatible_text"
                    }
                }
            ]
        });
        write_model_provider_operator_config(&tempdir, &raw.to_string());

        let config = model_provider_bridge_config(tempdir.path()).unwrap();

        assert_eq!(config.extra["offers"], raw["offers"]);
    }

    #[test]
    fn model_provider_bridge_config_rejects_invalid_top_level_shape_without_echoing_secrets() {
        let tempdir = TempDir::new().unwrap();
        let cases = [
            r#"{"offers":[],"extra":"nope"}"#,
            r#"{"offers":[],"offers":[]}"#,
            r#"{"offers":{"id":"not-an-array"}}"#,
            r#"["not-an-object"]"#,
        ];
        for raw in cases {
            write_model_provider_operator_config(&tempdir, raw);
            let error = model_provider_bridge_config(tempdir.path()).unwrap_err();
            assert!(!error.to_string().contains("super-secret"));
            fs::remove_file(model_provider_config_path(tempdir.path())).unwrap();
        }
        write_model_provider_operator_config(
            &tempdir,
            r#"{"offers":[],"authorization":"Bearer super-secret"}"#,
        );
        let error = model_provider_bridge_config(tempdir.path()).unwrap_err();
        assert!(!error.to_string().contains("Bearer super-secret"));
    }

    #[test]
    fn model_provider_bridge_config_has_no_env_fallback_and_is_read_only() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_MODEL_PROVIDER_CONFIG",
            "ELASTOS_MODEL_PROVIDER_CONFIG_PATH",
        ]);
        std::env::set_var(
            "ELASTOS_MODEL_PROVIDER_CONFIG",
            r#"{"offers":[{"id":"offer:env"}]}"#,
        );
        std::env::set_var("ELASTOS_MODEL_PROVIDER_CONFIG_PATH", "/tmp/not-used.json");
        let tempdir = TempDir::new().unwrap();

        let config = model_provider_bridge_config(tempdir.path()).unwrap();

        assert_eq!(config.extra["offers"], serde_json::json!([]));
        assert!(!model_provider_config_path(tempdir.path()).exists());
    }

    #[test]
    fn model_provider_bridge_config_repeated_reads_leave_operator_config_unchanged() {
        let tempdir = TempDir::new().unwrap();
        let path = write_model_provider_operator_config(&tempdir, r#"{"offers":[]}"#);
        let original_bytes = fs::read(&path).unwrap();
        let original_metadata = fs::metadata(&path).unwrap();

        let first = model_provider_bridge_config(tempdir.path()).unwrap();
        let second = model_provider_bridge_config(tempdir.path()).unwrap();

        assert_eq!(first.extra["offers"], serde_json::json!([]));
        assert_eq!(second.extra["offers"], serde_json::json!([]));
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        let final_metadata = fs::metadata(&path).unwrap();
        assert_eq!(final_metadata.len(), original_metadata.len());
        #[cfg(unix)]
        assert_eq!(
            final_metadata.permissions().mode() & 0o777,
            original_metadata.permissions().mode() & 0o777
        );
    }

    #[test]
    fn model_provider_bridge_config_rejects_oversized_operator_config() {
        let tempdir = TempDir::new().unwrap();
        let oversized = format!(
            "{{\"offers\":[],\"padding\":\"{}\"}}",
            "x".repeat(MODEL_PROVIDER_CONFIG_MAX_BYTES)
        );
        write_model_provider_operator_config(&tempdir, &oversized);

        let error = model_provider_bridge_config(tempdir.path()).unwrap_err();

        assert!(error.to_string().contains("byte limit"));
    }

    #[cfg(unix)]
    #[test]
    fn model_provider_bridge_config_rejects_insecure_or_nonregular_operator_config() {
        use std::os::unix::fs::symlink;

        let tempdir = TempDir::new().unwrap();
        let path = write_model_provider_operator_config(&tempdir, r#"{"offers":[]}"#);

        fs::set_permissions(
            tempdir.path().join("providers"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(model_provider_bridge_config(tempdir.path()).is_err());
        fs::set_permissions(
            tempdir.path().join("providers"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();

        fs::set_permissions(
            model_provider_root_dir(tempdir.path()),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(model_provider_bridge_config(tempdir.path()).is_err());
        fs::set_permissions(
            model_provider_root_dir(tempdir.path()),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(model_provider_bridge_config(tempdir.path()).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::remove_file(&path).unwrap();
        symlink(tempdir.path().join("missing.json"), &path).unwrap();
        assert!(model_provider_bridge_config(tempdir.path()).is_err());
    }

    #[test]
    fn setup_source_home_does_not_seed_model_provider_operator_config() {
        let script = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join("scripts")
                .join("setup-source-home.sh"),
        )
        .unwrap();

        assert!(!script.contains("providers/model-provider/config.json"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nested_model_provider_config_reaches_spawn_and_provider_exits_without_registration() {
        let tempdir = TempDir::new().unwrap();
        write_model_provider_operator_config(
            &tempdir,
            &serde_json::json!({
                "offers": [{
                    "id": "offer:nested-invalid",
                    "title": "Broken",
                    "operation": "text.generate",
                    "input_modalities": ["text/plain"],
                    "output_modalities": ["text/plain"],
                    "policy": {
                        "concurrency_limit": 1,
                        "input_bytes_limit": 1024,
                        "inline_output_bytes_limit": 2048,
                        "event_bytes_limit": 4096,
                        "runtime_ms_limit": 1000,
                        "retention_secs": 60,
                        "cancel_settlement_timeout_ms": 1000
                    },
                    "adapter": {
                        "kind": "open_ai_compatible_text"
                    }
                }]
            })
            .to_string(),
        );
        let script_path = tempdir.path().join("fake-model-provider.sh");
        let pid_path = tempdir.path().join("fake-model-provider.pid");
        fs::write(
            &script_path,
            format!(
                "#!/bin/sh\nprintf '%s' $$ > '{}'\nIFS= read -r _line || exit 0\nprintf '{{\"status\":\"error\",\"code\":\"invalid_config\",\"message\":\"invalid configuration\"}}\\n'\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
                pid_path.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700)).unwrap();

        let error = match provider::ProviderBridge::spawn(
            &script_path,
            model_provider_bridge_config(tempdir.path()).unwrap(),
        )
        .await
        {
            Ok(_) => panic!("nested invalid config should not initialize a provider bridge"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("invalid configuration"));
        let pid = fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        assert_pid_absent(pid);
    }

    struct EnvGuard {
        keys: &'static [&'static str],
    }

    impl EnvGuard {
        fn new(keys: &'static [&'static str]) -> Self {
            for key in keys {
                std::env::remove_var(key);
            }
            Self { keys }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in self.keys {
                std::env::remove_var(key);
            }
        }
    }

    #[test]
    fn availability_provider_config_uses_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            "ELASTOS_AVAILABILITY_ENSURE_URL",
        ]);
        std::env::set_var(
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            r#"{"targets":[{"id":"test","ensure_url":"https://example.invalid/ensure"}]}"#,
        );

        let config = availability_provider_config_from_env().unwrap();
        assert_eq!(config["targets"][0]["id"], "test");
    }

    #[test]
    fn availability_provider_config_uses_env_endpoint() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            "ELASTOS_AVAILABILITY_ENSURE_URL",
            "ELASTOS_AVAILABILITY_PROVIDER_ID",
            "ELASTOS_AVAILABILITY_AUTHORIZATION",
        ]);
        std::env::set_var(
            "ELASTOS_AVAILABILITY_ENSURE_URL",
            "https://example.invalid/ensure",
        );
        std::env::set_var("ELASTOS_AVAILABILITY_PROVIDER_ID", "elacity");
        std::env::set_var("ELASTOS_AVAILABILITY_AUTHORIZATION", "Bearer secret");

        let config = availability_provider_config_from_env().unwrap();
        assert_eq!(config["targets"][0]["id"], "elacity");
        assert_eq!(
            config["targets"][0]["ensure_url"],
            "https://example.invalid/ensure"
        );
        assert_eq!(config["targets"][0]["authorization"], "Bearer secret");
    }

    #[test]
    fn carrier_peer_attestation_exchange_config_uses_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
        ]);
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            r#"{"url":"https://attest.example.invalid/exchange","authorization":"Bearer attest","timeout_secs":8}"#,
        );

        let config = carrier_peer_attestation_exchange_config_from_env().unwrap();
        assert_eq!(config["url"], "https://attest.example.invalid/exchange");
        assert_eq!(config["authorization"], "Bearer attest");
        assert_eq!(config["timeout_secs"], 8);
    }

    #[test]
    fn carrier_peer_attestation_exchange_config_uses_env_endpoint() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS",
        ]);
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
            "http://127.0.0.1:9799/peer-attestation/exchange",
        );
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION",
            "Bearer local-attest",
        );
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS",
            "10",
        );

        let config = carrier_peer_attestation_exchange_config_from_env().unwrap();
        assert_eq!(
            config["url"],
            "http://127.0.0.1:9799/peer-attestation/exchange"
        );
        assert_eq!(config["authorization"], "Bearer local-attest");
        assert_eq!(config["timeout_secs"], 10);
    }

    #[test]
    fn content_repair_scheduler_is_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            CONTENT_REPAIR_SCHEDULER_ENV,
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV,
        ]);

        assert!(content_repair_scheduler_config_from_env().is_none());
    }

    #[test]
    fn content_repair_scheduler_config_clamps_operator_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            CONTENT_REPAIR_SCHEDULER_ENV,
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV,
        ]);
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_ENV, "true");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV, "5");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_LIMIT_ENV, "1000");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV, "999");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV, "999");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV, "yes");

        let config = content_repair_scheduler_config_from_env().unwrap();
        assert_eq!(
            config.interval_secs,
            CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS
        );
        assert_eq!(config.limit, 100);
        assert_eq!(config.max_attempts, 25);
        assert_eq!(config.failure_budget, 100);
        assert!(config.include_healthy_check);
    }

    #[test]
    fn exit_provider_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_EXIT_PROVIDER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_EXIT_PROVIDER_CONFIG",
            r#"{"backends":[{"id":"local","kind":"http_fetch","allowed_hosts":["example.com"]}]}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = exit_provider_config_from_env(&data_dir).unwrap();
        assert_eq!(config["backends"][0]["id"], "local");
        assert_eq!(config["backends"][0]["kind"], "http_fetch");
    }

    #[test]
    fn browser_engine_adapter_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG",
            r#"{"adapters":[{"id":"linux-proof","kind":"contract_proof","display_modes":["webrtc_remote_display"]}]}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = browser_engine_adapter_config_from_env(&data_dir).unwrap();
        assert_eq!(config["adapters"][0]["id"], "linux-proof");
        assert_eq!(config["adapters"][0]["kind"], "contract_proof");
        assert_eq!(
            config["adapters"][0]["display_modes"][0],
            "webrtc_remote_display"
        );
    }

    #[test]
    fn operator_drive_adapter_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG",
            r#"{"operator_endpoint":{"url":"http://127.0.0.1:9797/operator-drive","authorization":"Bearer secret"}}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = operator_drive_adapter_config_from_env(&data_dir).unwrap();
        assert_eq!(
            config["operator_endpoint"]["url"],
            "http://127.0.0.1:9797/operator-drive"
        );
        assert_eq!(
            config["operator_endpoint"]["authorization"],
            "Bearer secret"
        );
    }

    #[test]
    fn browser_local_exit_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_BROWSER_LOCAL_EXIT_CONFIG"]);
        std::env::set_var(
            "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
            r#"{"schema":"elastos.browser.local-exit.config/v1","relay_ipc_path":"/tmp/elastos-browser-local-exit.sock","allowed_hosts":["*"],"replace_existing_socket":true}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = browser_local_exit_config_from_env(&data_dir).unwrap();
        assert_eq!(config["schema"], "elastos.browser.local-exit.config/v1");
        assert_eq!(config["allowed_hosts"][0], "*");
    }

    #[test]
    fn browser_local_exit_relay_path_rejects_relative_and_whitespace_paths() {
        let relative = serde_json::json!({
            "relay_ipc_path": "relative.sock"
        });
        assert!(browser_local_exit_relay_path(&relative).is_err());

        let whitespace = serde_json::json!({
            "relay_ipc_path": "/tmp/elastos browser.sock"
        });
        assert!(browser_local_exit_relay_path(&whitespace).is_err());
    }

    #[test]
    fn browser_local_exit_replace_existing_socket_refuses_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let relay_path = dir.path().join("relay.sock");
        std::fs::write(&relay_path, b"not a socket").unwrap();

        let err = remove_existing_browser_local_exit_socket(&relay_path).unwrap_err();
        assert!(
            err.to_string().contains("not a Unix socket")
                || err
                    .to_string()
                    .contains("replace_existing_socket is only supported")
        );
        assert!(relay_path.exists());
    }

    /// Bindable socket paths must stay under `SUN_LEN`, which `tempfile::tempdir`
    /// paths can overrun on macOS.
    #[cfg(unix)]
    fn bindable_relay_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ele-srv-{}-{tag}.sock", std::process::id()))
    }

    #[cfg(unix)]
    #[test]
    fn browser_local_exit_replace_existing_socket_removes_a_socket_with_no_listener() {
        let relay_path = bindable_relay_path("stale");
        let _ = std::fs::remove_file(&relay_path);
        let listener = std::os::unix::net::UnixListener::bind(&relay_path).unwrap();
        // Rust does not unlink the path on drop, so this leaves the exact shape a
        // hard-killed helper leaves behind: a socket file with nothing serving it.
        drop(listener);
        assert!(relay_path.exists());

        remove_existing_browser_local_exit_socket(&relay_path).unwrap();
        assert!(!relay_path.exists(), "stale relay socket must be reclaimed");
    }

    #[cfg(unix)]
    #[test]
    fn browser_local_exit_replace_existing_socket_refuses_a_live_listener() {
        let relay_path = bindable_relay_path("live");
        let _ = std::fs::remove_file(&relay_path);
        let listener = std::os::unix::net::UnixListener::bind(&relay_path).unwrap();

        // Silently unlinking here is what stranded helpers one per launch: the
        // incumbent keeps serving an unlinked socket that nothing can reach again.
        let err = remove_existing_browser_local_exit_socket(&relay_path).unwrap_err();
        assert!(
            err.to_string().contains("still served by a live helper"),
            "unexpected error: {err}"
        );
        assert!(relay_path.exists(), "a live helper's socket must survive");

        drop(listener);
        let _ = std::fs::remove_file(&relay_path);
    }

    #[test]
    fn browser_local_exit_socket_ready_requires_socket() {
        let dir = tempfile::tempdir().unwrap();
        let relay_path = dir.path().join("relay.sock");
        assert!(!browser_local_exit_socket_ready(&relay_path).unwrap());
        std::fs::write(&relay_path, b"not a socket").unwrap();
        #[cfg(unix)]
        {
            let err = browser_local_exit_socket_ready(&relay_path).unwrap_err();
            assert!(err.to_string().contains("not a Unix socket"), "{err}");
        }
        #[cfg(not(unix))]
        {
            assert!(browser_local_exit_socket_ready(&relay_path).unwrap());
        }
    }

    #[tokio::test]
    async fn carrier_runtime_service_shutdown_releases_endpoint_for_restart() {
        let dir = tempfile::tempdir().unwrap();
        let device_key = elastos_identity::load_or_create_device_key(dir.path()).unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&device_key);

        let node = elastos_server::carrier::start_carrier_node_with_registry(
            &signing_key,
            &did,
            dir.path().to_path_buf(),
            None,
        )
        .await
        .unwrap();
        assert!(
            !node.endpoint.bound_sockets().is_empty(),
            "carrier endpoint should be bound before readiness is published"
        );

        let mut service = elastos_server::carrier::CarrierRuntimeService::new(node);
        service.shutdown().await.unwrap();

        let restarted = elastos_server::carrier::start_carrier_node_with_registry(
            &signing_key,
            &did,
            dir.path().to_path_buf(),
            None,
        )
        .await
        .unwrap();
        let mut restarted_service = elastos_server::carrier::CarrierRuntimeService::new(restarted);
        restarted_service.shutdown().await.unwrap();
    }
}
