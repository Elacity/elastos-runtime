use anyhow::Context;
use elastos_common::localhost::{publisher_publish_state_path, publisher_root_path};
use elastos_common::{CapsuleManifest, RequirementKind};
use elastos_runtime::signature::{self, generate_keypair};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_PUBLISH_CAPSULES: &[&str] = &[
    "shell",
    "localhost-provider",
    "chat",
    "chat-wasm",
    "did-provider",
    "tunnel-provider",
];
const REQUIRED_SUPPORTED_PUBLISH_CAPSULES: &[&str] =
    &["shell", "localhost-provider", "chat", "did-provider"];
const ALLOWED_RELEASE_CHANNELS: &[&str] = &["stable", "canary", "jetson-test"];

pub(crate) fn source_discovery_uri(publisher_did: &str, channel: &str) -> String {
    let channel = normalize_release_channel(channel);
    let mut hasher = sha2::Sha256::new();
    hasher.update(publisher_did.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("elastos://source/{}/{}", channel, &digest[..32])
}

fn release_discovery_topic_for_uri(discovery_uri: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(discovery_uri.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("elastos:source:{}", &digest[..32])
}

pub(crate) fn release_discovery_topics(
    discovery_uri: Option<&str>,
    publisher_did: &str,
    channel: &str,
) -> Vec<String> {
    let discovery_uri = discovery_uri
        .filter(|uri| !uri.trim().is_empty())
        .map(|uri| uri.trim().to_string())
        .unwrap_or_else(|| source_discovery_uri(publisher_did, channel));
    let channel = normalize_release_channel(channel);
    let mut hasher = sha2::Sha256::new();
    hasher.update(publisher_did.as_bytes());
    let digest = hex::encode(hasher.finalize());
    let specific = format!("elastos:releases:{}:{}", channel, &digest[..32]);
    vec![
        release_discovery_topic_for_uri(&discovery_uri),
        specific,
        "elastos:releases".to_string(),
    ]
}

fn normalize_release_channel(channel: &str) -> String {
    let mut normalized = String::new();
    for ch in channel.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            normalized.push(ch);
        } else {
            normalized.push('-');
        }
    }
    if normalized.is_empty() {
        "stable".to_string()
    } else {
        normalized
    }
}

fn validate_release_channel(channel: &str) -> anyhow::Result<()> {
    if ALLOWED_RELEASE_CHANNELS.contains(&channel) {
        Ok(())
    } else {
        anyhow::bail!(
            "Unsupported release channel '{}'. Allowed channels: {}",
            channel,
            ALLOWED_RELEASE_CHANNELS.join(", ")
        );
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PublishReleaseOptions {
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) profile: String,
    pub(crate) skip_build: bool,
    pub(crate) skip_rootfs: bool,
    pub(crate) cross: Option<String>,
    pub(crate) capsules: Vec<String>,
    pub(crate) key: Option<PathBuf>,
    pub(crate) dry_run: bool,
    pub(crate) preflight_only: bool,
    pub(crate) public_url: bool,
    pub(crate) public_with_sudo: bool,
    pub(crate) gateway_addr: String,
    pub(crate) public_timeout: u64,
    pub(crate) ipfs_provider_bin: Option<PathBuf>,
    pub(crate) allow_no_bootstrap: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct PublishState {
    #[serde(default)]
    last_release_cid: Option<String>,
    #[serde(default)]
    last_head_cid: Option<String>,
    #[serde(default)]
    last_version: Option<String>,
    #[serde(default)]
    last_published_at: Option<u64>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ReleaseLedger {
    #[serde(default = "default_release_ledger_schema")]
    schema: String,
    #[serde(default)]
    entries: Vec<ReleaseLedgerEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ReleaseLedgerEntry {
    version: String,
    channel: String,
    release_cid: String,
    head_cid: String,
    published_at: u64,
    signer_did: String,
    selected_capsules: Vec<String>,
    platforms: BTreeMap<String, ReleaseLedgerPlatform>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ReleaseLedgerPlatform {
    binary_cid: String,
    components_cid: String,
    capsules: BTreeMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseEnvelope {
    payload: ReleasePayload,
    signer_did: String,
}

#[derive(Debug, serde::Deserialize)]
struct ReleasePayload {
    channel: String,
    version: String,
    released_at: u64,
    platforms: BTreeMap<String, ReleasePlatformEnvelope>,
}

#[derive(Debug, serde::Deserialize)]
struct ReleasePlatformEnvelope {
    binary: ReleaseArtifactRef,
    components: ReleaseArtifactRef,
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseArtifactRef {
    cid: String,
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseHeadEnvelope {
    payload: ReleaseHeadPayload,
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseHeadPayload {
    latest_release_cid: String,
}

#[derive(Debug, Clone)]
struct PublishKey {
    path: PathBuf,
    signer_did: String,
}

#[derive(Debug, Clone)]
struct PublishKeyPreview {
    path: PathBuf,
    signer_did: Option<String>,
}

pub(crate) async fn run_publish_release(options: PublishReleaseOptions) -> anyhow::Result<()> {
    let workspace_root = workspace_root();
    let data_dir = elastos_server::sources::default_data_dir();
    let state_path = publish_state_path(&data_dir);
    let previous_state = load_publish_state(&state_path)?;
    let manifests = load_capsule_manifests(&workspace_root)?;
    let available_capsules = manifests.keys().cloned().collect::<Vec<_>>();
    let selected_capsules = select_capsules(&options.profile, &options.capsules, &manifests)?;
    validate_publish_inputs(&options, &workspace_root, &selected_capsules)?;

    if options.dry_run {
        let key = inspect_release_key(options.key.as_deref())?;
        print_publish_plan(
            &options,
            &key,
            &state_path,
            &previous_state,
            &available_capsules,
            &selected_capsules,
        );
        return Ok(());
    }

    let preflight = run_publish_preflight(&options, &workspace_root, &selected_capsules)?;
    if options.preflight_only {
        print_preflight_report(&options, &preflight);
        return Ok(());
    }

    let key = load_or_create_release_key(options.key.as_deref())?;
    let source_bootstrap = discover_source_connect_ticket().await;
    if bootstrap_required(&options, &source_bootstrap) {
        anyhow::bail!(
            "publish requires a stamped trusted-source Carrier bootstrap; start or refresh a local ElastOS runtime first, or re-run with --allow-no-bootstrap only for local-only testing"
        );
    }
    let script_path = workspace_root.join("scripts/publish-release.sh");
    if !script_path.is_file() {
        anyhow::bail!("Publish script not found at {}", script_path.display());
    }

    let state_dir = publish_state_dir(&data_dir);
    std::fs::create_dir_all(&state_dir)?;

    println!("ElastOS publish-release");
    println!("  Version:   {}", options.version);
    println!("  Channel:   {}", options.channel);
    println!("  Profile:   {}", options.profile);
    println!("  Signer:    {}", key.signer_did);
    match &source_bootstrap {
        Ok(Some(_)) => println!("  Bootstrap: publisher ticket auto-stamped"),
        Ok(None) => println!("  Bootstrap: unavailable (no running local runtime ticket)"),
        Err(error) => println!("  Bootstrap: unavailable ({})", error),
    }
    println!("  Capsules:  {}", selected_capsules.join(", "));
    if let Some(cross) = &options.cross {
        println!("  Cross:     {}", cross);
    }
    println!("  Key path:  {}", key.path.display());
    println!("  State dir: {}", state_dir.display());
    println!();

    let mut cmd = Command::new("bash");
    cmd.arg(script_path);
    cmd.arg("--version").arg(&options.version);
    cmd.arg("--channel").arg(&options.channel);
    cmd.arg("--key").arg(&key.path);
    cmd.arg("--capsules").arg(selected_capsules.join(","));
    cmd.env("ELASTOS_PUBLISH_STATE_DIR", &state_dir);
    if let Ok(Some(ticket)) = &source_bootstrap {
        cmd.env("ELASTOS_SOURCE_CONNECT_TICKET", ticket);
    }
    if options.allow_no_bootstrap {
        cmd.env("ELASTOS_ALLOW_NO_BOOTSTRAP", "1");
    }
    cmd.current_dir(&workspace_root);

    if options.skip_build {
        cmd.arg("--skip-build");
    }
    if options.skip_rootfs {
        cmd.arg("--skip-rootfs");
    }
    if let Some(cross) = &options.cross {
        cmd.arg("--cross").arg(cross);
    }
    if let Some(ipfs_provider_bin) = &options.ipfs_provider_bin {
        cmd.arg("--ipfs-provider-bin").arg(ipfs_provider_bin);
    }
    if options.public_url {
        if options.public_with_sudo {
            cmd.arg("--public-with-sudo");
        }
        cmd.arg("--gateway-addr").arg(&options.gateway_addr);
        cmd.arg("--public-timeout")
            .arg(options.public_timeout.to_string());
    } else {
        cmd.arg("--no-public-url");
    }

    let status = cmd
        .status()
        .context("Failed to launch publish-release.sh")?;
    if !status.success() {
        anyhow::bail!("publish-release.sh exited with status {}", status);
    }

    // Re-verify signatures on artifacts produced by the bash script.
    // Consumer-side verification is the primary trust boundary, but this
    // defense-in-depth check catches corrupt or unsigned artifacts before
    // they are announced to peers.
    let artifacts_dir = workspace_root.join("artifacts");
    let release_json_path = artifacts_dir.join("release.json");
    let head_json_path = artifacts_dir.join("release-head.json");
    if release_json_path.is_file() {
        let release_bytes = std::fs::read(&release_json_path)
            .context("Failed to read artifacts/release.json for verification")?;
        elastos_server::crypto::verify_release_envelope(
            &release_bytes,
            "elastos.release.v1",
            &key.signer_did,
        )
        .context("release.json signature verification failed after publish")?;
    }
    if head_json_path.is_file() {
        let head_bytes = std::fs::read(&head_json_path)
            .context("Failed to read artifacts/release-head.json for verification")?;
        elastos_server::crypto::verify_release_envelope(
            &head_bytes,
            "elastos.release.head.v1",
            &key.signer_did,
        )
        .context("release-head.json signature verification failed after publish")?;
    }

    let mut next_state = previous_state;
    next_state.last_release_cid = read_state_value(&state_dir.join("last-release-cid"))?;
    next_state.last_head_cid = read_state_value(&state_dir.join("last-release-head-cid"))?;
    next_state.last_version = Some(options.version);
    next_state.last_published_at = Some(now_unix()?);
    save_publish_state(&state_path, &next_state)?;

    if let (Some(release_cid), Some(head_cid)) = (
        next_state.last_release_cid.as_deref(),
        next_state.last_head_cid.as_deref(),
    ) {
        let artifacts_dir = workspace_root.join("artifacts");
        let current_entry =
            build_release_ledger_entry(&artifacts_dir, release_cid, head_cid, &selected_capsules)?;
        let ledger_path = release_ledger_path(&data_dir);
        let mut ledger = load_release_ledger(&ledger_path)?;
        let previous_entry = ledger
            .entries
            .iter()
            .rev()
            .find(|entry| {
                entry.channel == current_entry.channel && entry.head_cid != current_entry.head_cid
            })
            .cloned();
        ledger.upsert(current_entry.clone());
        save_release_ledger(&ledger_path, &ledger)?;
        print_release_diff_summary(&current_entry, previous_entry.as_ref(), &ledger_path);
        let notes = operator_release_notes(&current_entry, previous_entry.as_ref());
        if !notes.is_empty() {
            println!("  Notes:");
            for note in notes {
                println!("    - {}", note);
            }
        }
        match announce_release_head(&current_entry).await {
            Ok(topics) => println!("  Gossip:  announced on {}", topics.join(", ")),
            Err(error) => eprintln!("  Gossip skipped: {}", error),
        }
    }

    Ok(())
}

fn bootstrap_required(
    options: &PublishReleaseOptions,
    source_bootstrap: &anyhow::Result<Option<String>>,
) -> bool {
    if options.allow_no_bootstrap || options.dry_run || options.preflight_only {
        return false;
    }
    !matches!(source_bootstrap, Ok(Some(ticket)) if !ticket.trim().is_empty())
}

async fn discover_source_connect_ticket() -> anyhow::Result<Option<String>> {
    // Get ticket from the running runtime's built-in Carrier (via HTTP API).
    // No peer-provider spawn — Carrier is built into the runtime.
    discover_source_connect_ticket_from_runtime().await
}

async fn discover_source_connect_ticket_from_runtime() -> anyhow::Result<Option<String>> {
    let data_dir = elastos_server::sources::default_data_dir();
    let coords_path = super::shell_cmd::runtime_coord_path(&data_dir);
    let Some(coords) = super::shell_cmd::read_runtime_coords(&coords_path).await else {
        return Ok(None);
    };

    let expected_version = env!("ELASTOS_VERSION");
    let actual_version = runtime_version_from_runtime_api(&coords.api_url)
        .await
        .ok_or_else(|| {
            anyhow::anyhow!(
                "trusted-source runtime health unavailable at {}. Refresh the canonical source runtime before publish.",
                coords.api_url
            )
        })?;
    let expected_dev_version = format!("{}-dev", expected_version);
    if actual_version != expected_version && actual_version != expected_dev_version {
        anyhow::bail!(
            "trusted-source runtime is stale (running {}, expected {} or {}). Refresh the canonical source runtime before publish.",
            actual_version,
            expected_version,
            expected_dev_version
        );
    }

    let tokens = super::shell_cmd::attach_to_runtime(&coords).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let response = client
        .post(format!("{}/api/provider/peer/get_ticket", coords.api_url))
        .bearer_auth(&tokens.shell_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "live runtime peer ticket request failed ({})",
            response.status()
        );
    }
    let body: serde_json::Value = response.json().await?;
    if body["status"].as_str() == Some("error") {
        anyhow::bail!(
            "live runtime peer ticket request failed: {}",
            body["message"].as_str().unwrap_or("unknown error")
        );
    }
    Ok(body
        .get("data")
        .and_then(|v| v.get("ticket"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

async fn runtime_version_from_runtime_api(api_url: &str) -> Option<String> {
    let api_base = elastos_server::local_http::LoopbackHttpBaseUrl::parse(api_url).ok()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;
    let resp = client
        .get(api_base.join("/api/health").ok()?)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Resolve the cargo target directory, respecting `.cargo/config.toml` target-dir.
fn cargo_target_dir(ws_root: &Path) -> PathBuf {
    let config_path = ws_root.join("elastos/.cargo/config.toml");
    if let Ok(contents) = std::fs::read_to_string(&config_path) {
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("target-dir") {
                if let Some(val) = trimmed
                    .split('=')
                    .nth(1)
                    .map(|s| s.trim().trim_matches('"').trim_matches('\''))
                {
                    if !val.is_empty() {
                        return PathBuf::from(val);
                    }
                }
            }
        }
    }
    ws_root.join("elastos/target")
}

fn publish_state_dir(data_dir: &Path) -> PathBuf {
    publisher_root_path(data_dir)
}

fn publish_state_path(data_dir: &Path) -> PathBuf {
    publisher_publish_state_path(data_dir)
}

fn release_ledger_path(data_dir: &Path) -> PathBuf {
    data_dir.join("releases").join("cids.json")
}

fn release_key_path(default_override: Option<&Path>) -> PathBuf {
    default_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| elastos_server::sources::default_data_dir().join("release-key.hex"))
}

fn load_or_create_release_key(path_override: Option<&Path>) -> anyhow::Result<PublishKey> {
    let path = release_key_path(path_override);
    if path.exists() {
        let hex_str = std::fs::read_to_string(&path)?;
        let bytes = hex::decode(hex_str.trim())
            .map_err(|e| anyhow::anyhow!("Invalid release signing key: {}", e))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Release signing key must be 32 bytes"))?;
        let signing_key = signature::SigningKey::from_bytes(&arr);
        return Ok(PublishKey {
            signer_did: elastos_server::crypto::encode_did_key(&signing_key.verifying_key()),
            path,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (signing_key, verifying_key) = generate_keypair();
    std::fs::write(&path, hex::encode(signing_key.to_bytes()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(PublishKey {
        signer_did: elastos_server::crypto::encode_did_key(&verifying_key),
        path,
    })
}

fn inspect_release_key(path_override: Option<&Path>) -> anyhow::Result<PublishKeyPreview> {
    let path = release_key_path(path_override);
    if !path.exists() {
        return Ok(PublishKeyPreview {
            path,
            signer_did: None,
        });
    }

    let hex_str = std::fs::read_to_string(&path)?;
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| anyhow::anyhow!("Invalid release signing key: {}", e))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Release signing key must be 32 bytes"))?;
    let signing_key = signature::SigningKey::from_bytes(&arr);
    Ok(PublishKeyPreview {
        signer_did: Some(elastos_server::crypto::encode_did_key(
            &signing_key.verifying_key(),
        )),
        path,
    })
}

fn load_publish_state(path: &Path) -> anyhow::Result<PublishState> {
    if !path.exists() {
        return Ok(PublishState::default());
    }
    let data = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

fn default_release_ledger_schema() -> String {
    "elastos.release.ledger/v1".to_string()
}

impl ReleaseLedger {
    fn empty() -> Self {
        Self {
            schema: default_release_ledger_schema(),
            entries: Vec::new(),
        }
    }

    fn upsert(&mut self, entry: ReleaseLedgerEntry) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|item| item.head_cid == entry.head_cid)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }
}

fn load_release_ledger(path: &Path) -> anyhow::Result<ReleaseLedger> {
    if !path.exists() {
        return Ok(ReleaseLedger::empty());
    }
    let data = std::fs::read_to_string(path)?;
    let mut ledger: ReleaseLedger = serde_json::from_str(&data)?;
    if ledger.schema.is_empty() {
        ledger.schema = default_release_ledger_schema();
    }
    Ok(ledger)
}

fn save_release_ledger(path: &Path, ledger: &ReleaseLedger) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(ledger)?)?;
    Ok(())
}

fn save_publish_state(path: &Path, state: &PublishState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn load_capsule_manifests(
    workspace_root: &Path,
) -> anyhow::Result<BTreeMap<String, CapsuleManifest>> {
    let mut manifests = BTreeMap::new();
    for rel in ["capsules", "elastos/capsules"] {
        let base = workspace_root.join(rel);
        if !base.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&base)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let capsule_json = entry.path().join("capsule.json");
            if !capsule_json.is_file() {
                continue;
            }
            let manifest: CapsuleManifest =
                serde_json::from_str(&std::fs::read_to_string(&capsule_json)?)
                    .with_context(|| format!("Failed to parse {}", capsule_json.display()))?;
            if let Some(existing) = manifests.insert(manifest.name.clone(), manifest) {
                anyhow::bail!(
                    "Duplicate capsule '{}' discovered while scanning workspace",
                    existing.name
                );
            }
        }
    }
    Ok(manifests)
}

#[cfg(test)]
fn discover_available_capsules(workspace_root: &Path) -> anyhow::Result<Vec<String>> {
    Ok(load_capsule_manifests(workspace_root)?
        .into_keys()
        .collect::<Vec<_>>())
}

fn publish_profile_capsules(profile: &str, available: &[String]) -> anyhow::Result<Vec<String>> {
    let mut selected = match profile {
        "demo" => DEFAULT_PUBLISH_CAPSULES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
        "providers" => vec![
            "did-provider".to_string(),
            "ipfs-provider".to_string(),
            "localhost-provider".to_string(),
            "tunnel-provider".to_string(),
        ],
        "full" => available.to_vec(),
        other => {
            anyhow::bail!(
                "Unknown publish profile '{}'. Available profiles: demo, providers, full",
                other
            );
        }
    };
    selected.sort();
    selected.dedup();
    Ok(selected)
}

fn select_capsules(
    profile: &str,
    requested: &[String],
    manifests: &BTreeMap<String, CapsuleManifest>,
) -> anyhow::Result<Vec<String>> {
    let requested = if requested.is_empty() {
        publish_profile_capsules(profile, &manifests.keys().cloned().collect::<Vec<_>>())?
    } else {
        requested
            .iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
    };

    let mut selected = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    let available = manifests.keys().cloned().collect::<Vec<_>>();
    for name in &requested {
        expand_capsule_dependencies(name, manifests, &available, &mut visiting, &mut selected)?;
    }
    Ok(selected.into_iter().collect())
}

fn expand_capsule_dependencies(
    name: &str,
    manifests: &BTreeMap<String, CapsuleManifest>,
    available: &[String],
    visiting: &mut BTreeSet<String>,
    selected: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    let Some(manifest) = manifests.get(name) else {
        anyhow::bail!(
            "Unknown capsule '{}'. Available capsules: {}",
            name,
            available.join(", ")
        );
    };

    if selected.contains(name) {
        return Ok(());
    }
    if !visiting.insert(name.to_string()) {
        anyhow::bail!(
            "Capsule dependency cycle detected while expanding '{}'",
            name
        );
    }

    for requirement in &manifest.requires {
        if requirement.kind == RequirementKind::Capsule {
            if !manifests.contains_key(&requirement.name) {
                anyhow::bail!(
                    "Capsule '{}' requires capsule '{}', but it was not found in the workspace",
                    name,
                    requirement.name
                );
            }
            expand_capsule_dependencies(
                &requirement.name,
                manifests,
                available,
                visiting,
                selected,
            )?;
        }
    }

    visiting.remove(name);
    selected.insert(name.to_string());
    Ok(())
}

fn validate_publish_inputs(
    options: &PublishReleaseOptions,
    workspace_root: &Path,
    selected_capsules: &[String],
) -> anyhow::Result<()> {
    validate_release_channel(&options.channel)?;
    if options.version.trim().is_empty() {
        anyhow::bail!("Version cannot be empty");
    }
    if options.version.chars().any(char::is_whitespace) {
        anyhow::bail!("Version cannot contain whitespace");
    }
    if options.public_with_sudo && !options.public_url {
        anyhow::bail!("--public-with-sudo requires --public-url");
    }
    if let Some(ipfs_provider_bin) = &options.ipfs_provider_bin {
        if !ipfs_provider_bin.is_file() {
            anyhow::bail!(
                "Requested --ipfs-provider-bin does not exist: {}",
                ipfs_provider_bin.display()
            );
        }
    }
    if options.skip_build {
        let elastos_bin = cargo_target_dir(workspace_root).join("release/elastos");
        if !elastos_bin.is_file() {
            anyhow::bail!(
                "--skip-build requested but missing {}",
                elastos_bin.display()
            );
        }
        if let Some(cross) = &options.cross {
            let details = cross_build_details(cross, workspace_root)?;
            let cross_bin = details.binary_path;
            if !cross_bin.is_file() {
                anyhow::bail!(
                    "--cross {} with --skip-build requested but missing {}",
                    cross,
                    cross_bin.display()
                );
            }
        }
    }
    if options.skip_rootfs {
        let missing = selected_capsules
            .iter()
            .filter_map(|name| {
                let artifact = workspace_root
                    .join("artifacts")
                    .join(format!("{}.capsule.tar.gz", name));
                if artifact.is_file() {
                    None
                } else {
                    Some(format!("{} ({})", name, artifact.display()))
                }
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            anyhow::bail!(
                "--skip-rootfs requested but missing capsule artifacts: {}",
                missing.join(", ")
            );
        }
        if let Some(cross) = &options.cross {
            let details = cross_build_details(cross, workspace_root)?;
            let missing_cross = selected_capsules
                .iter()
                .filter_map(|name| {
                    let artifact = workspace_root
                        .join(details.artifacts_dir)
                        .join(format!("{}.capsule.tar.gz", name));
                    if artifact.is_file() {
                        None
                    } else {
                        Some(format!("{} ({})", name, artifact.display()))
                    }
                })
                .collect::<Vec<_>>();
            if !missing_cross.is_empty() {
                anyhow::bail!(
                    "--cross {} with --skip-rootfs requested but missing cross capsule artifacts: {}",
                    cross,
                    missing_cross.join(", ")
                );
            }
        }
    }

    let missing_supported = REQUIRED_SUPPORTED_PUBLISH_CAPSULES
        .iter()
        .filter(|name| !selected_capsules.iter().any(|selected| selected == *name))
        .copied()
        .collect::<Vec<_>>();
    if !missing_supported.is_empty() {
        anyhow::bail!(
            "Selected capsules would ship an incomplete supported release. Missing required capsules: {}",
            missing_supported.join(", ")
        );
    }

    Ok(())
}

struct CrossBuildDetails {
    binary_path: PathBuf,
    artifacts_dir: &'static str,
}

fn cross_build_details(arch: &str, ws_root: &Path) -> anyhow::Result<CrossBuildDetails> {
    let target_dir = cargo_target_dir(ws_root);
    match arch {
        "aarch64" => Ok(CrossBuildDetails {
            binary_path: target_dir.join("aarch64-unknown-linux-gnu/release/elastos"),
            artifacts_dir: "artifacts-aarch64",
        }),
        other => anyhow::bail!("Unsupported cross architecture: {}", other),
    }
}

struct PublishPreflight {
    script_path: PathBuf,
    ipfs_provider_bin: Option<PathBuf>,
    key_path: PathBuf,
    selected_capsules: Vec<String>,
    available_tools: Vec<String>,
}

fn run_publish_preflight(
    options: &PublishReleaseOptions,
    workspace_root: &Path,
    selected_capsules: &[String],
) -> anyhow::Result<PublishPreflight> {
    let script_path = workspace_root.join("scripts/publish-release.sh");
    if !script_path.is_file() {
        anyhow::bail!("Missing publish script: {}", script_path.display());
    }

    if !options.skip_build && which_in_path("cargo").is_none() {
        anyhow::bail!("`cargo` not found in PATH");
    }

    let mut available_tools = Vec::new();
    for tool in ["bash", "jq", "python3", "curl"] {
        let path =
            which_in_path(tool).ok_or_else(|| anyhow::anyhow!("`{}` not found in PATH", tool))?;
        available_tools.push(format!("{}={}", tool, path.display()));
    }
    let sha_tool = which_in_path("sha256sum")
        .map(|path| format!("sha256sum={}", path.display()))
        .or_else(|| which_in_path("shasum").map(|path| format!("shasum={}", path.display())))
        .ok_or_else(|| anyhow::anyhow!("Neither `sha256sum` nor `shasum` found in PATH"))?;
    available_tools.push(sha_tool);

    if !options.skip_rootfs {
        let rootfs_script = workspace_root.join("scripts/build/build-rootfs.sh");
        if !rootfs_script.is_file() {
            anyhow::bail!("Missing rootfs build script: {}", rootfs_script.display());
        }
        let mke2fs =
            which_in_path("mke2fs").ok_or_else(|| anyhow::anyhow!("`mke2fs` not found in PATH"))?;
        available_tools.push(format!("mke2fs={}", mke2fs.display()));
        let busybox = which_in_path("busybox")
            .ok_or_else(|| anyhow::anyhow!("`busybox` not found in PATH"))?;
        available_tools.push(format!("busybox={}", busybox.display()));
    }

    for capsule in selected_capsules {
        let dir = resolve_capsule_dir(workspace_root, capsule)
            .ok_or_else(|| anyhow::anyhow!("Capsule '{}' has no workspace directory", capsule))?;
        if !dir.join("capsule.json").is_file() {
            anyhow::bail!(
                "Capsule '{}' is missing {}",
                capsule,
                dir.join("capsule.json").display()
            );
        }
    }

    let ipfs_provider_bin = match &options.ipfs_provider_bin {
        Some(path) => Some(path.clone()),
        None => find_ipfs_provider_binary(workspace_root),
    };
    if ipfs_provider_bin.is_none() {
        anyhow::bail!(
            "No ipfs-provider binary found. Build/install it first or pass --ipfs-provider-bin"
        );
    }
    if let Some(path) = &ipfs_provider_bin {
        is_executable_file(path).then_some(()).ok_or_else(|| {
            anyhow::anyhow!("ipfs-provider is not executable: {}", path.display())
        })?;
        available_tools.push(format!("ipfs-provider={}", path.display()));
    }

    Ok(PublishPreflight {
        script_path,
        ipfs_provider_bin,
        key_path: release_key_path(options.key.as_deref()),
        selected_capsules: selected_capsules.to_vec(),
        available_tools,
    })
}

fn resolve_capsule_dir(workspace_root: &Path, capsule: &str) -> Option<PathBuf> {
    let root = workspace_root.join("capsules").join(capsule);
    if root.is_dir() {
        return Some(root);
    }
    let core = workspace_root.join("elastos/capsules").join(capsule);
    if core.is_dir() {
        return Some(core);
    }
    None
}

fn which_in_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_ipfs_provider_binary(workspace_root: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("ELASTOS_IPFS_PROVIDER_BIN") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::var_os("ELASTOS_CAPSULE_BIN_DIR") {
        candidates.push(PathBuf::from(dir).join("ipfs-provider"));
    }
    let target_dir = cargo_target_dir(workspace_root);
    candidates.push(target_dir.join("release/ipfs-provider"));
    candidates.push(workspace_root.join("capsules/ipfs-provider/target/release/ipfs-provider"));
    candidates.push(elastos_server::sources::default_data_dir().join("bin/ipfs-provider"));
    if let Some(path) = which_in_path("ipfs-provider") {
        candidates.push(path);
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn print_preflight_report(options: &PublishReleaseOptions, preflight: &PublishPreflight) {
    println!("ElastOS publish-release preflight");
    println!("  Version:   {}", options.version);
    println!("  Channel:   {}", options.channel);
    println!("  Profile:   {}", options.profile);
    println!("  Script:    {}", preflight.script_path.display());
    println!("  Key path:  {}", preflight.key_path.display());
    println!(
        "  IPFS bin:  {}",
        preflight
            .ipfs_provider_bin
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string())
    );
    println!("  Capsules:  {}", preflight.selected_capsules.join(", "));
    if let Some(cross) = &options.cross {
        println!("  Cross:     {}", cross);
    }
    println!("  Tools:     {}", preflight.available_tools.join(", "));
    println!("  Result:    preflight passed");
}

fn operator_release_notes(
    current: &ReleaseLedgerEntry,
    previous: Option<&ReleaseLedgerEntry>,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(commit) = current_git_commit() {
        notes.push(format!("commit {}", commit));
    }

    for (platform_name, platform) in &current.platforms {
        let changed = changed_capsules(
            previous
                .and_then(|entry| entry.platforms.get(platform_name))
                .map(|entry| &entry.capsules),
            &platform.capsules,
        );
        if !changed.is_empty() {
            notes.push(format!(
                "{} changed capsules: {}",
                platform_name,
                changed.join(", ")
            ));
        }
    }

    if current.selected_capsules.iter().any(|name| name == "chat") {
        notes.push("retest chat keyboard input, history persistence, and peer sync".to_string());
    }
    if current
        .selected_capsules
        .iter()
        .any(|name| matches!(name.as_str(), "shell" | "localhost-provider"))
    {
        notes.push("retest shell launch path and multi-peer chat connectivity".to_string());
    }
    if current
        .selected_capsules
        .iter()
        .any(|name| matches!(name.as_str(), "ipfs-provider" | "tunnel-provider"))
    {
        notes.push("retest install/update flow and public installer URL path".to_string());
    }
    if current
        .selected_capsules
        .iter()
        .any(|name| name == "did-provider")
    {
        notes.push("retest DID/provider auth flows used by update verification".to_string());
    }

    notes
}

fn current_git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if commit.is_empty() {
        None
    } else {
        Some(commit.to_string())
    }
}

fn read_state_value(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(path)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn build_release_ledger_entry(
    artifacts_dir: &Path,
    release_cid: &str,
    head_cid: &str,
    selected_capsules: &[String],
) -> anyhow::Result<ReleaseLedgerEntry> {
    let release: ReleaseEnvelope = serde_json::from_slice(
        &std::fs::read(artifacts_dir.join("release.json"))
            .with_context(|| format!("Missing {}", artifacts_dir.join("release.json").display()))?,
    )?;
    let head: ReleaseHeadEnvelope = serde_json::from_slice(
        &std::fs::read(artifacts_dir.join("release-head.json")).with_context(|| {
            format!(
                "Missing {}",
                artifacts_dir.join("release-head.json").display()
            )
        })?,
    )?;

    if head.payload.latest_release_cid != release_cid {
        anyhow::bail!(
            "Release CID mismatch between state ({}) and artifacts ({})",
            release_cid,
            head.payload.latest_release_cid
        );
    }

    let mut platforms = BTreeMap::new();
    for (platform_name, platform) in &release.payload.platforms {
        let components_path = components_artifact_path(artifacts_dir, platform_name);
        let manifest: elastos_server::setup::ComponentsManifest = serde_json::from_slice(
            &std::fs::read(&components_path)
                .with_context(|| format!("Missing {}", components_path.display()))?,
        )?;
        let capsules = manifest
            .capsules
            .into_iter()
            .map(|(name, entry)| (name, entry.cid))
            .collect();
        platforms.insert(
            platform_name.clone(),
            ReleaseLedgerPlatform {
                binary_cid: platform.binary.cid.clone(),
                components_cid: platform.components.cid.clone(),
                capsules,
            },
        );
    }

    Ok(ReleaseLedgerEntry {
        version: release.payload.version,
        channel: release.payload.channel,
        release_cid: release_cid.to_string(),
        head_cid: head_cid.to_string(),
        published_at: release.payload.released_at,
        signer_did: release.signer_did,
        selected_capsules: selected_capsules.to_vec(),
        platforms,
    })
}

fn components_artifact_path(artifacts_dir: &Path, platform: &str) -> PathBuf {
    let arch = platform.split('-').next().unwrap_or(platform);
    artifacts_dir.join(format!("components-{}.json", arch))
}

fn print_release_diff_summary(
    current: &ReleaseLedgerEntry,
    previous: Option<&ReleaseLedgerEntry>,
    ledger_path: &Path,
) {
    println!();
    println!("Publish summary");
    println!("  Version: {}", current.version);
    println!("  Channel: {}", current.channel);
    println!("  Release: {}", current.release_cid);
    println!("  Head:    {}", current.head_cid);
    println!("  Ledger:  {}", ledger_path.display());

    match previous {
        Some(previous) => {
            println!("  Previous: {} ({})", previous.version, previous.head_cid);
            for (platform_name, platform) in &current.platforms {
                let previous_platform = previous.platforms.get(platform_name);
                let runtime_changed = previous_platform.is_none_or(|prev| {
                    prev.binary_cid != platform.binary_cid
                        || prev.components_cid != platform.components_cid
                });
                println!(
                    "  {} runtime: {}",
                    platform_name,
                    if runtime_changed {
                        "changed"
                    } else {
                        "unchanged"
                    }
                );
                let changed_capsules = changed_capsules(
                    previous_platform.map(|item| &item.capsules),
                    &platform.capsules,
                );
                if changed_capsules.is_empty() {
                    println!("  {} capsules: unchanged", platform_name);
                } else {
                    println!(
                        "  {} capsules: {}",
                        platform_name,
                        changed_capsules.join(", ")
                    );
                }
            }
        }
        None => {
            println!("  Previous: none recorded for this channel");
            for platform_name in current.platforms.keys() {
                println!("  {} runtime: first recorded publish", platform_name);
            }
        }
    }
}

/// Announce a release via the running runtime's built-in Carrier (HTTP API).
/// No peer-provider process spawn — Carrier is built into the runtime.
async fn announce_release_head(entry: &ReleaseLedgerEntry) -> anyhow::Result<Vec<String>> {
    let data_dir = elastos_server::sources::default_data_dir();
    let coords_path = super::shell_cmd::runtime_coord_path(&data_dir);
    let coords = super::shell_cmd::read_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No running runtime found. Start `elastos serve` first for gossip announcements."
            )
        })?;

    let tokens = super::shell_cmd::attach_to_runtime(&coords).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let topics = release_discovery_topics(None, &entry.signer_did, &entry.channel);

    for topic in &topics {
        // Join topic via built-in CarrierGossipProvider
        let _ = client
            .post(format!("{}/api/provider/peer/gossip_join", coords.api_url))
            .bearer_auth(&tokens.shell_token)
            .json(&serde_json::json!({"topic": topic}))
            .send()
            .await;

        // Broadcast announcement
        let announcement = serde_json::json!({
            "head_cid": entry.head_cid,
            "release_cid": entry.release_cid,
            "version": entry.version,
            "channel": entry.channel,
            "signer_did": entry.signer_did,
            "discovery_uri": source_discovery_uri(&entry.signer_did, &entry.channel),
        });
        client
            .post(format!("{}/api/provider/peer/gossip_send", coords.api_url))
            .bearer_auth(&tokens.shell_token)
            .json(&serde_json::json!({
                "topic": topic,
                "message": announcement.to_string(),
                "sender": "publisher",
                "sender_id": entry.signer_did,
                "ts": entry.published_at,
            }))
            .send()
            .await
            .context("gossip announcement failed")?;
    }

    Ok(topics)
}

fn changed_capsules(
    previous: Option<&BTreeMap<String, String>>,
    current: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (name, cid) in current {
        match previous.and_then(|items| items.get(name)) {
            Some(previous_cid) if previous_cid == cid => {}
            _ => changed.push(name.clone()),
        }
    }
    if let Some(previous) = previous {
        for name in previous.keys() {
            if !current.contains_key(name) {
                changed.push(name.clone());
            }
        }
    }
    changed.sort();
    changed.dedup();
    changed
}

fn now_unix() -> anyhow::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_secs())
}

fn print_publish_plan(
    options: &PublishReleaseOptions,
    key: &PublishKeyPreview,
    state_path: &Path,
    previous_state: &PublishState,
    available_capsules: &[String],
    selected_capsules: &[String],
) {
    println!("ElastOS publish-release (dry run)");
    println!("  Version:   {}", options.version);
    println!("  Channel:   {}", options.channel);
    println!("  Profile:   {}", options.profile);
    println!(
        "  Signer:    {}",
        key.signer_did
            .as_deref()
            .unwrap_or("(will be generated on first real publish)")
    );
    println!("  Key path:  {}", key.path.display());
    println!("  Capsules:  {}", selected_capsules.join(", "));
    println!("  Available: {}", available_capsules.join(", "));
    println!(
        "  Build:     {}",
        if options.skip_build {
            "reuse existing binaries (--skip-build)"
        } else {
            "build runtime and selected capsules"
        }
    );
    println!(
        "  Preflight: {}",
        if options.preflight_only {
            "validate prerequisites only"
        } else {
            "not requested"
        }
    );
    println!(
        "  Rootfs:    {}",
        if options.skip_rootfs {
            "reuse artifacts/ (*.capsule.tar.gz)"
        } else {
            "rebuild selected capsule rootfs artifacts"
        }
    );
    println!(
        "  Public URL: {}",
        if options.public_url {
            format!(
                "enabled (addr={}, timeout={}s{})",
                options.gateway_addr,
                options.public_timeout,
                if options.public_with_sudo {
                    ", sudo"
                } else {
                    ""
                }
            )
        } else {
            "disabled".to_string()
        }
    );
    println!(
        "  Cross:     {}",
        options.cross.as_deref().unwrap_or("host platform only")
    );
    println!(
        "  IPFS bin:  {}",
        options
            .ipfs_provider_bin
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "auto-detect".to_string())
    );
    println!(
        "  Prev head: {}",
        previous_state
            .last_head_cid
            .as_deref()
            .unwrap_or("none recorded")
    );
    println!(
        "  Prev rel:  {}",
        previous_state
            .last_release_cid
            .as_deref()
            .unwrap_or("none recorded")
    );
    println!(
        "  Prev ver:  {}",
        previous_state
            .last_version
            .as_deref()
            .unwrap_or("none recorded")
    );
    println!("  State:     {}", state_path.display());
    println!();
    println!("No build or upload actions were run.");
}

#[cfg(test)]
mod tests {
    use super::{
        bootstrap_required, build_release_ledger_entry, changed_capsules,
        discover_available_capsules, load_publish_state, operator_release_notes,
        publish_profile_capsules, release_discovery_topics, save_publish_state, select_capsules,
        source_discovery_uri, validate_publish_inputs, PublishReleaseOptions, PublishState,
        ReleaseLedgerEntry, ReleaseLedgerPlatform,
    };
    use elastos_common::{
        CapsuleManifest, CapsuleType, MicroVmConfig, Permissions, RequirementKind, ResourceLimits,
    };
    use std::collections::BTreeMap;
    use std::path::Path;

    fn test_manifest(name: &str, capsule_requires: &[&str]) -> CapsuleManifest {
        CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.to_string(),
            version: "0.1.0".to_string(),
            name: name.to_string(),
            description: None,
            author: None,
            capsule_type: CapsuleType::MicroVM,
            entrypoint: "rootfs.ext4".to_string(),
            requires: capsule_requires
                .iter()
                .map(|requirement| elastos_common::CapsuleRequirement {
                    name: (*requirement).to_string(),
                    kind: RequirementKind::Capsule,
                })
                .collect(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: Some(MicroVmConfig::default()),
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    fn test_manifests(entries: &[(&str, &[&str])]) -> BTreeMap<String, CapsuleManifest> {
        entries
            .iter()
            .map(|(name, requires)| ((*name).to_string(), test_manifest(name, requires)))
            .collect()
    }

    #[test]
    fn test_select_capsules_defaults_to_public_publish_set() {
        let manifests = test_manifests(&[
            ("chat", &["did-provider"]),
            ("chat-wasm", &[]),
            ("did-provider", &[]),
            ("localhost-provider", &[]),
            ("shell", &[]),
            ("tunnel-provider", &[]),
        ]);
        let selected = select_capsules("demo", &[], &manifests).unwrap();
        assert_eq!(
            selected,
            vec![
                "chat".to_string(),
                "chat-wasm".to_string(),
                "did-provider".to_string(),
                "localhost-provider".to_string(),
                "shell".to_string(),
                "tunnel-provider".to_string(),
            ]
        );
    }

    #[test]
    fn test_select_capsules_rejects_unknown_name() {
        let manifests = test_manifests(&[("chat", &[])]);
        let err = select_capsules("demo", &["missing".to_string()], &manifests).unwrap_err();
        assert!(err.to_string().contains("Unknown capsule"));
    }

    #[test]
    fn test_select_capsules_expands_transitive_capsule_dependencies() {
        let manifests = test_manifests(&[
            ("chat", &["did-provider"]),
            ("did-provider", &[]),
            ("shell", &[]),
        ]);
        let selected = select_capsules(
            "demo",
            &["shell".to_string(), "chat".to_string()],
            &manifests,
        )
        .unwrap();
        assert_eq!(
            selected,
            vec![
                "chat".to_string(),
                "did-provider".to_string(),
                "shell".to_string(),
            ]
        );
    }

    #[test]
    fn test_select_capsules_rejects_missing_transitive_dependency() {
        let manifests = test_manifests(&[("chat", &["ipfs-provider"])]);
        let err = select_capsules("demo", &["chat".to_string()], &manifests).unwrap_err();
        assert!(err.to_string().contains("requires capsule 'ipfs-provider'"));
    }

    #[test]
    fn test_publish_profile_full_uses_all_available_capsules() {
        let available = vec![
            "chat".to_string(),
            "shell".to_string(),
            "peer-provider".to_string(),
        ];
        assert_eq!(
            publish_profile_capsules("full", &available).unwrap(),
            vec![
                "chat".to_string(),
                "peer-provider".to_string(),
                "shell".to_string()
            ]
        );
    }

    #[test]
    fn test_publish_state_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("publish-state.json");
        let state = PublishState {
            last_release_cid: Some("release-cid".to_string()),
            last_head_cid: Some("head-cid".to_string()),
            last_version: Some("0.11.0".to_string()),
            last_published_at: Some(42),
        };
        save_publish_state(&path, &state).unwrap();
        assert_eq!(load_publish_state(&path).unwrap(), state);
    }

    #[test]
    fn test_release_discovery_topics_include_scoped_and_legacy_topics() {
        let discovery_uri = source_discovery_uri("did:key:z6Mktest", "stable");
        let topics = release_discovery_topics(Some(&discovery_uri), "did:key:z6Mktest", "stable");
        assert_eq!(topics.len(), 3);
        assert!(topics[0].starts_with("elastos:source:"));
        assert!(topics[1].starts_with("elastos:releases:stable:"));
        assert_eq!(topics[2], "elastos:releases");
    }

    #[test]
    fn test_discover_available_capsules_reads_workspace_layout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let capsules = discover_available_capsules(&root).unwrap();
        assert!(capsules.iter().any(|name| name == "chat"));
        assert!(capsules.iter().any(|name| name == "shell"));
        assert!(capsules.iter().any(|name| name == "ipfs-provider"));
    }

    #[test]
    fn test_changed_capsules_reports_new_updated_and_removed_entries() {
        let previous = ReleaseLedgerPlatform {
            binary_cid: "old-binary".to_string(),
            components_cid: "old-components".to_string(),
            capsules: BTreeMap::from([
                ("chat".to_string(), "cid-chat-1".to_string()),
                ("peer-provider".to_string(), "cid-peer-1".to_string()),
            ]),
        };
        let current = BTreeMap::from([
            ("chat".to_string(), "cid-chat-2".to_string()),
            ("did-provider".to_string(), "cid-did-1".to_string()),
        ]);
        assert_eq!(
            changed_capsules(Some(&previous.capsules), &current),
            vec![
                "chat".to_string(),
                "did-provider".to_string(),
                "peer-provider".to_string()
            ]
        );
    }

    #[test]
    fn test_build_release_ledger_entry_reads_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts_dir = temp.path();
        std::fs::write(
            artifacts_dir.join("release.json"),
            serde_json::json!({
                "payload": {
                    "channel": "stable",
                    "version": "0.11.0",
                    "released_at": 42,
                    "platforms": {
                        "x86_64-linux": {
                            "binary": { "cid": "binary-cid" },
                            "components": { "cid": "components-cid" }
                        }
                    }
                },
                "signer_did": "did:key:z6Mktest"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            artifacts_dir.join("release-head.json"),
            serde_json::json!({
                "payload": {
                    "latest_release_cid": "release-cid"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            artifacts_dir.join("components-x86_64.json"),
            serde_json::json!({
                "external": {},
                "profiles": {},
                "capsules": {
                    "chat": { "cid": "cid-chat-1", "sha256": "a", "size": 1, "platforms": ["x86_64-linux"] }
                }
            })
            .to_string(),
        )
        .unwrap();

        let entry = build_release_ledger_entry(
            artifacts_dir,
            "release-cid",
            "head-cid",
            &["chat".to_string()],
        )
        .unwrap();
        assert_eq!(entry.version, "0.11.0");
        assert_eq!(entry.channel, "stable");
        assert_eq!(entry.release_cid, "release-cid");
        assert_eq!(entry.head_cid, "head-cid");
        assert_eq!(entry.signer_did, "did:key:z6Mktest");
        assert_eq!(
            entry
                .platforms
                .get("x86_64-linux")
                .unwrap()
                .capsules
                .get("chat"),
            Some(&"cid-chat-1".to_string())
        );
    }

    #[test]
    fn test_validate_publish_inputs_requires_rootfs_artifacts_for_skip_rootfs() {
        let temp = tempfile::tempdir().unwrap();
        let options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            profile: "demo".to_string(),
            skip_build: false,
            skip_rootfs: true,
            cross: None,
            capsules: Vec::new(),
            key: None,
            dry_run: true,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        };
        let err =
            validate_publish_inputs(&options, temp.path(), &["chat".to_string()]).unwrap_err();
        assert!(err.to_string().contains("--skip-rootfs requested"));
    }

    #[test]
    fn test_validate_publish_inputs_requires_cross_binary_for_skip_build() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("elastos/target/release")).unwrap();
        std::fs::write(temp.path().join("elastos/target/release/elastos"), b"bin").unwrap();

        let options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            profile: "demo".to_string(),
            skip_build: true,
            skip_rootfs: false,
            cross: Some("aarch64".to_string()),
            capsules: Vec::new(),
            key: None,
            dry_run: true,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        };
        let err =
            validate_publish_inputs(&options, temp.path(), &["chat".to_string()]).unwrap_err();
        assert!(err
            .to_string()
            .contains("--cross aarch64 with --skip-build requested"));
    }

    #[test]
    fn test_validate_publish_inputs_requires_cross_rootfs_artifacts_for_skip_rootfs() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("artifacts")).unwrap();
        std::fs::write(
            temp.path().join("artifacts/chat.capsule.tar.gz"),
            b"capsule",
        )
        .unwrap();

        let options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            profile: "demo".to_string(),
            skip_build: false,
            skip_rootfs: true,
            cross: Some("aarch64".to_string()),
            capsules: Vec::new(),
            key: None,
            dry_run: true,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        };
        let err =
            validate_publish_inputs(&options, temp.path(), &["chat".to_string()]).unwrap_err();
        assert!(err
            .to_string()
            .contains("--cross aarch64 with --skip-rootfs requested"));
    }

    #[test]
    fn test_bootstrap_required_for_publish_without_ticket() {
        let options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            profile: "demo".to_string(),
            skip_build: false,
            skip_rootfs: false,
            cross: None,
            capsules: Vec::new(),
            key: None,
            dry_run: false,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        };
        assert!(bootstrap_required(&options, &Ok(None)));
        assert!(bootstrap_required(
            &options,
            &Err(anyhow::anyhow!("ticket unavailable"))
        ));
        assert!(!bootstrap_required(
            &options,
            &Ok(Some("ticket".to_string()))
        ));
    }

    #[test]
    fn test_bootstrap_requirement_can_be_opted_out() {
        let mut options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            profile: "demo".to_string(),
            skip_build: false,
            skip_rootfs: false,
            cross: None,
            capsules: Vec::new(),
            key: None,
            dry_run: false,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: true,
        };
        assert!(!bootstrap_required(&options, &Ok(None)));

        options.allow_no_bootstrap = false;
        options.channel = "canary".to_string();
        assert!(bootstrap_required(&options, &Ok(None)));
    }

    #[test]
    fn test_validate_publish_inputs_rejects_unknown_channel() {
        let temp = tempfile::tempdir().unwrap();
        let options = PublishReleaseOptions {
            version: "0.11.0".to_string(),
            channel: "nightly".to_string(),
            profile: "demo".to_string(),
            skip_build: false,
            skip_rootfs: false,
            cross: None,
            capsules: Vec::new(),
            key: None,
            dry_run: false,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        };
        let err =
            validate_publish_inputs(&options, temp.path(), &["chat".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Allowed channels"));
    }

    #[test]
    fn test_operator_release_notes_flag_chat_and_update_risks() {
        let current = ReleaseLedgerEntry {
            version: "0.11.0".to_string(),
            channel: "stable".to_string(),
            release_cid: "release-cid".to_string(),
            head_cid: "head-cid".to_string(),
            published_at: 42,
            signer_did: "did:key:z6Mktest".to_string(),
            selected_capsules: vec![
                "chat".to_string(),
                "peer-provider".to_string(),
                "ipfs-provider".to_string(),
            ],
            platforms: BTreeMap::from([(
                "x86_64-linux".to_string(),
                ReleaseLedgerPlatform {
                    binary_cid: "bin".to_string(),
                    components_cid: "components".to_string(),
                    capsules: BTreeMap::from([
                        ("chat".to_string(), "cid-chat-2".to_string()),
                        ("peer-provider".to_string(), "cid-peer-1".to_string()),
                    ]),
                },
            )]),
        };
        let previous = ReleaseLedgerEntry {
            version: "0.10.0".to_string(),
            channel: "stable".to_string(),
            release_cid: "old-release".to_string(),
            head_cid: "old-head".to_string(),
            published_at: 1,
            signer_did: "did:key:z6Mktest".to_string(),
            selected_capsules: vec!["chat".to_string()],
            platforms: BTreeMap::from([(
                "x86_64-linux".to_string(),
                ReleaseLedgerPlatform {
                    binary_cid: "old-bin".to_string(),
                    components_cid: "old-components".to_string(),
                    capsules: BTreeMap::from([("chat".to_string(), "cid-chat-1".to_string())]),
                },
            )]),
        };
        let notes = operator_release_notes(&current, Some(&previous));
        assert!(notes.iter().any(|note| note.contains("changed capsules")));
        assert!(notes.iter().any(|note| note.contains("keyboard input")));
        assert!(notes
            .iter()
            .any(|note| note.contains("install/update flow")));
    }
}
